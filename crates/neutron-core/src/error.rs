use thiserror::Error;

/// Errors surfaced to the UI. Messages never contain passwords.
#[derive(Debug, Error)]
pub enum Error {
    #[error("IMAP error: {0}")]
    Imap(String),
    #[error("SMTP error: {0}")]
    Smtp(String),
    #[error("TLS error: {0}")]
    Tls(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("credential store error: {0}")]
    Secret(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("operation timed out")]
    Timeout,
}

impl From<async_imap::error::Error> for Error {
    fn from(e: async_imap::error::Error) -> Self {
        Error::Imap(e.to_string())
    }
}

impl From<lettre::transport::smtp::Error> for Error {
    fn from(e: lettre::transport::smtp::Error) -> Self {
        Error::Smtp(e.to_string())
    }
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
