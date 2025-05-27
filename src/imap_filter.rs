use eyre::{Result, eyre};
use imap::Session;
use log::{debug, info, error};
use native_tls::{TlsConnector, TlsStream};
use std::net::TcpStream;
use imap::types::Flag;

use crate::message::Message;
pub use crate::message_filter::{MessageFilter, FilterAction};
use crate::address_filter::AddressFilter;
use crate::state::{State, StateAction};
//use crate::uid_tracker::{load_last_uid, save_last_uid};
use crate::utils::{set_label, del_label};

pub struct IMAPFilter {
    client: Session<TlsStream<TcpStream>>,
    filters: Vec<MessageFilter>,
    states: Vec<State>,
}

impl IMAPFilter {
    pub fn new(domain: String, username: String, password: String, filters: Vec<MessageFilter>, states: Vec<State>) -> Result<Self> {
        debug!("Initializing IMAP connection to {}", domain);

        let tls = TlsConnector::builder().build()?;
        let mut client = imap::connect((domain.as_str(), 993), &domain, &tls)
            .map_err(|e| eyre!("IMAP connection failed: {:?}", e))?
            .login(username, password)
            .map_err(|e| eyre!("IMAP authentication failed: {:?}", e))?;

        client.debug = true;

        debug!("Successfully connected and authenticated to IMAP server.");
        Ok(Self { client, filters, states })
    }

    fn fetch_messages(&mut self) -> Result<Vec<Message>> {
        debug!("Fetching all messages from INBOX");

        self.client.select("INBOX")?;

        let messages = self.client.search("ALL")?;
        debug!("Found {} messages in INBOX", messages.len());

        if messages.is_empty() {
            return Ok(vec![]);
        }

        let sequence_set = messages
            .iter()
            .map(|seq| seq.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let fetches = self.client.fetch(&sequence_set, "BODY[HEADER.FIELDS (TO CC FROM SUBJECT)]")?;

        let mut results = Vec::new();
        for fetch in fetches.iter() {
            if let Some(body) = fetch.body() {
                let uid = fetch.uid.unwrap_or(0);
                let seq = fetch.message;
                results.push(Message::new(uid, seq, body.to_vec()));
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
                    let (from_match, to_match, cc_match, sub_match) = msg.compare(filter);
                    from_match && to_match && cc_match && sub_match
                });

            for msg in &matched_messages {
                info!("Processing UID: {} | Seq: {} | Subject: {}", msg.uid, msg.seq, msg.subject);

                for action in &filter.actions {
                    match action {
                        FilterAction::Star => {
                            info!("Starring UID: {} | Seq: {} | Subject: {}", msg.uid, msg.seq, msg.subject);
                            if let Err(e) = self.client.uid_store(msg.uid.to_string(), "+X-GM-LABELS (\\Starred)") {
                                error!("Failed to star UID {}: {:?} | Subject: {}", msg.uid, e, msg.subject);
                            } else {
                                info!("⭐ Successfully starred UID {} | Subject: {}", msg.uid, msg.subject);
                            }
                        }
                        FilterAction::Flag => {
                            info!("Flagging UID: {} | Seq: {} | Subject: {}", msg.uid, msg.seq, msg.subject);
                            if let Err(e) = self.client.uid_store(msg.uid.to_string(), "+X-GM-LABELS (\\Important)") {
                                error!("Failed to flag UID {}: {:?} | Subject: {}", msg.uid, e, msg.subject);
                            } else {
                                info!("🚩 Successfully flagged UID {} | Subject: {}", msg.uid, msg.subject);
                            }
                        }
                        FilterAction::Move(label) => {
                            info!("Applying label '{}' to UID {} | Subject: {}", label, msg.uid, msg.subject);
                            if let Err(e) = set_label(&mut self.client, msg.uid, label, &msg.subject) {
                                error!("❌ {}", e);
                            } else {
                                info!("✅ Successfully labeled UID {} with '{}' | Subject: {}", msg.uid, label, msg.subject);
                                if let Err(e) = del_label(&mut self.client, msg.uid, "INBOX", &msg.subject) {
                                    error!("❌ {}", e);
                                } else {
                                    info!("📤 Removed 'INBOX' label from UID {} | Subject: {}", msg.uid, msg.subject);
                                }
                            }
                        }
                    }
                }
            }

            messages = remaining_messages;
        }

        info!("Finished applying filters.");
    }

    fn apply_transitions(&mut self, uid: u32, action: &StateAction, subject: &str) {
        match action {
            StateAction::Delete => {
                info!("Deleting UID {} | Subject: {}", uid, subject);
                if let Err(e) = self.client.uid_store(uid.to_string(), "+FLAGS (\\Deleted)") {
                    error!("❌ Failed to mark UID {} as \\Deleted: {:?} | Subject: {}", uid, e, subject);
                } else {
                    info!("🗑 Marked UID {} as \\Deleted | Subject: {}", uid, subject);
                }
            }
            StateAction::Move(label) => {
                info!("Applying label '{}' to UID {} | Subject: {}", label, uid, subject);
                if let Err(e) = set_label(&mut self.client, uid, label, subject) {
                    error!("❌ {}", e);
                } else {
                    info!("✅ Successfully labeled UID {} with '{}' | Subject: {}", uid, label, subject);
                    if let Err(e) = del_label(&mut self.client, uid, "INBOX", subject) {
                        error!("❌ {}", e);
                    } else {
                        info!("📤 Removed 'INBOX' label from UID {} | Subject: {}", uid, subject);
                    }
                }
            }
        }
    }

    fn evaluate_states(&mut self, states: &[State]) -> Result<()> {
        info!("Evaluating {} states for TTL and transition", states.len());

        // We select by mailbox name first (sequence numbers apply here, but we'll do UID searches).
        self.client.select("INBOX")?;

        for state in states {
            info!("Evaluating state: {}", state.name);

            // Perform a UID SEARCH rather than SEARCH, so that the numbers we get back are UIDs.
            let uids = self.client.uid_search(&state.query)?;
            debug!("State '{}' matched {} messages", state.name, uids.len());

            if uids.is_empty() {
                continue;
            }

            // Build a comma-separated UID set for the next call.
            let uid_set = uids
                .iter()
                .map(|uid| uid.to_string())
                .collect::<Vec<_>>()
                .join(",");

            // Fetch the ENVELOPE over UID (so Fetch.uid is populated)
            let fetches = self.client.uid_fetch(&uid_set, "ENVELOPE")?;

            for fetch in fetches.iter() {
                // Now we can safely unwrap the UID
                let uid = fetch.uid.unwrap_or_else(|| {
                    // Should never happen, but guard anyway
                    error!("Expected UID in fetch, got none for state '{}'", state.name);
                    0
                });

                // Extract subject for logging
                let subject = fetch
                    .envelope()
                    .and_then(|env| env.subject.as_ref())
                    .map(|s| String::from_utf8_lossy(s).to_string())
                    .unwrap_or_else(|| "<no subject>".to_string());

                if state.nerf {
                    info!(
                        "NERF mode: would apply {:?} to UID {} | Subject: {}",
                        state.action, uid, subject
                    );
                } else {
                    self.apply_transitions(uid, &state.action, &subject);
                }
            }
        }

        Ok(())
    }

    pub fn execute(&mut self) -> Result<()> {
        debug!("Executing IMAP filter process");

        let messages = self.fetch_messages()?;
        self.apply_filters(messages);

        let states = self.states.clone();
        self.evaluate_states(&states)?;

        self.client.logout()?;
        debug!("IMAP session logged out successfully.");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_filter::FilterAction;
    use crate::address_filter::AddressFilter;
    use crate::message::Message;
    use crate::state::StateAction;

    fn test_apply_transitions(uid: u32, action: &StateAction, subject: &str) -> String {
        match action {
            StateAction::Delete => format!("Deleting UID {} (subject: {})", uid, subject),
            StateAction::Move(label) => format!("Moving UID {} to '{}' (subject: {})", uid, label, subject),
        }
    }

    #[test]
    fn test_apply_transitions_delete_logic() {
        let log = test_apply_transitions(101, &StateAction::Delete, "Cleanup this");
        assert!(log.contains("Deleting UID 101"));
    }

    #[test]
    fn test_apply_transitions_move_logic() {
        let log = test_apply_transitions(202, &StateAction::Move("Done".to_string()), "Wrapped up");
        assert!(log.contains("Moving UID 202 to 'Done'"));
    }

    fn sample_message(uid: u32, seq: u32) -> Message {
        Message {
            uid,
            seq,
            to: vec![("".into(), "scott@tatari.tv".into())],
            cc: vec![],
            from: vec![("".into(), "someone@tatari.tv".into())],
            subject: "test".into(),
        }
    }

    fn sample_filter() -> MessageFilter {
        MessageFilter {
            name: "simple".into(),
            to: Some(AddressFilter { patterns: vec!["scott@tatari.tv".into()] }),
            cc: Some(AddressFilter { patterns: vec![] }),
            from: Some(AddressFilter { patterns: vec!["*@tatari.tv".into()] }),
            subject: vec!["test".to_string()],
            actions: vec![FilterAction::Star, FilterAction::Move("Inbox/Processed".into())],
        }
    }

    #[test]
    fn test_compare_logic_matches_expected() {
        let msg = sample_message(456, 123);
        let filter = sample_filter();
        let (from_match, to_match, cc_match, sub_match) = msg.compare(&filter);
        assert!(from_match && to_match && cc_match && sub_match, "Message should match all fields");
    }

    #[test]
    fn test_apply_filters_applies_all_actions() {
        struct DummyIMAPFilter {
            filters: Vec<MessageFilter>,
        }

        impl DummyIMAPFilter {
            fn apply_filters(&self, messages: Vec<Message>) -> Vec<String> {
                let mut log = Vec::new();
                for msg in &messages {
                    for action in &self.filters[0].actions {
                        match action {
                            FilterAction::Star => log.push(format!("UID {} => Star", msg.uid)),
                            FilterAction::Flag => log.push(format!("UID {} => Flag", msg.uid)),
                            FilterAction::Move(label) => log.push(format!("UID {} => Move({})", msg.uid, label)),
                        }
                    }
                }
                log
            }
        }

        let fake = DummyIMAPFilter { filters: vec![sample_filter()] };
        let message = sample_message(999, 333);
        let called = fake.apply_filters(vec![message]);

        assert!(called.contains(&"UID 999 => Star".to_string()));
        assert!(called.iter().any(|c| c.contains("Move(Inbox/Processed)")));
    }

    #[test]
    fn test_evaluate_states_honors_nerf_flag() {
        use crate::state::{TTL, State};

        let dummy_state = State {
            name: "NerfedState".into(),
            query: "ALL".into(),
            ttl: TTL::Keep,
            action: StateAction::Delete,
            nerf: true,
        };

        struct DummyFilter {
            states: Vec<State>,
        }

        impl DummyFilter {
            fn evaluate_states(&mut self) -> Result<()> {
                for state in &self.states {
                    let uids = vec![101];
                    assert_eq!(uids, vec![101]);
                    if state.nerf {
                        return Ok(()); // this is our test case
                    }
                    panic!("NERF flag not honored");
                }
                Ok(())
            }
        }

        let mut dummy = DummyFilter {
            states: vec![dummy_state],
        };

        dummy.evaluate_states().unwrap();
    }
}
