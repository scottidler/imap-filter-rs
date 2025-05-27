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

    #[test]
    fn test_matches_with_single_pattern() {
        let filter = AddressFilter {
            patterns: vec!["*@tatari.tv".to_string()],
        };

        let matching = vec!["alice@tatari.tv".to_string()];
        let non_matching = vec!["bob@example.com".to_string()];

        assert!(filter.matches(&matching));
        assert!(!filter.matches(&non_matching));
    }

    #[test]
    fn test_matches_with_multiple_patterns() {
        let filter = AddressFilter {
            patterns: vec!["*@tatari.tv".to_string(), "noreply@github.com".to_string()],
        };

        let emails = vec!["noreply@github.com".to_string()];
        assert!(filter.matches(&emails));
    }

    #[test]
    fn test_does_not_match_any() {
        let filter = AddressFilter {
            patterns: vec!["*@tatari.tv".to_string()],
        };

        let emails = vec!["user@outlook.com".to_string(), "admin@example.org".to_string()];
        assert!(!filter.matches(&emails));
    }

    #[test]
    fn test_empty_filter_does_not_match() {
        let filter = AddressFilter {
            patterns: vec![],
        };

        let emails = vec!["scott.idler@tatari.tv".to_string()];
        assert!(!filter.matches(&emails));
    }

    #[test]
    #[should_panic(expected = "Invalid glob pattern")]
    fn test_invalid_glob_panics() {
        let _ = AddressFilter {
            patterns: vec!["invalid[glob".to_string()],
        }
        .matches(&["test@example.com".to_string()]);
    }

    #[test]
    fn test_partial_match_with_multiple_emails() {
        let filter = AddressFilter {
            patterns: vec!["*@tatari.tv".to_string()],
        };

        let emails = vec![
            "random@foo.com".to_string(),
            "matchme@tatari.tv".to_string(),
            "junk@bar.org".to_string(),
        ];

        assert!(filter.matches(&emails));
    }

    #[test]
    fn test_username_wildcard_match() {
        let filter = AddressFilter {
            patterns: vec!["scott.*@tatari.tv".to_string()],
        };

        let emails = vec!["scott.idler@tatari.tv".to_string()];
        assert!(filter.matches(&emails));
    }
}
