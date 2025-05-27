use serde::{Deserialize, Deserializer};
use serde::de::{SeqAccess, Visitor};
use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum TTL {
    Keep,
    Simple(String),
    Detailed { read: String, unread: String },
}

impl<'de> Deserialize<'de> for TTL {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawDetailed {
            read: String,
            unread: String,
        }

        struct TTLVisitor;

        impl<'de> Visitor<'de> for TTLVisitor {
            type Value = TTL;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a TTL string like '7d', the literal 'keep', or a map with {read, unread}")
            }

            fn visit_str<E>(self, value: &str) -> Result<TTL, E>
            where
                E: serde::de::Error,
            {
                if value.eq_ignore_ascii_case("keep") {
                    Ok(TTL::Keep)
                } else {
                    Ok(TTL::Simple(value.to_string()))
                }
            }

            fn visit_map<M>(self, map: M) -> Result<TTL, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                let RawDetailed { read, unread } =
                    Deserialize::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(TTL::Detailed { read, unread })
            }
        }

        deserializer.deserialize_any(TTLVisitor)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StateAction {
    Move(String),
    Delete,
}

impl<'de> Deserialize<'de> for StateAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ActionVisitor;

        impl<'de> Visitor<'de> for ActionVisitor {
            type Value = StateAction;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a folder name string, the literal 'delete', or a map like { Move: 'FolderName' }")
            }

            fn visit_str<E>(self, value: &str) -> Result<StateAction, E>
            where
                E: serde::de::Error,
            {
                if value.eq_ignore_ascii_case("delete") {
                    Ok(StateAction::Delete)
                } else {
                    Ok(StateAction::Move(value.to_string()))
                }
            }

            fn visit_map<M>(self, mut map: M) -> Result<StateAction, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                let key: Option<String> = map.next_key()?;
                if let Some(k) = key {
                    match k.as_str() {
                        "Move" => {
                            let value: String = map.next_value()?;
                            Ok(StateAction::Move(value))
                        }
                        _ => Err(serde::de::Error::unknown_field(&k, &["Move"])),
                    }
                } else {
                    Err(serde::de::Error::custom("Expected a key like 'Move'"))
                }
            }
        }

        deserializer.deserialize_any(ActionVisitor)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct State {
    #[serde(skip_deserializing)]
    pub name: String,

    pub query: String,
    pub ttl: TTL,

    #[serde(default = "default_action")]
    pub action: StateAction,

    #[serde(default)]
    pub nerf: bool,
}

fn default_action() -> StateAction {
    StateAction::Move("ToBeDeleted".to_string())
}

pub fn deserialize_state_maps<'de, D>(deserializer: D) -> Result<Vec<HashMap<String, State>>, D::Error>
where
    D: Deserializer<'de>,
{
    struct StateMapVisitor;

    impl<'de> Visitor<'de> for StateMapVisitor {
        type Value = Vec<HashMap<String, State>>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a list of maps with a single key for each state")
        }

        fn visit_seq<M>(self, mut seq: M) -> Result<Self::Value, M::Error>
        where
            M: SeqAccess<'de>,
        {
            let mut states = Vec::new();
            while let Some(map) = seq.next_element::<HashMap<String, State>>()? {
                let mut updated = HashMap::new();
                for (name, mut state) in map {
                    state.name = name.clone();
                    updated.insert(name, state);
                }
                states.push(updated);
            }
            Ok(states)
        }
    }

    deserializer.deserialize_seq(StateMapVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml;
    use std::collections::HashMap;

    #[test]
    fn test_deserialize_ttl_simple() {
        let yaml = r#"ttl: 7d"#;
        let value: HashMap<String, TTL> = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(value["ttl"], TTL::Simple("7d".to_string()));
    }

    #[test]
    fn test_deserialize_ttl_keep_case_insensitive() {
        for val in ["Keep", "keep", "KEEP"] {
            let yaml = format!("ttl: {val}");
            let value: HashMap<String, TTL> = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(value["ttl"], TTL::Keep);
        }
    }

    #[test]
    fn test_deserialize_ttl_detailed() {
        let yaml = r#"ttl: { read: "7d", unread: "21d" }"#;
        let value: HashMap<String, TTL> = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            value["ttl"],
            TTL::Detailed {
                read: "7d".to_string(),
                unread: "21d".to_string()
            }
        );
    }

    #[test]
    fn test_deserialize_state_action_delete_case_insensitive() {
        for val in ["Delete", "delete", "DELETE"] {
            let yaml = format!("action: {val}");
            let value: HashMap<String, StateAction> = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(value["action"], StateAction::Delete);
        }
    }

    #[test]
    fn test_deserialize_state_action_move() {
        let yaml = r#"action: ToBeDeleted"#;
        let value: HashMap<String, StateAction> = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            value["action"],
            StateAction::Move("ToBeDeleted".to_string())
        );
    }

    #[test]
    fn test_deserialize_state_action_map_form() {
        let yaml = r#"action: { Move: "Trash" }"#;
        let value: HashMap<String, StateAction> = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(value["action"], StateAction::Move("Trash".to_string()));
    }

    #[test]
    fn test_deserialize_state_map_into_states() {
        let yaml = r#"
- Starred:
    query: 'X-GM-LABELS "\Starred"'
    ttl: Keep
- Triaged:
    query: 'SEEN NOT X-GM-LABELS "\Starred"'
    ttl:
      read: 7d
      unread: 21d
    action: ToBeDeleted
- Junk:
    query: 'X-GM-LABELS "Junk"'
    ttl: 3d
    action: Delete
"#;

        let result: Vec<HashMap<String, State>> =
            serde_yaml::from_str::<Vec<HashMap<String, State>>>(yaml).unwrap();

        let mut flat: Vec<State> = result
            .into_iter()
            .flat_map(|map| map.into_iter().map(|(name, mut state)| {
                state.name = name;
                state
            }))
            .collect();

        flat.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(flat.len(), 3);
        assert_eq!(flat[0].name, "Junk");
        assert_eq!(flat[0].ttl, TTL::Simple("3d".to_string()));
        assert_eq!(flat[0].action, StateAction::Delete);

        assert_eq!(flat[1].name, "Starred");
        assert_eq!(flat[1].ttl, TTL::Keep);

        assert_eq!(flat[2].name, "Triaged");
        assert_eq!(
            flat[2].ttl,
            TTL::Detailed {
                read: "7d".to_string(),
                unread: "21d".to_string()
            }
        );
        assert_eq!(
            flat[2].action,
            StateAction::Move("ToBeDeleted".to_string())
        );
    }
}
