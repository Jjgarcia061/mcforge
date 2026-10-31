use serde::Serialize;

/// Single error type for the whole core. Variants carry a message so the UI
/// receives a typed `{ kind, message }` object instead of an opaque string.
/// Exported to TypeScript as `AppError` so it does not shadow the built-in `Error`.
#[derive(Debug, thiserror::Error, Serialize, specta::Type)]
#[serde(
    tag = "kind",
    content = "message",
    rename_all = "camelCase",
    rename = "AppError"
)]
pub enum Error {
    #[error("invalid catalog: {0}")]
    Catalog(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("keychain error: {0}")]
    Keychain(String),
    #[error("{0}")]
    Provider(String),
    #[error("database error: {0}")]
    Database(String),
    #[error("{0}")]
    Io(String),
    #[error("report error: {0}")]
    Report(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e.to_string())
    }
}

impl From<keyring::Error> for Error {
    fn from(e: keyring::Error) -> Self {
        Self::Keychain(e.to_string())
    }
}

impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<minijinja::Error> for Error {
    fn from(e: minijinja::Error) -> Self {
        // The alternate form includes the template line that failed.
        Self::Report(format!("{e:#}"))
    }
}
