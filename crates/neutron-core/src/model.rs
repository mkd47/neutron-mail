use serde::{Deserialize, Serialize};

/// Transport security for a server connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// Implicit TLS (IMAPS 993 / SMTPS 465).
    Tls,
    /// Plain connection upgraded with STARTTLS (IMAP 143 / submission 587).
    Starttls,
}

impl Security {
    pub fn as_str(self) -> &'static str {
        match self {
            Security::Tls => "tls",
            Security::Starttls => "starttls",
        }
    }
    pub fn parse(s: &str) -> crate::Result<Self> {
        match s {
            "tls" => Ok(Security::Tls),
            "starttls" => Ok(Security::Starttls),
            other => Err(crate::Error::Invalid(format!(
                "unknown security mode {other}"
            ))),
        }
    }
}

/// How the account logs in. OAuth2 tokens live in the keyring like
/// passwords; this only selects the SASL mechanism (XOAUTH2 vs LOGIN/PLAIN).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    #[default]
    Password,
    OauthGoogle,
    OauthMicrosoft,
}

impl AuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::OauthGoogle => "oauth_google",
            Self::OauthMicrosoft => "oauth_microsoft",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "oauth_google" => Self::OauthGoogle,
            "oauth_microsoft" => Self::OauthMicrosoft,
            _ => Self::Password,
        }
    }
    pub fn is_oauth(self) -> bool {
        self != Self::Password
    }
}

/// Server settings for one account. Contains no password.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountConfig {
    pub email: String,
    pub display_name: String,
    pub username: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: Security,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: Security,
    /// Optional extra trusted CA certificate (PEM), e.g. for a self-hosted
    /// server with a private CA. Certificate validation is never disabled.
    #[serde(default)]
    pub extra_ca_pem: Option<String>,
    #[serde(default)]
    pub auth: AuthMethod,
}

impl AccountConfig {
    pub fn validate(&self) -> crate::Result<()> {
        let bad = |f: &str| Err(crate::Error::Invalid(format!("{f} is required")));
        if self.email.trim().is_empty() || !self.email.contains('@') {
            return bad("a valid email address");
        }
        if self.username.trim().is_empty() {
            return bad("username");
        }
        if self.imap_host.trim().is_empty() {
            return bad("IMAP host");
        }
        if self.smtp_host.trim().is_empty() {
            return bad("SMTP host");
        }
        if self.imap_port == 0 || self.smtp_port == 0 {
            return bad("a non-zero port");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Account {
    pub id: i64,
    #[serde(flatten)]
    pub config: AccountConfig,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Folder {
    pub id: i64,
    pub account_id: i64,
    pub name: String,
    pub role: Option<String>,
    pub unread: i64,
    pub total: i64,
    /// Decoded (Unicode) full name for display; `name` is the wire name.
    pub display: String,
    /// IMAP hierarchy delimiter, e.g. "." or "/".
    pub delimiter: Option<String>,
    pub subscribed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MessageSummary {
    pub id: i64,
    pub folder_id: i64,
    pub uid: u32,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub date: i64,
    pub preview: String,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AttachmentInfo {
    /// Position in the message's attachment list (stable for a given raw
    /// message); used to save/open/forward the attachment.
    pub index: usize,
    /// Display name, already made safe (see `attachments::safe_filename`).
    pub name: String,
    pub mime: String,
    pub size: usize,
    /// Executable/script type: the UI must confirm before opening.
    pub risky: bool,
}

/// One mailbox from an address header (name may be empty).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailAddress {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MessageDetail {
    #[serde(flatten)]
    pub summary: MessageSummary,
    pub to: String,
    pub cc: String,
    /// The same recipients as `to`/`cc`, one entry per address.
    pub to_list: Vec<MailAddress>,
    pub cc_list: Vec<MailAddress>,
    pub text: String,
    /// Sanitized HTML, safe to place in a sandboxed iframe. `None` for
    /// text-only messages.
    pub html: Option<String>,
    pub remote_images_blocked: usize,
    /// Remote images that were fetched and embedded (only when the user
    /// asked to load remote images).
    pub remote_images_loaded: usize,
    pub attachments: Vec<AttachmentInfo>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchHit {
    #[serde(flatten)]
    pub summary: MessageSummary,
    /// Snippet with matches wrapped in U+0002 ... U+0003 markers. The UI
    /// renders these as text spans (never as HTML).
    pub snippet: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct OutgoingMessage {
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    /// Envelope-only recipients: never written to the sent message headers.
    #[serde(default)]
    pub bcc: Vec<String>,
    pub subject: String,
    /// Plain-text body (the text/plain part).
    pub body: String,
    /// Rich body from the HTML editor; when set the message is sent as
    /// multipart/alternative (text + HTML). Sanitized again before sending.
    #[serde(default)]
    pub html_body: Option<String>,
    /// Message-ID being replied to (with or without angle brackets).
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub attachments: Vec<AttachmentRef>,
}

/// An attachment chosen in the composer. Files are referenced by an opaque
/// token issued by the backend file picker, never by a path from the UI.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttachmentRef {
    File {
        token: String,
    },
    /// Attachment `index` of cached message `message_id` (forward/draft).
    Message {
        message_id: i64,
        index: usize,
    },
}

/// Resolved attachment content, ready to be put into a MIME message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingAttachment {
    pub name: String,
    pub mime: String,
    pub data: Vec<u8>,
}

/// Composer prefill for reply / reply all / forward / resume draft.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ComposeDraft {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    /// Initial content of the HTML editor (sanitized for compose).
    pub html: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// Attachments carried over (forward / draft), shown as removable chips.
    pub attachments: Vec<ForwardedAttachment>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ForwardedAttachment {
    pub message_id: i64,
    pub index: usize,
    pub name: String,
    pub mime: String,
    pub size: usize,
}

/// Address suggestion for recipient autocomplete.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContactSuggestion {
    pub name: String,
    pub email: String,
    /// "contacts" (GNOME address book via EDS) or "history" (mail cache).
    pub source: &'static str,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SyncReport {
    pub folders: usize,
    pub new_messages: usize,
    pub removed_messages: usize,
    /// Folders that could not be synced this round (retried next time).
    pub failed_folders: Vec<String>,
    /// New INBOX messages that matched a local rule.
    pub rules_matched: usize,
    pub rule_errors: Vec<String>,
}
