use eyre::{Result, eyre};
use imap::Session;
use log::{debug, warn, error};
use native_tls::{TlsConnector, TlsStream};
use serde::de::{Deserializer, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::net::TcpStream;

use crate::leatherman::{compare, listify};

#[derive(Debug, Deserialize)]
pub struct MessageFilter {
    #[serde(skip_deserializing)]
    pub name: String,
    pub to: Option<Vec<String>>,
    pub cc: Option<Vec<String>>,
    pub fr: Option<String>,
    pub move_to: Option<String>,
    pub star: Option<bool>,
}

#[derive(Debug)]
pub struct IMAPFilter {
    client: Session<TlsStream<TcpStream>>,
    filters: Vec<MessageFilter>,
}

/// Custom deserialization for `filters`, handling a sequence of single-key maps.
pub fn deserialize_filters<'de, D>(deserializer: D) -> Result<Vec<MessageFilter>, D::Error>
where
    D: Deserializer<'de>,
{
    struct FilterList;

    impl<'de> Visitor<'de> for FilterList {
        type Value = Vec<MessageFilter>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a sequence of maps where each map contains a single key-value pair")
        }

        fn visit_seq<M>(self, mut seq: M) -> Result<Self::Value, M::Error>
        where
            M: SeqAccess<'de>,
        {
            let mut filters = Vec::new();
            while let Some(map) = seq.next_element::<HashMap<String, MessageFilter>>()? {
                for (name, mut filter) in map {
                    filter.name = name;
                    filters.push(filter);
                }
            }
            Ok(filters)
        }
    }

    deserializer.deserialize_seq(FilterList)
}

impl IMAPFilter {
    pub fn new(domain: String, username: String, password: String, filters: Vec<MessageFilter>) -> Result<Self> {
        debug!("Initializing IMAP connection to {}", domain);

        let tls = TlsConnector::builder().build()?;
        let client = imap::connect((domain.as_str(), 993), &domain, &tls)
            .map_err(|e| {
                error!("IMAP connection failed: {:?}", e);
                eyre!("IMAP connection failed: {:?}", e)
            })?
            .login(username, password)
            .map_err(|e| {
                error!("IMAP authentication failed: {:?}", e);
                eyre!("IMAP authentication failed: {:?}", e)
            })?;

        debug!("Successfully connected and authenticated to IMAP server.");

        debug!("Filters loaded: {:?}", filters);

        Ok(Self { client, filters })
    }

    fn fetch_messages(&mut self) -> Result<Vec<Message>> {
        debug!("Fetching messages from INBOX");

        self.client.select("INBOX")?;
        let messages = self.client.search("ALL")?;
        debug!("Found {} messages in INBOX", messages.len());

        let fetches = self.client.fetch(
            messages.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(","),
            "RFC822"
        )?;
        debug!("Fetched message details for {} messages", fetches.len());

        let mut results = Vec::new();
        for fetch in fetches.iter() {
            if let Some(body) = fetch.body() {
                debug!("Processing message UID: {}", fetch.message);
                results.push(Message::new(fetch.message, body.to_vec()));
            } else {
                warn!("Message UID: {} has no body", fetch.message);
            }
        }

        debug!("Successfully fetched {} messages", results.len());
        Ok(results)
    }

    fn apply_filters(&mut self, messages: &[Message]) -> Result<()> {
        debug!("Applying filters to {} messages", messages.len());

        for filter in &self.filters {
            debug!("Applying filter: {:?}", filter);

            let filtered: Vec<&Message> = messages.iter().filter(|msg| msg.compare(filter)).collect();
            if filtered.is_empty() {
                debug!("No messages matched filter: {}", filter.name);
                continue;
            }

            let uids: Vec<String> = filtered.iter().map(|m| m.uid.to_string()).collect();
            debug!("Messages matching filter '{}': {:?}", filter.name, uids);

            if let Some(folder) = &filter.move_to {
                debug!("Moving messages {:?} to folder {}", uids, folder);
                self.client.uid_copy(&uids.join(","), folder)
                    .map_err(|e| {
                        error!("Failed to move messages {:?} to {}: {:?}", uids, folder, e);
                        e
                    })?;
            }

            if filter.star.unwrap_or(false) {
                debug!("Starring messages {:?}", uids);
                self.client.uid_store(&uids.join(","), "+FLAGS (\\Flagged)")
                    .map_err(|e| {
                        error!("Failed to star messages {:?}: {:?}", uids, e);
                        e
                    })?;
            }
        }

        debug!("Finished applying filters.");
        Ok(())
    }

    pub fn execute(&mut self) -> Result<()> {
        debug!("Executing IMAP filter process");

        let messages = self.fetch_messages()?;
        self.apply_filters(&messages)?;

        self.client.logout()?;
        debug!("IMAP session logged out successfully.");

        Ok(())
    }
}

#[derive(Debug)]
struct Message {
    uid: u32,
    to: Vec<String>,
    cc: Vec<String>,
    fr: String,
    sub: String,
}

impl Message {
    fn new(raw_uid: u32, raw_data: Vec<u8>) -> Self {
        debug!("Parsing message UID: {}", raw_uid);

        let raw_string = String::from_utf8_lossy(&raw_data);
        let headers: HashMap<String, String> = raw_string
            .lines()
            .filter_map(|line| line.split_once(": "))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();

        let message = Self {
            uid: raw_uid,
            to: listify(headers.get("To").map(|s| s.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()).as_ref()),
            cc: listify(headers.get("Cc").map(|s| s.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()).as_ref()),
            fr: headers.get("From").cloned().unwrap_or_default(),
            sub: headers.get("Subject").cloned().unwrap_or_default(),
        };

        debug!("Parsed message: {:?}", message);
        message
    }

    fn compare(&self, filter: &MessageFilter) -> bool {
        debug!("Comparing message UID {} against filter: {}", self.uid, filter.name);

        if let Some(to) = &filter.to {
            if !compare(&self.to.join(","), to) {
                debug!("Message UID {} did not match 'to' filter", self.uid);
                return false;
            }
        }
        if let Some(cc) = &filter.cc {
            if !compare(&self.cc.join(","), cc) {
                debug!("Message UID {} did not match 'cc' filter", self.uid);
                return false;
            }
        }
        if let Some(fr) = &filter.fr {
            if !compare(&self.fr, &[fr.clone()]) {
                debug!("Message UID {} did not match 'from' filter", self.uid);
                return false;
            }
        }

        debug!("Message UID {} matches filter: {}", self.uid, filter.name);
        true
    }
}
