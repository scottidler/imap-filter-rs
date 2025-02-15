use addr::psl::parse_email_address;
use eyre::{Result, eyre};
use imap::Session;
use log::{debug, warn, error, info};
use native_tls::{TlsConnector, TlsStream};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use serde::de::{SeqAccess, Visitor};
use std::collections::HashMap;
use std::fmt;
use std::net::TcpStream;
use std::str::FromStr;

/// Represents a person with a name and an email address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    pub email: String,
}

impl Person {
    /// Parses a raw email string into a `Person` struct.
    pub fn from_email_string(input: &str) -> Self {
        let re = Regex::new(r#"^"?([^<"]+)"?\s*<([^>]+)>$"#).unwrap();

        if let Some(caps) = re.captures(input) {
            Self {
                name: caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default(),
                email: caps.get(2).map(|m| m.as_str().to_string()).unwrap_or_default(),
            }
        } else {
            Self {
                name: "".to_string(),
                email: input.trim().to_string(),
            }
        }
    }
}

/// Custom deserialization for lists of `Person`
fn deserialize_person_list<'de, D>(deserializer: D) -> Result<Option<Vec<Person>>, D::Error>
where
    D: Deserializer<'de>,
{
    struct PersonListVisitor;

    impl<'de> Visitor<'de> for PersonListVisitor {
        type Value = Option<Vec<Person>>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a list of email strings")
        }

        fn visit_seq<M>(self, mut seq: M) -> Result<Self::Value, M::Error>
        where
            M: SeqAccess<'de>,
        {
            let mut persons = Vec::new();
            while let Some(email_str) = seq.next_element::<String>()? {
                persons.push(Person::from_email_string(&email_str));
            }
            Ok(Some(persons))
        }
    }

    deserializer.deserialize_seq(PersonListVisitor)
}

/// Custom deserialization for a single `Person`
fn deserialize_person<'de, D>(deserializer: D) -> Result<Option<Person>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    Ok(s.map(|input| Person::from_email_string(&input)))
}

#[derive(Debug, Deserialize)]
pub struct MessageFilter {
    #[serde(skip_deserializing)]
    pub name: String,

    #[serde(default, deserialize_with = "deserialize_person_list")]
    pub to: Option<Vec<Person>>,

    #[serde(default, deserialize_with = "deserialize_person_list")]
    pub cc: Option<Vec<Person>>,

    #[serde(default, deserialize_with = "deserialize_person")]
    pub fr: Option<Person>,

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

            let filtered: Vec<&Message> = messages.iter().filter(|&msg| msg.compare(filter)).collect();
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
    to: Vec<Person>,
    cc: Vec<Person>,
    from: Person,
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
            to: to_list.iter().map(|s| Person::from_email_string(s)).collect(),
            cc: cc_list.iter().map(|s| Person::from_email_string(s)).collect(),
            from: Person::from_email_string(headers.get("From").unwrap_or(&"".to_string())),
            subject: headers.get("Subject").cloned().unwrap_or_default(),
        }
    }

    /// Compare a message against a filter
    fn compare(&self, filter: &MessageFilter) -> bool {
        debug!("Comparing message UID {}: FROM '{}' TO {:?} CC {:?}",
            self.uid, self.from.email,
            self.to.iter().map(|p| &p.email).collect::<Vec<_>>(),
            self.cc.iter().map(|p| &p.email).collect::<Vec<_>>()
        );

        filter.to.as_ref().map_or(true, |to| self.to.iter().any(|p| to.contains(p)))
            && filter.cc.as_ref().map_or(true, |cc| self.cc.iter().any(|p| cc.contains(p)))
            && filter.fr.as_ref().map_or(true, |fr| &self.from == fr)
    }
}
