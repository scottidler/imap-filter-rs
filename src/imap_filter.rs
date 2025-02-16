use addr::parse_email_address;
use eyre::{Result, eyre};
use globset::Glob;
use imap::Session;
use log::{debug, warn, error, info};
use native_tls::{TlsConnector, TlsStream};
use serde::{Deserialize, Deserializer};
use serde::de::{SeqAccess, Visitor};
use std::collections::HashMap;
use std::fmt;
use std::net::TcpStream;
use std::str::FromStr;

fn extract_email(input: &str) -> String {
    parse_email_address(input)
        .map(|parsed| parsed.to_string())
        .unwrap_or_else(|_| input.trim().to_string())
}

#[derive(Debug)]
struct Message {
    uid: u32,
    to: Vec<String>,
    cc: Vec<String>,
    from: Vec<String>,
    subject: String,
}

impl Message {
    fn new(raw_uid: u32, raw_data: Vec<u8>) -> Self {
        let raw_string = String::from_utf8_lossy(&raw_data);
        let headers: HashMap<String, String> = raw_string
            .lines()
            .filter_map(|line| line.split_once(": "))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();

        let to_list = headers.get("To")
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>())
            .unwrap_or_default();
        let cc_list = headers.get("Cc")
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>())
            .unwrap_or_default();
        let from_list = headers.get("From")
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>())
            .unwrap_or_default();

        Self {
            uid: raw_uid,
            to: to_list.iter().map(|s| extract_email(s)).collect(),
            cc: cc_list.iter().map(|s| extract_email(s)).collect(),
            from: from_list.iter().map(|s| extract_email(s)).collect(),
            subject: headers.get("Subject").cloned().unwrap_or_default(),
        }
    }

    fn compare(&self, filter: &MessageFilter) -> bool {
        let to_match = filter.to.as_ref().map_or(false, |f| f.matches(&self.to));
        let cc_match = filter.cc.as_ref().map_or(false, |f| f.matches(&self.cc));
        let from_match = filter.from.as_ref().map_or(false, |f| f.matches(&self.from));
        let final_match = to_match || cc_match || from_match;

        debug!(
            "\n    subject: {}\n[{}] from: {:?}\n[{}] to: {:?}\n[{}] cc: {:?}\n[{}]",
            self.subject,
            if from_match { "T" } else { "F" },
            self.from,
            if to_match { "T" } else { "F" },
            self.to,
            if cc_match { "T" } else { "F" },
            self.cc,
            if final_match { "T" } else { "F" }
        );

        final_match
    }
}

#[derive(Debug, Deserialize)]
pub struct AddressFilter {
    pub patterns: Vec<String>,
}

impl AddressFilter {
    fn matches(&self, emails: &[String]) -> bool {
        self.patterns.iter().any(|pattern| {
            let glob = Glob::new(pattern).expect("Invalid glob pattern").compile_matcher();
            emails.iter().any(|email| glob.is_match(email))
        })
    }
}

fn deserialize_address_filter<'de, D>(deserializer: D) -> Result<Option<AddressFilter>, D::Error>
where
    D: Deserializer<'de>,
{
    struct AddressFilterVisitor;

    impl<'de> Visitor<'de> for AddressFilterVisitor {
        type Value = Option<AddressFilter>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a single email pattern or a list of email patterns")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(AddressFilter {
                patterns: vec![value.to_string()],
            }))
        }

        fn visit_seq<M>(self, mut seq: M) -> Result<Self::Value, M::Error>
        where
            M: SeqAccess<'de>,
        {
            let mut patterns = Vec::new();
            while let Some(email_str) = seq.next_element::<String>()? {
                patterns.push(email_str);
            }
            Ok(Some(AddressFilter { patterns }))
        }
    }

    deserializer.deserialize_any(AddressFilterVisitor)
}

#[derive(Debug, Deserialize)]
pub struct MessageFilter {
    #[serde(skip_deserializing)]
    pub name: String,

    #[serde(default, deserialize_with = "deserialize_address_filter")]
    pub to: Option<AddressFilter>,

    #[serde(default, deserialize_with = "deserialize_address_filter")]
    pub cc: Option<AddressFilter>,

    #[serde(default, deserialize_with = "deserialize_address_filter")]
    pub from: Option<AddressFilter>,

    pub move_to: Option<String>,
    pub star: Option<bool>,
}

#[derive(Debug)]
pub struct IMAPFilter {
    client: Session<TlsStream<TcpStream>>,
    filters: Vec<MessageFilter>,
}

impl IMAPFilter {
    pub fn new(domain: String, username: String, password: String, filters: Vec<MessageFilter>) -> Result<Self> {
        debug!("Initializing IMAP connection to {}", domain);

        let tls = TlsConnector::builder().build()?;
        let client = imap::connect((domain.as_str(), 993), &domain, &tls)
            .map_err(|e| eyre!("IMAP connection failed: {:?}", e))?
            .login(username, password)
            .map_err(|e| eyre!("IMAP authentication failed: {:?}", e))?;

        debug!("Successfully connected and authenticated to IMAP server.");
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

        let mut results = Vec::new();
        for fetch in fetches.iter() {
            if let Some(body) = fetch.body() {
                results.push(Message::new(fetch.message, body.to_vec()));
            }
        }

        debug!("Successfully fetched {} messages", results.len());
        Ok(results)
    }

    fn apply_filters(&mut self, messages: &[Message]) -> Result<()> {
        info!("Applying filters to {} messages", messages.len());

        for filter in &self.filters {
            info!("Applying filter: {}", filter.name);

            let filtered: Vec<&Message> = messages.iter().filter(|&msg| msg.compare(filter)).collect();

            if filtered.is_empty() {
                info!("No messages matched filter: {}", filter.name);
                continue;
            }

            for msg in &filtered {
                info!(
                    "Matched filter '{}': SUBJECT '{}' FROM {:?} TO {:?} CC {:?}",
                    filter.name, msg.subject, msg.from, msg.to, msg.cc
                );
            }

            if let Some(folder) = &filter.move_to {
                let uids: Vec<String> = filtered.iter().map(|m| m.uid.to_string()).collect();
                self.client.uid_copy(&uids.join(","), folder)?;
            }

            if filter.star.unwrap_or(false) {
                let uids: Vec<String> = filtered.iter().map(|m| m.uid.to_string()).collect();
                self.client.uid_store(&uids.join(","), "+FLAGS (\\Flagged)")?;
            }
        }

        info!("Finished applying filters.");
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
