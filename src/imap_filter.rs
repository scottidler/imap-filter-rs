use eyre::{Result, eyre};
use imap::Session;
use log::{debug, info, error};
use native_tls::{TlsConnector, TlsStream};
use std::net::TcpStream;
use imap::types::Flag; // Import Flag type for correct comparison

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

        let inbox_status = self.client.select("INBOX")?;
        debug!("Mailbox selection status: {:?}", inbox_status);

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

    fn apply_filters(&mut self, mut messages: Vec<Message>) {
        info!("Applying filters to {} messages", messages.len());

        for filter in &self.filters {
            filter.print_details();

            let (matched_messages, remaining_messages): (Vec<_>, Vec<_>) = messages
                .into_iter()
                .partition(|msg| {
                    let (from_match, to_match, cc_match) = msg.compare(filter);
                    from_match && to_match && cc_match
                });

            for msg in &matched_messages {
                info!("Processing UID: {} | Subject: {}", msg.uid, msg.subject);

                // Moving message by applying a Gmail label instead of using `uid_mv`
                if let Some(destination) = &filter.move_to {
                    info!("Applying label '{}' to email UID {}", destination, msg.uid);
                    if let Err(e) = self.client.uid_store(msg.uid.to_string(), &format!("+X-GM-LABELS \"{}\"", destination)) {
                        error!("Failed to apply label '{}' to email UID {}: {:?} | Subject: {}", destination, msg.uid, e, msg.subject);
                    } else {
                        info!("✅ Successfully labeled UID {} with '{}' | Subject: {}", msg.uid, destination, msg.subject);
                    }
                }

                // Starring the email using Gmail-friendly X-GM-LABELS
                if filter.star.unwrap_or(false) {
                    info!("Starring email UID: {} | Subject: {}", msg.uid, msg.subject);
                    if let Err(e) = self.client.uid_store(msg.uid.to_string(), "+X-GM-LABELS (\\Starred)") {
                        error!("Failed to star email UID {}: {:?} | Subject: {}", msg.uid, e, msg.subject);
                    } else {
                        info!("⭐ Successfully starred UID {} using Gmail's X-GM-LABELS | Subject: {}", msg.uid, msg.subject);

                        // Fetch and log the updated labels for verification
                        if let Ok(updated_labels) = self.client.uid_fetch(msg.uid.to_string(), "X-GM-LABELS") {
                            debug!("Updated LABELS for UID {}: {:?}", msg.uid, updated_labels);

                            if !updated_labels.iter().any(|fetch| fetch.flags().contains(&imap::types::Flag::Custom("\\Starred".to_string().into()))) {
                                error!("❌ FAILURE: Email UID {} does NOT have \\Starred after operation! | Subject: {}", msg.uid, msg.subject);
                            }
                        }
                    }
                }
            }

            messages = remaining_messages; // Continue filtering only the remaining messages
        }

        info!("Finished applying filters.");
    }

    pub fn execute(&mut self) -> Result<()> {
        debug!("Executing IMAP filter process");

        let messages = self.fetch_messages()?;
        self.apply_filters(messages);

        self.client.logout()?;
        debug!("IMAP session logged out successfully.");

        Ok(())
    }
}
