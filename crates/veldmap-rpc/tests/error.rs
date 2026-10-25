use connectrpc::ErrorCode;
use veldmap_proto::veldmap::common::v1::ItemErrorCode;
use veldmap_rpc::error::DomainError;

#[test]
fn recoverable_items_and_terminal_errors_remain_distinct() {
    let item = DomainError::new(ErrorCode::Unavailable, "upstream unavailable")
        .with_retryable(true)
        .into_item_error("tile-7")
        .unwrap();

    assert_eq!(item.id, "tile-7");
    assert_eq!(item.code, ItemErrorCode::Unavailable);
    assert_eq!(item.message, "upstream unavailable");
    assert!(item.retryable);

    let terminal = DomainError::new(ErrorCode::OutOfRange, "cursor evicted")
        .into_item_error("download-1")
        .unwrap_err();
    assert_eq!(terminal.code, ErrorCode::OutOfRange);
    assert_eq!(terminal.message.as_deref(), Some("cursor evicted"));
}
