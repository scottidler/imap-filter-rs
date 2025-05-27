use std::collections::HashMap;
use mailparse::{addrparse, MailAddr};
use serde::{Deserialize, Serialize};
use globset::Glob;

use crate::message_filter::MessageFilter;
use crate::address_filter::AddressFilter;

fn parse_email_header(header: &str) -> Vec<(String, String)> {
    match addrparse(header) {
        Ok(parsed) => parsed
            .iter()
            .flat_map(|addr| match addr {
                MailAddr::Single(info) => vec![
                    (info.display_name.clone().unwrap_or_default(), info.addr.clone())
                ],
                MailAddr::Group(group) => group.addrs.iter()
                    .map(|info| (
                        info.display_name.clone().unwrap_or_default(),
                        info.addr.clone()
                    ))
                    .collect(),
            })
            .collect(),
        Err(_) => vec![],
    }
}

#[derive(Debug)]
pub struct Message {
    pub uid: u32,
    pub to: Vec<(String, String)>,
    pub cc: Vec<(String, String)>,
    pub from: Vec<(String, String)>,
    pub subject: String,
}

impl Message {
    pub fn new(raw_uid: u32, raw_data: Vec<u8>) -> Self {
        let raw_string = String::from_utf8_lossy(&raw_data);
        let headers: HashMap<String, String> = raw_string
            .lines()
            .filter_map(|line| line.split_once(": "))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();

        let to_list = headers.get("To").map(|s| parse_email_header(s)).unwrap_or_default();
        let cc_list = headers.get("Cc").map(|s| parse_email_header(s)).unwrap_or_default();
        let from_list = headers.get("From").map(|s| parse_email_header(s)).unwrap_or_default();

        Self {
            uid: raw_uid,
            to: to_list,
            cc: cc_list,
            from: from_list,
            subject: headers.get("Subject").cloned().unwrap_or_default(),
        }
    }

    fn matches_field(field: &Option<AddressFilter>, message: &Message, extractor: fn(&Message) -> &Vec<(String, String)>) -> bool {
        match field {
            Some(filter) if filter.patterns.is_empty() => extractor(message).is_empty(),
            Some(filter) => filter.matches(&extractor(message).iter().map(|(_, email)| email.clone()).collect::<Vec<_>>()),
            None => true,
        }
    }

    pub fn compare(&self, filter: &MessageFilter) -> (bool, bool, bool, bool) {
        let from_match = Self::matches_field(&filter.from, self, |m| &m.from);
        let to_match = Self::matches_field(&filter.to, self, |m| &m.to);
        let cc_match = Self::matches_field(&filter.cc, self, |m| &m.cc);

        let subject_match = if filter.subject.is_empty() {
            true
        } else {
            let subject = &self.subject;
            filter.subject.iter().any(|pattern| {
                Glob::new(pattern)
                    .expect("Invalid glob pattern")
                    .compile_matcher()
                    .is_match(subject)
            })
        };

        (from_match, to_match, cc_match, subject_match)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_filter::{MessageFilter, FilterAction};
    use crate::address_filter::AddressFilter;

    #[test]
    fn test_only_me_star_filter_behavior() {
        let filter = MessageFilter {
            name: "only-me-star".to_string(),
            to: Some(AddressFilter { patterns: vec!["scott.idler@tatari.tv".to_string()] }),
            from: Some(AddressFilter { patterns: vec!["*@tatari.tv".to_string()] }),
            cc: Some(AddressFilter { patterns: vec![] }), // Must match emails with no CCs
            subject: vec!["only to me".to_string()],
            actions: vec![FilterAction::Star, FilterAction::Flag],
        };

        let matching_email = Message {
            uid: 1,
            to: vec![("Scott Idler".to_string(), "scott.idler@tatari.tv".to_string())],
            from: vec![("Scott Idler".to_string(), "scott.idler@tatari.tv".to_string())],
            cc: vec![],
            subject: "only to me".to_string(),
        };

        let non_matching_email = Message {
            uid: 2,
            to: vec![("Scott Idler".to_string(), "scott.idler@tatari.tv".to_string())],
            from: vec![("Scott Idler".to_string(), "scott.idler@tatari.tv".to_string())],
            cc: vec![("Someone Else".to_string(), "someone@tatari.tv".to_string())],
            subject: "cc included".to_string(),
        };

        assert_eq!(matching_email.compare(&filter), (true, true, true, true), "Matching email should be accepted");
        assert_eq!(non_matching_email.compare(&filter), (true, true, false, false), "Non-matching email should be rejected due to CC and subject");
    }

    #[test]
    fn test_header_parsing_extracts_subject_and_addresses() {
        let raw_data = b"To: Scott <scott@tatari.tv>\r\nFrom: Admin <admin@tatari.tv>\r\nSubject: Test Subject\r\n\r\nBody text.".to_vec();
        let message = Message::new(10, raw_data);

        assert_eq!(message.uid, 10);
        assert_eq!(message.subject, "Test Subject");
        assert_eq!(message.to.len(), 1);
        assert_eq!(message.to[0].1, "scott@tatari.tv");
        assert_eq!(message.from[0].1, "admin@tatari.tv");
    }

    #[test]
    fn test_header_parsing_gracefully_handles_missing_headers() {
        let raw_data = b"Subject: Just Subject\r\n\r\nBody".to_vec();
        let message = Message::new(99, raw_data);

        assert_eq!(message.uid, 99);
        assert_eq!(message.subject, "Just Subject");
        assert!(message.to.is_empty());
        assert!(message.cc.is_empty());
        assert!(message.from.is_empty());
    }

    #[test]
    fn test_matches_field_with_no_filter_matches_anything() {
        let message = Message {
            uid: 7,
            to: vec![("Name".to_string(), "anyone@example.com".to_string())],
            from: vec![],
            cc: vec![],
            subject: "".to_string(),
        };

        let result = Message::matches_field(&None, &message, |m| &m.to);
        assert!(result, "None filter should match any value");
    }

    #[test]
    fn test_matches_field_with_empty_filter_only_matches_empty_vec() {
        let filter = Some(AddressFilter { patterns: vec![] });

        let msg_nonempty = Message {
            uid: 8,
            to: vec![("Name".to_string(), "foo@example.com".to_string())],
            from: vec![],
            cc: vec![],
            subject: "".to_string(),
        };

        let msg_empty = Message {
            uid: 9,
            to: vec![],
            from: vec![],
            cc: vec![],
            subject: "".to_string(),
        };

        assert!(!Message::matches_field(&filter, &msg_nonempty, |m| &m.to));
        assert!(Message::matches_field(&filter, &msg_empty, |m| &m.to));
    }

    #[test]
    fn test_compare_matches_when_only_to_field_is_filtered() {
        let filter = MessageFilter {
            name: "to-only".to_string(),
            to: Some(AddressFilter { patterns: vec!["scott@tatari.tv".to_string()] }),
            from: None,
            cc: None,
            subject: vec![],
            actions: vec![],
        };

        let msg = Message {
            uid: 12,
            to: vec![("Scott".to_string(), "scott@tatari.tv".to_string())],
            from: vec![("X".to_string(), "x@somewhere.com".to_string())],
            cc: vec![],
            subject: "Ping".to_string(),
        };

        assert_eq!(msg.compare(&filter), (true, true, true, true));
    }
}
