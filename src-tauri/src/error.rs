// SPDX-License-Identifier: MPL-2.0
//! Single error type returned to the frontend by Tauri commands.

use std::path::{Path, PathBuf};

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::catalog::CatalogError;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// A file operation, with the path it was about: a bare `io::Error` names no file, so there
    /// is deliberately no `From<io::Error>`. Use [`IoContext::at`].
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Anything else, as a message ready for the user.
    #[error("{0}")]
    Other(String),
}

impl AppError {
    /// What the frontend can test without parsing the message.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Database(_) => "database",
            Self::Catalog(CatalogError::Network(_)) => "network",
            Self::Catalog(_) => "catalog",
            Self::Io { .. } => "io",
            Self::Other(_) => "other",
        }
    }
}

impl From<String> for AppError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

/// Attaches the path to an I/O error: `std::fs::write(&path, text).at(&path)?`.
pub trait IoContext<T> {
    fn at(self, path: impl AsRef<Path>) -> AppResult<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl AsRef<Path>) -> AppResult<T> {
        self.map_err(|source| AppError::Io {
            path: path.as_ref().to_path_buf(),
            source,
        })
    }
}

// Tauri requires `Serialize`: the frontend receives `{ kind, message }`.
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut error = serializer.serialize_struct("AppError", 2)?;
        error.serialize_field("kind", self.kind())?;
        error.serialize_field("message", &self.to_string())?;
        error.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_with_a_kind_and_a_message() {
        let error = AppError::from("Unknown station: x".to_string());
        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            serde_json::json!({ "kind": "other", "message": "Unknown station: x" })
        );
    }

    #[test]
    fn an_io_error_names_its_path() {
        let missing = Path::new("/nonexistent/soundfolk/file.toml");
        let error = std::fs::read_to_string(missing).at(missing).unwrap_err();
        assert_eq!(error.kind(), "io");
        let message = error.to_string();
        assert!(message.starts_with("/nonexistent/soundfolk/file.toml: "));
    }

    #[test]
    fn a_catalog_network_error_has_its_own_kind() {
        let error = AppError::from(CatalogError::Network("timeout".into()));
        assert_eq!(error.kind(), "network");
    }
}
