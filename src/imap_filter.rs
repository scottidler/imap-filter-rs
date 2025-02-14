use eyre::{Result, eyre};
use imap::Session;
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
        let tls = TlsConnector::builder().build()?;
        let client = imap::connect((domain.as_str(), 993), &domain, &tls)
            .map_err(|e| eyre!("IMAP connection failed: {:?}", e))?
            .login(username, password)
            .map_err(|e| eyre!("IMAP authentication failed: {:?}", e))?;

        Ok(Self { client, filters })
    }

    fn fetch_messages(&mut self) -> Result<Vec<Message>> {
        self.client.select("INBOX")?;
        let messages = self.client.search("ALL")?;
        let fetches = self.client.fetch(messages.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(","), "RFC822")?;

        let mut results = Vec::new();
        for fetch in fetches.iter() {
            if let Some(body) = fetch.body() {
                results.push(Message::new(fetch.message, body.to_vec()));
            }
        }

        Ok(results)
    }

    fn apply_filters(&mut self, messages: &[Message]) -> Result<()> {
        for filter in &self.filters {
            let filtered: Vec<&Message> = messages.iter().filter(|msg| msg.compare(filter)).collect();
            if !filtered.is_empty() {
                let uids: Vec<String> = filtered.iter().map(|m| m.uid.to_string()).collect();

                if let Some(folder) = &filter.move_to {
                    println!("Moving messages {:?} to {}", uids, folder);
                    self.client.uid_copy(&uids.join(","), folder)?;
                }
                if filter.star.unwrap_or(false) {
                    println!("Starring messages {:?}", uids);
                    self.client.uid_store(&uids.join(","), "+FLAGS (\\Flagged)")?;
                }
            }
        }
        Ok(())
    }

    pub fn execute(&mut self) -> Result<()> {
        let messages = self.fetch_messages()?;
        self.apply_filters(&messages)?;
        self.client.logout()?;
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
        let raw_string = String::from_utf8_lossy(&raw_data);
        let headers: HashMap<String, String> = raw_string
            .lines()
            .filter_map(|line| line.split_once(": "))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();

        Self {
            uid: raw_uid,
            to: listify(headers.get("To").map(|s| s.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()).as_ref()),
            cc: listify(headers.get("Cc").map(|s| s.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()).as_ref()),
            fr: headers.get("From").cloned().unwrap_or_default(),
            sub: headers.get("Subject").cloned().unwrap_or_default(),
        }
    }

    fn compare(&self, filter: &MessageFilter) -> bool {
        if let Some(to) = &filter.to {
            if !compare(&self.to.join(","), to) {
                return false;
            }
        }
        if let Some(cc) = &filter.cc {
            if !compare(&self.cc.join(","), cc) {
                return false;
            }
        }
        if let Some(fr) = &filter.fr {
            if !compare(&self.fr, &[fr.clone()]) {
                return false;
            }
        }
        true
    }
}
