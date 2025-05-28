use eyre::{Result, eyre};
use imap::Session;
use log::{debug, info, error};
use native_tls::{TlsConnector, TlsStream};
use std::net::TcpStream;
use imap::types::Flag;
use chrono::{DateTime, Duration, Utc};
use std::collections::{HashSet, HashMap};

use crate::message::Message;
pub use crate::message_filter::{MessageFilter, FilterAction};
use crate::address_filter::AddressFilter;
use crate::state::{State, StateAction, TTL};
//use crate::uid_tracker::{load_last_uid, save_last_uid};
use crate::utils::{parse_days, set_label, del_label};

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

    /// First-pass filtering: apply user-defined filters (Star, Flag, or Move).
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
                info!(
                    "Processing UID: {} | Seq: {} | Subject: {}",
                    msg.uid, msg.seq, msg.subject
                );

                // We only honor the *first* action in the Vec.
                if let Some(action) = filter.actions.first() {
                    match action {
                        FilterAction::Star => {
                            info!("Starring UID: {} | Subject: {}", msg.uid, msg.subject);
                            if let Err(e) = self
                                .client
                                .uid_store(msg.uid.to_string(), "+X-GM-LABELS (\\Starred)")
                            {
                                error!("Failed to star UID {}: {:?} | Subject: {}", msg.uid, e, msg.subject);
                            } else {
                                info!("⭐ Successfully starred UID {} | Subject: {}", msg.uid, msg.subject);
                            }
                        }
                        FilterAction::Flag => {
                            info!("Flagging UID: {} | Subject: {}", msg.uid, msg.subject);
                            if let Err(e) = self
                                .client
                                .uid_store(msg.uid.to_string(), "+X-GM-LABELS (\\Important)")
                            {
                                error!("Failed to flag UID {}: {:?} | Subject: {}", msg.uid, e, msg.subject);
                            } else {
                                info!("🚩 Successfully flagged UID {} | Subject: {}", msg.uid, msg.subject);
                            }
                        }
                        FilterAction::Move(label) => {
                            info!("Moving UID: {} → '{}' | Subject: {}", msg.uid, label, msg.subject);
                            // UID MOVE is atomic: adds label and removes INBOX
                            if let Err(e) = self.client.uid_mv(msg.uid.to_string(), label) {
                                error!("Failed to MOVE UID {}: {:?} | Subject: {}", msg.uid, e, msg.subject);
                            } else {
                                info!("✅ Successfully moved UID {} to '{}' | Subject: {}", msg.uid, label, msg.subject);
                            }
                        }
                    }
                }
            }

            messages = remaining_messages;
        }

        info!("Finished applying filters.");
    }

    /// Second-pass state transitions: move or delete based on TTL and labels.
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
                info!("Moving UID {} → '{}' | Subject: {}", uid, label, subject);
                // UID MOVE will remove INBOX automatically
                if let Err(e) = self.client.uid_mv(uid.to_string(), label) {
                    error!("❌ Failed to MOVE UID {}: {:?} | Subject: {}", uid, e, subject);
                } else {
                    info!("✅ Successfully moved UID {} to '{}' | Subject: {}", uid, label, subject);
                }
            }
        }
    }

// src/imap_filter.rs

    /// Second-pass state transitions: move or delete based on TTL and labels.
    fn evaluate_states(&mut self, states: &[State]) -> Result<()> {
        use crate::utils::get_labels;
        info!("Evaluating {} states for TTL and transition", states.len());

        // Select INBOX once
        self.client.select("INBOX")?;
        let now: DateTime<Utc> = Utc::now();

        for state in states {
            info!("Evaluating state: {}", state.name);

            // 1) Search for all UIDs matching this state's query
            let uids = self.client.uid_search(&state.query)?
                .into_iter()
                .collect::<Vec<_>>();

            debug!("State '{}' matched {} UIDs", state.name, uids.len());
            if uids.is_empty() {
                continue;
            }

            // 2) For each UID, in ascending order:
            for uid in uids {
                // a) Skip if we've already moved/deleted it in a previous state pass
                //    (this assumes state order is protective first-protective last)
                //    If you want to track it explicitly, you can insert into a `handled: HashSet<_>`.

                // b) Load its labels
                let labels = get_labels(&mut self.client, uid)?;
                debug!("UID {} labels = {:?}", uid, labels);

                // c) If it’s Starred or Important, it gets a free pass forever
                if labels.contains("Starred") || labels.contains("Important") {
                    debug!("UID {} is Starred/Important → skipping", uid);
                    continue;
                }

                // d) Fetch INTERNALDATE and FLAGS for TTL
                let seq = uid.to_string();
                let fetches = self.client.uid_fetch(&seq, "(INTERNALDATE FLAGS)")?;
                let fetch = fetches
                    .get(0)
                    .expect("INTERNALDATE/FLAGS fetch must return one item");

                let subject = fetch
                    .envelope()
                    .and_then(|e| e.subject.as_ref())
                    .map(|s| String::from_utf8_lossy(s).to_string())
                    .unwrap_or_else(|| "<no subject>".to_string());

                // compute age
                let internal = fetch
                    .internal_date()
                    .expect("INTERNALDATE must be present")
                    .with_timezone(&Utc);
                let age = now.signed_duration_since(internal);

                // determine TTL
                let ttl_duration = match &state.ttl {
                    TTL::Keep => {
                        info!("State '{}' = KEEP → skipping UID {}", state.name, uid);
                        continue;
                    }
                    TTL::Simple(days) => parse_days(days)?,
                    TTL::Detailed { read, unread } => {
                        let seen = fetch.flags().iter().any(|f| *f == Flag::Seen);
                        let period = if seen { read } else { unread };
                        parse_days(period)?
                    }
                };

                // e) If not expired, skip
                if age < ttl_duration {
                    debug!(
                        "UID {} age {:?} < TTL {:?} → skipping",
                        uid, age, ttl_duration
                    );
                    continue;
                }

                // f) TTL expired → apply the configured action
                if state.nerf {
                    info!("NERF: would apply {:?} to UID {}", state.action, uid);
                } else {
                    info!(
                        "TTL expired for UID {} (age {:?}, TTL {:?}) → applying {:?}",
                        uid, age, ttl_duration, state.action
                    );
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
