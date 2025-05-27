use eyre::{Result, eyre};
use imap::Session;
use native_tls::TlsStream;
use std::net::TcpStream;
use log::{info, debug};
use std::collections::HashSet;
use regex::Regex;

/// Validates that an IMAP search query uses supported flags and syntax.
pub fn validate_imap_query(query: &str) -> Result<()> {
    // Allowed base tokens: standard flags + Gmail extensions
    let valid_tokens = [
        "ALL", "ANSWERED", "DELETED", "DRAFT", "FLAGGED", "NEW", "OLD",
        "RECENT", "SEEN", "UNANSWERED", "UNDELETED", "UNDRAFT", "UNFLAGGED", "UNSEEN",
        "X-GM-LABELS", "X-GM-RAW", "X-GM-THRID", "X-GM-MSGID",
        "INBOX",  // technically not a search keyword, but Gmail often uses it
        "NOT", "OR", "AND"
    ];

    // Basic sanity checks — more can be added
    if query.trim().is_empty() {
        return Err(eyre!("IMAP query must not be empty"));
    }

    if query.contains('\\') {
        // Check if flags like \Seen, \Flagged etc. are correctly escaped
        if !query.contains("\\Seen") &&
           !query.contains("\\Deleted") &&
           !query.contains("\\Flagged") &&
           !query.contains("\\Draft") &&
           !query.contains("\\Answered") {
            return Err(eyre!("Unknown or improperly escaped IMAP flag in query: {}", query));
        }
    }

    // Basic token scan — not a full parser, but catches most errors
    for token in query.split_whitespace() {
        let t = token.trim_matches(|c| c == '(' || c == ')' || c == '"');
        if t.starts_with("X-GM-LABELS") || valid_tokens.iter().any(|&v| v.eq_ignore_ascii_case(t)) {
            continue;
        } else if t.starts_with("\\") {
            // Might be valid, already checked above
            continue;
        } else if t.chars().all(char::is_alphanumeric) {
            // Possibly a user-defined label or UID
            continue;
        } else {
            return Err(eyre!("Unsupported or malformed token in IMAP query: '{}'", token));
        }
    }

    Ok(())
}

/// Ensures the given label exists on the server.
/// If the label already exists, this is a no-op.
/// If it doesn't, attempts to create it.
pub fn ensure_label_exists(client: &mut Session<TlsStream<TcpStream>>, label: &str) -> Result<()> {
    let list = client.list(None, Some("*"))?;
    let exists = list.iter().any(|item| item.name() == label);

    if !exists {
        info!("Creating missing label '{}'", label);
        client.create(label).map_err(|e| eyre!("Failed to create label '{}': {:?}", label, e))?;
        info!("✅ Label '{}' created successfully", label);
    }

    Ok(())
}

pub fn get_labels(session: &mut Session<TlsStream<TcpStream>>, seq: u32) -> Result<HashSet<String>> {
    let fetches = session.fetch(seq.to_string(), "X-GM-LABELS")?;
    let mut labels = HashSet::new();

    for fetch in fetches.iter() {
        let raw = format!("{:?}", fetch);
        debug!("FETCH raw: {}", raw);

        if let Some(start) = raw.find("X-GM-LABELS (") {
            let rest = &raw[start + "X-GM-LABELS (".len()..];
            if let Some(end) = rest.find(')') {
                let label_str = &rest[..end];

                for label in label_str.split_whitespace() {
                    let label = label.trim_matches('"');
                    if !label.is_empty() {
                        labels.insert(label.to_string());
                    }
                }
            }
        }
    }

    Ok(labels)
}

pub fn set_label(
    client: &mut Session<TlsStream<TcpStream>>,
    seq: u32,
    label: &str,
    subject: &str,
) -> Result<()> {
    let current_labels = get_labels(client, seq)?;
    if current_labels.contains(label) {
        debug!(
            "Label '{}' already present on seq {} — skipping. Subject: {}",
            label, seq, subject
        );
        return Ok(());
    }

    ensure_label_exists(client, label)?;

    let cmd = format!("+X-GM-LABELS (\"{}\")", label);
    client
        .store(seq.to_string(), &cmd)
        .map(|_| ())
        .map_err(|e| eyre!(
            "Failed to set label '{}' on seq {}: {:?} | Subject: {}",
            label, seq, e, subject
        ))
}

pub fn del_label(
    client: &mut Session<TlsStream<TcpStream>>,
    seq: u32,
    label: &str,
    subject: &str,
) -> Result<()> {
    let cmd = format!("-X-GM-LABELS (\"{}\")", label);
    client
        .store(seq.to_string(), &cmd)
        .map(|_| ())
        .map_err(|e| eyre!(
            "Failed to remove label '{}' from seq {}: {:?} | Subject: {}",
            label, seq, e, subject
        ))
}

pub fn format_gmail_label(label: &str) -> String {
    let escaped = label.replace('\\', "\\\\").replace('"', "\\\"");
    format!("+X-GM-LABELS (\"{}\")", escaped)
}
