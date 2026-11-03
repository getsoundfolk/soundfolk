// SPDX-License-Identifier: MPL-2.0
//! Single error type returned to the frontend by Tauri commands.

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("{0}")]
    Audio(String),
    #[error(transparent)]
    Catalog(#[from] crate::catalog::CatalogError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl From<String> for AppError {
    fn from(message: String) -> Self {
        Self::Audio(message)
    }
}

// Tauri requires `Serialize`: the frontend simply receives the message.
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
