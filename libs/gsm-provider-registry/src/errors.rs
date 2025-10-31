use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// Error emitted by provider adapters.
#[derive(Debug)]
pub struct MsgError {
    code: String,
    message: String,
    retryable: bool,
    backoff_ms: Option<u64>,
    #[allow(dead_code)]
    source: Option<anyhow::Error>,
}

impl MsgError {
    /// Creates a non-retryable error with the provided code and message.
    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            backoff_ms: None,
            source: None,
        }
    }

    /// Creates a retryable error with an optional backoff hint in milliseconds.
    pub fn retryable(
        code: impl Into<String>,
        message: impl Into<String>,
        backoff_ms: Option<u64>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: true,
            backoff_ms,
            source: None,
        }
    }

    /// Attaches a source error for debugging purposes.
    pub fn with_source(mut self, source: impl Into<anyhow::Error>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Returns the machine-readable error code.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Returns the descriptive error message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Indicates whether the failure should be retried.
    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    /// Optional backoff hint in milliseconds.
    pub fn backoff_ms(&self) -> Option<u64> {
        self.backoff_ms
    }
}

impl Display for MsgError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl Error for MsgError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|err| err.as_ref() as &(dyn Error + 'static))
    }
}

impl From<anyhow::Error> for MsgError {
    fn from(err: anyhow::Error) -> Self {
        MsgError::permanent("internal_error", err.to_string()).with_source(err)
    }
}
