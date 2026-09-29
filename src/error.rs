use std::fmt;

/// Stable error categories. Match on the code rather than the message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorCode {
    UnsupportedFormat,
    DecodeError,
    EncodeError,
    InvalidArgument,
    LimitExceeded,
    Io,
}

/// An error with a stable category and human-readable context.
#[derive(Debug)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }
    pub(crate) fn decode(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::DecodeError, message)
    }
    pub(crate) fn encode(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::EncodeError, message)
    }
    pub(crate) fn limit(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::LimitExceeded, message)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::new(ErrorCode::Io, value.to_string())
    }
}
pub type Result<T> = std::result::Result<T, Error>;
