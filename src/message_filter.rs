use serde::{Deserialize};
use serde::de::{SeqAccess, Visitor, Deserializer};
use std::fmt;

use crate::address_filter::AddressFilter;

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

