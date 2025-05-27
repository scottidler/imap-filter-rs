use serde::{Deserialize, Deserializer};
use serde::de::{self, Visitor, SeqAccess, MapAccess};
use std::fmt;
use std::str::FromStr;

use crate::address_filter::AddressFilter;

#[derive(Debug, PartialEq, Deserialize)]
pub enum FilterAction {
    Star,
    Flag,
    Move(String),
}

impl FromStr for FilterAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Star" => Ok(FilterAction::Star),
            "Flag" => Ok(FilterAction::Flag),
            _ => {
                if let Some(rest) = s.strip_prefix("Move:") {
                    Ok(FilterAction::Move(rest.to_string()))
                } else {
                    Err(format!("Invalid action: {}", s))
                }
            }
        }
    }
}

fn deserialize_actions<'de, D>(deserializer: D) -> Result<Vec<FilterAction>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ActionsVisitor;

    impl<'de> Visitor<'de> for ActionsVisitor {
        type Value = Vec<FilterAction>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a single action or a list of actions")
        }

        fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let raw: Vec<String> = Deserialize::deserialize(de::value::SeqAccessDeserializer::new(seq))?;
            raw.into_iter()
                .map(|s| FilterAction::from_str(&s)
                    .map_err(|e| de::Error::custom(format!("invalid action '{}': {}", s, e))))
                .collect()
        }


        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let single = Deserialize::deserialize(de::value::MapAccessDeserializer::new(map))?;
            Ok(vec![single])
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            let single = FilterAction::from_str(value)
                .map_err(|e| E::custom(format!("invalid action: {}", e)))?;
            Ok(vec![single])
        }
    }

    deserializer.deserialize_any(ActionsVisitor)
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
            E: de::Error,
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

    #[serde(default)]
    pub subject: Vec<String>,

    #[serde(default, deserialize_with = "deserialize_actions")]
    #[serde(alias = "action", alias = "actions")]
    pub actions: Vec<FilterAction>,
}

impl MessageFilter {
    pub fn print_details(&self) {
        println!("\n{}", self.name);
        if let Some(to) = &self.to {
            println!("    to: {:?}", to.patterns);
        }
        if let Some(cc) = &self.cc {
            println!("    cc: {:?}", cc.patterns);
        }
        if let Some(from) = &self.from {
            println!("    from: {:?}", from.patterns);
        }
        println!("    actions: {:?}", self.actions);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml;

    #[test]
    fn test_deserialize_single_action_string() {
        let yaml = r#"
            to: "alice@example.com"
            action: Star
        "#;
        let parsed: MessageFilter = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.actions, vec![FilterAction::Star]);
    }

    #[test]
    fn test_deserialize_multiple_actions_list() {
        let yaml = r#"
            from: ["*@tatari.tv"]
            actions: ["Star", "Flag", "Move:Archive"]
        "#;
        let parsed: MessageFilter = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            parsed.actions,
            vec![
                FilterAction::Star,
                FilterAction::Flag,
                FilterAction::Move("Archive".to_string())
            ]
        );
    }

    #[test]
    fn test_deserialize_action_map_form() {
        let yaml = r#"
            to: ["bob@example.com"]
            action: { Move: "Important" }
        "#;
        let parsed: MessageFilter = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.actions, vec![FilterAction::Move("Important".to_string())]);
    }

    #[test]
    fn test_deserialize_address_filter_variants() {
        let yaml = r#"
            to: "bob@example.com"
            cc: ["team@example.com", "admin@example.com"]
            from: []
            actions: [Flag]
        "#;
        let parsed: MessageFilter = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(parsed.to.unwrap().patterns, vec!["bob@example.com"]);
        assert_eq!(
            parsed.cc.unwrap().patterns,
            vec!["team@example.com", "admin@example.com"]
        );
        assert_eq!(parsed.from.unwrap().patterns, Vec::<String>::new());
    }

    #[test]
    fn test_from_str_for_filter_action() {
        use std::str::FromStr;
        assert_eq!(FilterAction::from_str("Star").unwrap(), FilterAction::Star);
        assert_eq!(FilterAction::from_str("Move:Trash").unwrap(), FilterAction::Move("Trash".to_string()));
        assert!(FilterAction::from_str("Unknown").is_err());
    }

    #[test]
    fn test_default_actions_empty() {
        let yaml = r#"
            to: "user@example.com"
        "#;
        let parsed: MessageFilter = serde_yaml::from_str(yaml).unwrap();
        assert!(parsed.actions.is_empty());
    }

    #[test]
    fn test_print_details_runs_without_panic() {
        let filter = MessageFilter {
            name: "debug-me".to_string(),
            to: Some(AddressFilter { patterns: vec!["alice@foo.com".to_string()] }),
            cc: None,
            from: Some(AddressFilter { patterns: vec!["*@tatari.tv".to_string()] }),
            subject: vec!["*urgent*".to_string()],
            actions: vec![FilterAction::Flag],
        };

        filter.print_details();
    }
}
