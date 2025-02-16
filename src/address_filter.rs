use serde::{Deserialize};

use globset::Glob;

#[derive(Debug, Deserialize)]
pub struct AddressFilter {
    pub patterns: Vec<String>,
}

impl AddressFilter {
    pub fn matches(&self, emails: &[String]) -> bool {
        self.patterns.iter().any(|pattern| {
            let glob = Glob::new(pattern).expect("Invalid glob pattern").compile_matcher();
            emails.iter().any(|email| glob.is_match(email))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::AddressFilter;
    use std::collections::HashSet;

    fn test_emails() -> Vec<String> {
        vec![
            "scott.idler@tatari.tv".to_string(),
            "someone@gmail.com".to_string(),
            "admin@tatari.tv".to_string(),
            "user@example.com".to_string(),
            "noreply@github.com".to_string(),
        ]
    }

    #[test]
    fn test_address_filter_single_match() {
        let filter = AddressFilter {
            patterns: vec!["*@tatari.tv".to_string()],
        };
        let emails = test_emails();

        let expected_matches = vec!["scott.idler@tatari.tv", "admin@tatari.tv"];
        let actual_matches: Vec<_> = emails
            .iter()
            .filter(|email| filter.matches(&vec![email.to_string()]))
            .collect();

        assert_eq!(actual_matches, expected_matches);
    }
}

