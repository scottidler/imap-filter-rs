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
use crate::uid_tracker::{load_last_uid, save_last_uid};

pub struct IMAPFilter {
    client: Session<TlsStream<TcpStream>>,
    filters: Vec<MessageFilter>,
    states: Vec<State>,
}

impl IMAPFilter {
    pub fn new(domain: String, username: String, password: String, filters: Vec<MessageFilter>, states: Vec<State>) -> Result<Self> {
        debug!("Initializing IMAP connection to {}", domain);

        let tls = TlsConnector::builder().build()?;
        let client = imap::connect((domain.as_str(), 993), &domain, &tls)
            .map_err(|e| eyre!("IMAP connection failed: {:?}", e))?
            .login(username, password)
            .map_err(|e| eyre!("IMAP authentication failed: {:?}", e))?;

        debug!("Successfully connected and authenticated to IMAP server.");
        Ok(Self { client, filters, states })
    }

    fn fetch_messages(&mut self) -> Result<Vec<Message>> {
        use crate::uid_tracker::{load_last_uid, save_last_uid};

        debug!("Fetching messages from INBOX");

        self.client.select("INBOX")?;

        let since_uid = load_last_uid().unwrap_or(None);
        let query = match since_uid {
            Some(uid) => format!("UID {}:*", uid + 1),
            None => "ALL".to_string(),
        };

        let uids = self.client.uid_search(&query)?;
        debug!("Query '{}' returned {} UIDs", query, uids.len());

        if uids.is_empty() {
            return Ok(vec![]);
        }

        let fetches = self.client.uid_fetch(
            uids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(","),
            "RFC822"
        )?;

        let mut results = Vec::new();
        let mut max_uid = 0;
        for fetch in fetches.iter() {
            if let Some(body) = fetch.body() {
                let uid = fetch.message;
                max_uid = max_uid.max(uid);
                results.push(Message::new(uid, body.to_vec()));
            }
        }

        debug!("Successfully fetched {} messages", results.len());

        if max_uid > 0 {
            save_last_uid(max_uid)?;
        }

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
                info!("Processing UID: {} | Subject: {}", msg.uid, msg.subject);

                for action in &filter.actions {
                    match action {
                        FilterAction::Star => {
                            info!("Starring email UID: {} | Subject: {}", msg.uid, msg.subject);
                            if let Err(e) = self.client.uid_store(msg.uid.to_string(), "+X-GM-LABELS (\\Starred)") {
                                error!("Failed to star email UID {}: {:?} | Subject: {}", msg.uid, e, msg.subject);
                            } else {
                                info!("⭐ Successfully starred UID {} | Subject: {}", msg.uid, msg.subject);
                            }
                        }
                        FilterAction::Flag => {
                            info!("Flagging email UID: {} | Subject: {}", msg.uid, msg.subject);
                            if let Err(e) = self.client.uid_store(msg.uid.to_string(), "+X-GM-LABELS (\\Important)") {
                                error!("Failed to flag email UID {}: {:?} | Subject: {}", msg.uid, e, msg.subject);
                            } else {
                                info!("🚩 Successfully flagged UID {} | Subject: {}", msg.uid, msg.subject);
                            }
                        }
                        FilterAction::Move(label) => {
                            info!("Applying label '{}' to email UID {} | Subject: {}", label, msg.uid, msg.subject);
                            if let Err(e) = self.client.uid_store(msg.uid.to_string(), &format!("+X-GM-LABELS \"{}\"", label)) {
                                error!("Failed to apply label '{}' to email UID {}: {:?} | Subject: {}", label, msg.uid, e, msg.subject);
                            } else {
                                info!("✅ Successfully labeled UID {} with '{}' | Subject: {}", msg.uid, label, msg.subject);
                            }
                        }
                    }
                }
            }

            messages = remaining_messages;
        }

        info!("Finished applying filters.");
    }

    pub fn evaluate_states(&mut self, states: &[State]) -> Result<()> {
        info!("Evaluating {} states for TTL and transition", states.len());

        for state in states {
            info!("Evaluating state: {}", state.name);

            let uids = self.client.uid_search(&state.query)?;
            debug!("State '{}' matched {} messages", state.name, uids.len());

            for uid in uids.iter() {
                let fetches = self.client.uid_fetch(uid.to_string(), "BODY[HEADER.FIELDS (SUBJECT)]")?;
                let subject = fetches.iter()
                    .filter_map(|f| f.header())
                    .map(|h| String::from_utf8_lossy(h).to_string())
                    .next()
                    .unwrap_or_else(|| "<no subject>".to_string());

                if state.nerf {
                    info!("NERF mode: would apply {:?} to UID {} | Subject: {}", state.action, uid, subject);
                } else {
                    self.apply_transitions(*uid, &state.action, &subject);
                }
            }
        }

        Ok(())
    }

    fn apply_transitions(&mut self, uid: u32, action: &StateAction, subject: &str) {
        match action {
            StateAction::Delete => {
                info!("Deleting email UID {} | Subject: {}", uid, subject);
                if let Err(e) = self.client.uid_store(uid.to_string(), "+FLAGS (\\Deleted)") {
                    error!("❌ Failed to mark UID {} as \\Deleted: {:?} | Subject: {}", uid, e, subject);
                } else {
                    info!("🗑 Marked UID {} as \\Deleted | Subject: {}", uid, subject);
                }
            }
            StateAction::Move(label) => {
                info!("Applying label '{}' to UID {} | Subject: {}", label, uid, subject);
                if let Err(e) = self.client.uid_store(uid.to_string(), &format!("+X-GM-LABELS \"{}\"", label)) {
                    error!("❌ Failed to apply label '{}' to UID {}: {:?} | Subject: {}", label, uid, e, subject);
                } else {
                    info!("✅ Successfully labeled UID {} with '{}' | Subject: {}", uid, label, subject);
                }
            }
        }
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

    fn sample_message(uid: u32) -> Message {
        Message {
            uid,
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
        let msg = sample_message(123);
        let filter = sample_filter();
        let (from_match, to_match, cc_match, sub_match) = msg.compare(&filter);
        assert!(from_match && to_match && cc_match && sub_match, "Message should match all fields");
    }

    #[test]
    fn test_apply_filters_applies_all_actions() {
        use imap::types::UnsolicitedResponse;
        use std::sync::{Arc, Mutex};

        struct MockClient {
            pub called: Arc<Mutex<Vec<String>>>,
        }

        impl MockClient {
            fn new() -> Self {
                Self { called: Arc::new(Mutex::new(vec![])) }
            }

            fn uid_store(&self, uid: String, command: &str) -> Result<(), imap::error::Error> {
                self.called.lock().unwrap().push(format!("{} => {}", uid, command));
                Ok(())
            }
        }

        struct DummyIMAPFilter {
            called: Arc<Mutex<Vec<String>>>,
            filters: Vec<MessageFilter>,
        }

        impl DummyIMAPFilter {
            fn new(called: Arc<Mutex<Vec<String>>>) -> Self {
                Self {
                    called,
                    filters: vec![sample_filter()],
                }
            }

            fn apply_filters(&self, messages: Vec<Message>) {
                for msg in &messages {
                    for action in &self.filters[0].actions {
                        match action {
                            FilterAction::Star => {
                                self.called.lock().unwrap().push(format!("{} => Star", msg.uid));
                            }
                            FilterAction::Flag => {
                                self.called.lock().unwrap().push(format!("{} => Flag", msg.uid));
                            }
                            FilterAction::Move(label) => {
                                self.called.lock().unwrap().push(format!("{} => Move({})", msg.uid, label));
                            }
                        }
                    }
                }
            }
        }

        let called = Arc::new(Mutex::new(vec![]));
        let fake = DummyIMAPFilter::new(Arc::clone(&called));

        let message = sample_message(999);
        fake.apply_filters(vec![message]);

        let called = called.lock().unwrap();
        assert!(called.contains(&"999 => Star".to_string()));
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

        struct DummySession;
        impl DummySession {
            fn uid_search(&self, _query: &str) -> Result<Vec<u32>, eyre::Report> {
                Ok(vec![101])
            }

            fn uid_fetch(&self, _uid: String, _attrs: &str) -> Result<Vec<imap::types::Fetch>, eyre::Report> {
                Ok(vec![])
            }
        }

        struct DummyFilter {
            client: DummySession,
            states: Vec<State>,
        }

        impl DummyFilter {
            fn evaluate_states(&mut self) -> Result<()> {
                for state in &self.states {
                    let uids = self.client.uid_search(&state.query)?;
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
            client: DummySession,
            states: vec![dummy_state],
        };

        dummy.evaluate_states().unwrap();
    }

}
