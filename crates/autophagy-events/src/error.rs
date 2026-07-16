use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

/// One field-addressed semantic validation failure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationError {
    /// JSON-style path to the rejected value.
    pub path: String,
    /// Stable machine-readable error code.
    pub code: String,
    /// Human-readable diagnostic.
    pub message: String,
}

impl ValidationError {
    pub(crate) fn new(path: impl Into<String>, code: &str, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

/// All semantic failures found in one event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationErrors(Vec<ValidationError>);

impl ValidationErrors {
    pub(crate) fn new(errors: Vec<ValidationError>) -> Self {
        Self(errors)
    }

    /// Iterate over individual field failures.
    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &ValidationError> {
        self.0.iter()
    }

    /// Consume the collection.
    #[must_use]
    pub fn into_vec(self) -> Vec<ValidationError> {
        self.0
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(formatter, "{}: {}", error.path, error.message)?;
        }
        Ok(())
    }
}

impl Error for ValidationErrors {}

/// Failure to decode or semantically validate one event.
#[derive(Debug, thiserror::Error)]
pub enum EventParseError {
    /// The input was not a structurally valid AEP JSON event.
    #[error("event JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// The event decoded but violated AEP semantic invariants.
    #[error("event semantics are invalid: {0}")]
    Validation(#[from] ValidationErrors),
}
