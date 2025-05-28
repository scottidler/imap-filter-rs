use eyre::{Result, eyre};
use imap::Session;
use native_tls::TlsStream;
use std::net::TcpStream;
use log::{info, debug};
use std::collections::HashSet;
use chrono::{DateTime, Duration, Utc};
use std::io::{Read, Write};
use regex::Regex;

/// Parse a string like "7d" into a chrono::Duration of days.
/// Returns an error if the format is unsupported.
pub fn parse_days(s: &str) -> Result<Duration, eyre::ErrReport> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix('d') {
        let days: i64 = num.parse()
            .map_err(|e| eyre::eyre!("Invalid TTL duration '{}': {}", s, e))?;
        Ok(Duration::days(days))
    } else {
        Err(eyre::eyre!(
            "Unsupported TTL format '{}'; expected '<n>d'",
            s
        ))
    }
}

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
pub fn ensure_label_exists<T>(
    client: &mut Session<T>,
    label: &str,
) -> Result<()>
where
    T: Read + Write,
{
    // List all mailboxes and check for existence
    let list = client.list(None, Some("*"))?;
    let exists = list.iter().any(|item| item.name() == label);

    // Create if missing
    if !exists {
        info!("Creating missing label '{}'", label);
        client
            .create(label)
            .map_err(|e| eyre!("Failed to create label '{}': {:?}", label, e))?;
        info!("✅ Label '{}' created successfully", label);
    }

    Ok(())
}

/// Returns the set of Gmail labels currently on this message (by UID).
pub fn get_labels<T>(
    session: &mut Session<T>,
    uid: u32,
) -> Result<HashSet<String>>
where
    T: Read + Write,
{
    let fetches = session.fetch(uid.to_string(), "X-GM-LABELS")?;
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

/// Adds `label` to the message (by UID), creating the label if needed.
pub fn set_label<T>(
    client: &mut Session<T>,
    uid: u32,
    label: &str,
    subject: &str,
) -> Result<()>
where
    T: Read + Write,
{
    let current_labels = get_labels(client, uid)?;
    if current_labels.contains(label) {
        debug!(
            "Label '{}' already present on UID {} — skipping. Subject: {}",
            label, uid, subject
        );
        return Ok(());
    }

    ensure_label_exists(client, label)?;

    let cmd = format!("+X-GM-LABELS (\"{}\")", label.replace('\\', "\\\\").replace('"', "\\\""));
    client
        .store(uid.to_string(), &cmd)
        .map(|_| ())
        .map_err(|e| eyre!(
            "Failed to set label '{}' on UID {}: {:?} | Subject: {}",
            label, uid, e, subject
        ))
}

/// Removes `label` from the message (by UID).
pub fn del_label<T>(
    client: &mut Session<T>,
    uid: u32,
    label: &str,
    subject: &str,
) -> Result<()>
where
    T: Read + Write,
{
    let cmd = format!("-X-GM-LABELS (\"{}\")", label.replace('\\', "\\\\").replace('"', "\\\""));
    client
        .store(uid.to_string(), &cmd)
        .map(|_| ())
        .map_err(|e| eyre!(
            "Failed to remove label '{}' from UID {}: {:?} | Subject: {}",
            label, uid, e, subject
        ))
}

/// “Move” a Gmail message by UID: add your target label and remove INBOX.
/// Mirrors `client.uid_mv`, but on Gmail must explicitly STORE labels.
pub fn uid_move<T>(
    client: &mut Session<T>,
    uid: u32,
    label: &str,
    subject: &str,
) -> Result<()>
where
    T: Read + Write,
{
    set_label(client, uid, label, subject)?;
    del_label(client, uid, "INBOX", subject)?;
    Ok(())
}

/// Strip Gmail’s system INBOX label from a message by UID.
pub fn del_inbox<T>(
    client: &mut Session<T>,
    uid: u32,
    subject: &str,
) -> Result<()>
where
    T: Read + Write,
{
    client
        .uid_store(
            uid.to_string(),
            // remove the system INBOX flag (note the leading backslash)
            "-X-GM-LABELS (\"\\INBOX\")",
        )
        .map(|_| ())   // drop the Vec<Fetch>
        .map_err(|e| eyre!(
            "Failed to remove \\INBOX from UID {}: {:?} | Subject: {}",
            uid, e, subject
        ))
}

/// Move a Gmail message by UID: add your target label (creating if needed), then strip INBOX.
pub fn uid_move_gmail<T>(
    client: &mut Session<T>,
    uid: u32,
    label: &str,
    subject: &str,
) -> Result<()>
where
    T: Read + Write,
{
    // 1) add (and create, if missing) the custom label
    set_label(client, uid, label, subject)?;
    // 2) remove the system INBOX label
    del_inbox(client, uid, subject)?;
    Ok(())
}
