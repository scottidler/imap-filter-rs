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
        .map(|parsed| parsed.to_string()) // Convert Address<'_> to String
        .unwrap_or_else(|_| input.trim().to_string())
}

fn deserialize_email_list<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    struct EmailListVisitor;

    impl<'de> Visitor<'de> for EmailListVisitor {
        type Value = Option<Vec<String>>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a list of email strings")
        }

        fn visit_seq<M>(self, mut seq: M) -> Result<Self::Value, M::Error>
        where
            M: SeqAccess<'de>,
        {
            let mut emails = Vec::new();
            while let Some(email_str) = seq.next_element::<String>()? {
                emails.push(extract_email(&email_str));
            }
            Ok(Some(emails))
        }
    }

    deserializer.deserialize_seq(EmailListVisitor)
}

fn deserialize_email<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    Ok(s.map(|input| extract_email(&input)))
}

#[derive(Debug)]
struct Message {
    uid: u32,
    to: Vec<String>,
    cc: Vec<String>,
    from: String,
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

        Self {
            uid: raw_uid,
            to: to_list.iter().map(|s| extract_email(s)).collect(),
            cc: cc_list.iter().map(|s| extract_email(s)).collect(),
            from: extract_email(headers.get("From").unwrap_or(&"".to_string())),
            subject: headers.get("Subject").cloned().unwrap_or_default(),
        }
    }

    fn compare(&self, filter: &MessageFilter) -> bool {
        debug!(
            "Comparing message: UID {} SUBJECT '{}' FROM '{}' TO {:?} CC {:?}",
            self.uid,
            self.subject,
            self.from,
            self.to,
            self.cc
        );

        let mut matched = true;

        // Match TO field using globs
        if let Some(to_filter) = &filter.to {
            matched &= to_filter.iter().any(|pattern| {
                let glob = Glob::new(pattern).expect("Invalid glob pattern").compile_matcher();
                self.to.iter().any(|email| glob.is_match(email))
            });

            debug!("TO filter match result: {}", matched);
        }

        // Match CC field using globs
        if let Some(cc_filter) = &filter.cc {
            matched &= cc_filter.iter().any(|pattern| {
                let glob = Glob::new(pattern).expect("Invalid glob pattern").compile_matcher();
                self.cc.iter().any(|email| glob.is_match(email))
            });

            debug!("CC filter match result: {}", matched);
        }

        // Match FROM field using globs
        if let Some(fr_filter) = &filter.fr {
            let glob = Glob::new(fr_filter).expect("Invalid glob pattern").compile_matcher();
            matched &= glob.is_match(&self.from);

            debug!(
                "FROM filter match: '{}' against '{}' -> {}",
                self.from, fr_filter, matched
            );
        }

        debug!("Final filter match result: {}", matched);
        matched
    }
}

#[derive(Debug, Deserialize)]
pub struct MessageFilter {
    #[serde(skip_deserializing)]
    pub name: String,

    #[serde(default, deserialize_with = "deserialize_email_list")]
    pub to: Option<Vec<String>>,

    #[serde(default, deserialize_with = "deserialize_email_list")]
    pub cc: Option<Vec<String>>,

    #[serde(default, deserialize_with = "deserialize_email")]
    pub fr: Option<String>,

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
                    "Matched filter '{}': SUBJECT '{}' FROM '{}' TO {:?} CC {:?}",
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
