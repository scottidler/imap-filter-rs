use globset::{Glob, GlobMatcher};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SubjectFilter {
    pub patterns: Vec<String>,
}

impl SubjectFilter {
    pub fn matches(&self, subject: &str) -> bool {
        self.patterns.iter().any(|pattern| {
            let matcher = Glob::new(pattern)
                .expect("Invalid glob pattern in subject filter")
                .compile_matcher();
            matcher.is_match(subject)
        })
    }
}
