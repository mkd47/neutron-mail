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
    pub name: String,
    pub mime: String,
    pub size: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MessageDetail {
    #[serde(flatten)]
    pub summary: MessageSummary,
    pub to: String,
    pub cc: String,
    pub text: String,
    /// Sanitized HTML, safe to place in a sandboxed iframe. `None` for
    /// text-only messages.
    pub html: Option<String>,
    pub remote_images_blocked: usize,
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

#[derive(Debug, Clone, Deserialize)]
pub struct OutgoingMessage {
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SyncReport {
    pub folders: usize,
    pub new_messages: usize,
    pub removed_messages: usize,
    /// Folders that could not be synced this round (retried next time).
    pub failed_folders: Vec<String>,
}
