use regex::Regex;
use std::collections::HashMap;

/// Remove newlines and tabs from a string
pub fn clean(s: &str) -> String {
    s.replace("\r\n\t", "").replace('"', "")
}

/// Check if a string matches any pattern in a list
pub fn compare(test: &str, items: &[String]) -> bool {
    items.iter().any(|item| {
        let escaped_item = if item.starts_with('*') {
            format!(r"\{}", item) // Escape leading `*`
        } else {
            item.clone()
        };

        Regex::new(&escaped_item).map(|re| re.is_match(test)).unwrap_or(false)
    })
}

/// Convert an object into a list
pub fn listify<T: Into<String> + Clone>(obj: Option<&Vec<T>>) -> Vec<String> {
    match obj {
        Some(vec) => vec.iter().cloned().map(|v| v.into()).collect(),
        None => vec![],
    }
}

/// Ensure default values in a struct
pub fn ensure_defaults<T: Clone>(obj: &Option<T>, default: T) -> T {
    obj.clone().unwrap_or(default)
}

/// Update a HashMap with new values
pub fn update<K: std::hash::Hash + Eq + Clone, V: Clone>(
    d: &mut HashMap<K, V>,
    values: HashMap<K, V>,
) {
    for (key, value) in values {
        d.insert(key, value);
    }
}

/// Extract the first key from a HashMap
pub fn head<K: Clone, V>(map: &HashMap<K, V>) -> Option<K> {
    map.keys().next().cloned()
}

/// Extract the first key-value pair from a HashMap
pub fn head_body<K: Clone, V: Clone>(map: &HashMap<K, V>) -> Option<(K, V)> {
    map.iter().next().map(|(k, v)| (k.clone(), v.clone()))
}

/// Remove matching elements from a list
pub fn subtract<T: PartialEq + Clone>(list1: &[T], list2: &[T]) -> Vec<T> {
    list1.iter().filter(|x| !list2.contains(x)).cloned().collect()
}
