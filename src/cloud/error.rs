//! Errors of the cloud client, named after rmapi-js's.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CloudError {
    /// The root changed since it was read (retried internally first).
    #[error("root generation was stale; try put again")]
    Generation,
    /// The item's hash is not in the root: it changed since it was read.
    #[error("hash {hash} not found")]
    HashNotFound { hash: String },
    /// A failed request.
    #[error("{message}")]
    Response {
        status: u16,
        status_text: String,
        message: String,
    },
    /// Registration refused.
    #[error("couldn't register api")]
    Register {
        status: u16,
        status_text: String,
        body: String,
    },
    /// Session token refused.
    #[error("couldn't fetch auth token")]
    Auth {
        status: u16,
        status_text: String,
        body: String,
    },
    /// The request failed without a response.
    #[error("{message}")]
    Network { message: String },
    /// Invalid input.
    #[error("{message}")]
    Validation { message: String },
    #[error("{message}")]
    Other { message: String },
}

impl CloudError {
    pub(crate) fn other(msg: impl Into<String>) -> Self {
        CloudError::Other {
            message: msg.into(),
        }
    }
}
