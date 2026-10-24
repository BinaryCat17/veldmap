use connectrpc::{ConnectError, ErrorCode};
use veldmap_proto::veldmap::common::v1::{ItemError, ItemErrorCode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainErrorKind {
    Canceled,
    Unknown,
    InvalidArgument,
    DeadlineExceeded,
    NotFound,
    AlreadyExists,
    PermissionDenied,
    ResourceExhausted,
    FailedPrecondition,
    Aborted,
    OutOfRange,
    Unimplemented,
    Internal,
    Unavailable,
    DataLoss,
    Unauthenticated,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct DomainError {
    kind: DomainErrorKind,
    message: String,
    retryable: bool,
}

impl DomainError {
    #[must_use]
    pub fn new(kind: DomainErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable: false,
        }
    }

    #[must_use]
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    #[must_use]
    pub fn kind(&self) -> DomainErrorKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn retryable(&self) -> bool {
        self.retryable
    }

    #[must_use]
    pub fn into_connect_error(self) -> ConnectError {
        ConnectError::new(self.kind.connect_code(), self.message)
    }

    pub fn into_item_error(self, id: impl Into<String>) -> Result<ItemError, ConnectError> {
        let Some(code) = self.kind.item_code() else {
            return Err(self.into_connect_error());
        };
        Ok(ItemError {
            id: id.into(),
            code: code.into(),
            message: self.message,
            retryable: self.retryable,
            ..Default::default()
        })
    }
}

impl DomainErrorKind {
    fn connect_code(self) -> ErrorCode {
        match self {
            Self::Canceled => ErrorCode::Canceled,
            Self::Unknown => ErrorCode::Unknown,
            Self::InvalidArgument => ErrorCode::InvalidArgument,
            Self::DeadlineExceeded => ErrorCode::DeadlineExceeded,
            Self::NotFound => ErrorCode::NotFound,
            Self::AlreadyExists => ErrorCode::AlreadyExists,
            Self::PermissionDenied => ErrorCode::PermissionDenied,
            Self::ResourceExhausted => ErrorCode::ResourceExhausted,
            Self::FailedPrecondition => ErrorCode::FailedPrecondition,
            Self::Aborted => ErrorCode::Aborted,
            Self::OutOfRange => ErrorCode::OutOfRange,
            Self::Unimplemented => ErrorCode::Unimplemented,
            Self::Internal => ErrorCode::Internal,
            Self::Unavailable => ErrorCode::Unavailable,
            Self::DataLoss => ErrorCode::DataLoss,
            Self::Unauthenticated => ErrorCode::Unauthenticated,
        }
    }

    fn item_code(self) -> Option<ItemErrorCode> {
        match self {
            Self::InvalidArgument => Some(ItemErrorCode::InvalidArgument),
            Self::NotFound => Some(ItemErrorCode::NotFound),
            Self::AlreadyExists => Some(ItemErrorCode::AlreadyExists),
            Self::PermissionDenied => Some(ItemErrorCode::PermissionDenied),
            Self::ResourceExhausted => Some(ItemErrorCode::ResourceExhausted),
            Self::FailedPrecondition => Some(ItemErrorCode::FailedPrecondition),
            Self::Internal => Some(ItemErrorCode::Internal),
            Self::Unavailable => Some(ItemErrorCode::Unavailable),
            Self::DataLoss => Some(ItemErrorCode::DataLoss),
            Self::Unauthenticated => Some(ItemErrorCode::Unauthenticated),
            Self::Canceled
            | Self::Unknown
            | Self::DeadlineExceeded
            | Self::Aborted
            | Self::OutOfRange
            | Self::Unimplemented => None,
        }
    }

    fn from_item_code(code: Option<ItemErrorCode>) -> Self {
        match code {
            Some(ItemErrorCode::InvalidArgument) => Self::InvalidArgument,
            Some(ItemErrorCode::NotFound) => Self::NotFound,
            Some(ItemErrorCode::AlreadyExists) => Self::AlreadyExists,
            Some(ItemErrorCode::PermissionDenied) => Self::PermissionDenied,
            Some(ItemErrorCode::ResourceExhausted) => Self::ResourceExhausted,
            Some(ItemErrorCode::FailedPrecondition) => Self::FailedPrecondition,
            Some(ItemErrorCode::Internal) => Self::Internal,
            Some(ItemErrorCode::Unavailable) => Self::Unavailable,
            Some(ItemErrorCode::DataLoss) => Self::DataLoss,
            Some(ItemErrorCode::Unauthenticated) => Self::Unauthenticated,
            Some(ItemErrorCode::Unspecified) | None => Self::Unknown,
        }
    }
}

impl From<ItemError> for DomainError {
    fn from(error: ItemError) -> Self {
        Self {
            kind: DomainErrorKind::from_item_code(error.code.as_known()),
            message: error.message,
            retryable: error.retryable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recoverable_items_and_terminal_errors_remain_distinct() {
        let item = DomainError::new(DomainErrorKind::Unavailable, "upstream unavailable")
            .with_retryable(true)
            .into_item_error("tile-7")
            .unwrap();

        assert_eq!(item.id, "tile-7");
        assert_eq!(item.code, ItemErrorCode::Unavailable);
        assert_eq!(item.message, "upstream unavailable");
        assert!(item.retryable);

        let terminal = DomainError::new(DomainErrorKind::OutOfRange, "cursor evicted")
            .into_item_error("download-1")
            .unwrap_err();
        assert_eq!(terminal.code, ErrorCode::OutOfRange);
        assert_eq!(terminal.message.as_deref(), Some("cursor evicted"));
    }
}
