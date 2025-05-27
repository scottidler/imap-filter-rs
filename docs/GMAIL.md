# IMAP FLAGS vs. GMAIL LABELS REFERENCE

This document summarizes the differences between **standard IMAP flags** and **Gmail's label system** when accessing Gmail programmatically via IMAP.

---

## 📨 IMAP Flags

IMAP flags are standard message-level metadata that track the state of an email. These are part of the [IMAP protocol specification (RFC 3501)](https://datatracker.ietf.org/doc/html/rfc3501).

| IMAP Flag   | Description                                   |
|-------------|-----------------------------------------------|
| `\Seen`     | Email has been read                           |
| `\Answered` | Email has been replied to                     |
| `\Flagged`  | Marked as important (e.g., starred)           |
| `\Deleted`  | Marked for deletion                           |
| `\Draft`    | Draft message                                 |
| `\Recent`   | Recently arrived message (read-only, server managed) |

> Note: `\Read` is **not** a real IMAP flag — people often use it informally to mean `\Seen`.

These flags are universal in IMAP and not Gmail-specific.

---

## 🏷 Gmail Labels (IMAP Folders)

Gmail uses **labels instead of folders**. Messages can have multiple labels (unlike IMAP folders where a message lives in exactly one).

When accessed via IMAP, Gmail labels appear as folders such as:

- `[Gmail]/Inbox`
- `[Gmail]/Starred`
- `[Gmail]/Important`
- `[Gmail]/Drafts`
- `[Gmail]/Sent Mail`
- `[Gmail]/Spam`
- `[Gmail]/Trash`
- `[Gmail]/All Mail`
- User-defined labels like `Projects`, `Clients`, `Finance`, etc.

Gmail provides proprietary IMAP extensions:

- `X-GM-LABELS`: Lists all labels applied to a message
- `X-GM-THRID`: Gmail thread ID (useful for threading)
- `X-GM-MSGID`: Gmail message ID

More on Gmail IMAP extensions:
<https://developers.google.com/gmail/imap/imap-extensions>

---

## 🔍 Comparison Table

| Feature         | IMAP Flag        | Gmail Label / Folder        |
|-----------------|------------------|------------------------------|
| Read/Unread     | `\Seen`          | Not a label                 |
| Starred         | `\Flagged`       | `[Gmail]/Starred`           |
| Important       | *(not standard)* | `[Gmail]/Important`         |
| Draft           | `\Draft`         | `[Gmail]/Drafts`            |
| Deleted         | `\Deleted`       | `[Gmail]/Trash`             |
| Spam            | *(not standard)* | `[Gmail]/Spam`              |
| Sent            | *(not standard)* | `[Gmail]/Sent Mail`         |
| All Messages    | *(N/A)*          | `[Gmail]/All Mail`          |
| Custom Tags     | *(N/A)*          | Custom user labels          |

---

## ⚠ Key Differences

- IMAP flags describe **state**; Gmail labels define **categorization**.
- A Gmail message may exist in **multiple "folders"** at once (i.e., it has multiple labels).
- IMAP clients that don't understand labels will treat them as folders.
- Flags like `\Seen` work universally. Labels like "Starred" might **sync** with `\Flagged` depending on the client.

---

## ✅ Best Practices for Developers

- Use `\Seen` to track read/unread, not Gmail-specific labels.
- Use `X-GM-LABELS` to access or modify Gmail labels when supported.
- For "Starred", check both:
  - `\Flagged` (standard)
  - `[Gmail]/Starred` (label)
- Deleting from `[Gmail]/Inbox` does not delete the message — it just removes the "Inbox" label. The message remains in `[Gmail]/All Mail` unless explicitly moved to Trash.
- Consider using the [Gmail API](https://developers.google.com/gmail/api) instead of IMAP for modern integrations.

---

## 🔗 References

- Gmail IMAP Overview: <https://support.google.com/mail/answer/7126229>
- Gmail IMAP Extensions: <https://developers.google.com/gmail/imap/imap-extensions>
- IMAP Protocol (RFC 3501): <https://datatracker.ietf.org/doc/html/rfc3501>

---
```GMAIL.md``` End of Document
