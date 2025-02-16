use std::collections::HashMap;
use mailparse::{addrparse, MailAddr};
use serde::{Deserialize, Serialize};

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

    pub fn compare(&self, filter: &MessageFilter) -> (bool, bool, bool) {
        let from_match = Self::matches_field(&filter.from, self, |m| &m.from);
        let to_match = Self::matches_field(&filter.to, self, |m| &m.to);
        let cc_match = Self::matches_field(&filter.cc, self, |m| &m.cc);

        (from_match, to_match, cc_match)
    }
}
