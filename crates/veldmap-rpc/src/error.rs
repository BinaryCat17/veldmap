use connectrpc::{ConnectError, ErrorCode};
use veldmap_proto::veldmap::common::v1::{ItemError, ItemErrorCode};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct DomainError {
    kind: ErrorCode,
    message: String,
    retryable: bool,
}

impl DomainError {
    #[must_use]
    pub fn new(kind: ErrorCode, message: impl Into<String>) -> Self {
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
    pub fn kind(&self) -> ErrorCode {
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
        ConnectError::new(self.kind, self.message)
    }

    pub fn into_item_error(self, id: impl Into<String>) -> Result<ItemError, ConnectError> {
        let Some(code) = item_code(self.kind) else {
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

fn item_code(code: ErrorCode) -> Option<ItemErrorCode> {
    match code {
        ErrorCode::InvalidArgument => Some(ItemErrorCode::InvalidArgument),
        ErrorCode::NotFound => Some(ItemErrorCode::NotFound),
        ErrorCode::AlreadyExists => Some(ItemErrorCode::AlreadyExists),
        ErrorCode::PermissionDenied => Some(ItemErrorCode::PermissionDenied),
        ErrorCode::ResourceExhausted => Some(ItemErrorCode::ResourceExhausted),
        ErrorCode::FailedPrecondition => Some(ItemErrorCode::FailedPrecondition),
        ErrorCode::Internal => Some(ItemErrorCode::Internal),
        ErrorCode::Unavailable => Some(ItemErrorCode::Unavailable),
        ErrorCode::DataLoss => Some(ItemErrorCode::DataLoss),
        ErrorCode::Unauthenticated => Some(ItemErrorCode::Unauthenticated),
        _ => None,
    }
}

fn from_item_code(code: Option<ItemErrorCode>) -> ErrorCode {
    match code {
        Some(ItemErrorCode::InvalidArgument) => ErrorCode::InvalidArgument,
        Some(ItemErrorCode::NotFound) => ErrorCode::NotFound,
        Some(ItemErrorCode::AlreadyExists) => ErrorCode::AlreadyExists,
        Some(ItemErrorCode::PermissionDenied) => ErrorCode::PermissionDenied,
        Some(ItemErrorCode::ResourceExhausted) => ErrorCode::ResourceExhausted,
        Some(ItemErrorCode::FailedPrecondition) => ErrorCode::FailedPrecondition,
        Some(ItemErrorCode::Internal) => ErrorCode::Internal,
        Some(ItemErrorCode::Unavailable) => ErrorCode::Unavailable,
        Some(ItemErrorCode::DataLoss) => ErrorCode::DataLoss,
        Some(ItemErrorCode::Unauthenticated) => ErrorCode::Unauthenticated,
        Some(ItemErrorCode::Unspecified) | None => ErrorCode::Unknown,
    }
}

impl From<ItemError> for DomainError {
    fn from(error: ItemError) -> Self {
        Self {
            kind: from_item_code(error.code.as_known()),
            message: error.message,
            retryable: error.retryable,
        }
    }
}
