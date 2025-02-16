use eyre::{Result, eyre};
use imap::Session;
use log::{debug, info};
use native_tls::{TlsConnector, TlsStream};
use std::net::TcpStream;

use crate::message::Message;
pub use crate::message_filter::MessageFilter;
use crate::address_filter::AddressFilter;


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

    pub fn execute(&mut self) -> Result<()> {
        debug!("Executing IMAP filter process");

        let messages = self.fetch_messages()?;
        self.client.logout()?;
        debug!("IMAP session logged out successfully.");

        Ok(())
    }
}
