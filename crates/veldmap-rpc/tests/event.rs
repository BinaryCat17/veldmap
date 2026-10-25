use std::num::NonZeroUsize;
use std::time::Duration;

use buffa_types::google::protobuf::Timestamp;
use connectrpc::{ConnectError, ErrorCode};
use futures::StreamExt;
use tokio::time::timeout;
use ulid::Ulid;
use veldmap_proto::veldmap::common::v1::__buffa::oneof::read_options::{End, Start};
use veldmap_proto::veldmap::common::v1::read_options::View;
use veldmap_proto::veldmap::common::v1::{Op, ReadOptions};
use veldmap_rpc::event::{Event, EventStream, EventTable};

fn capacity(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap()
}

fn events_options() -> ReadOptions {
    ReadOptions {
        view: View::VIEW_EVENTS.into(),
        ..Default::default()
    }
}

fn after_cursor(cursor: &str) -> ReadOptions {
    ReadOptions {
        view: View::VIEW_EVENTS.into(),
        start: Some(Start::AfterEventId(cursor.to_owned())),
        ..Default::default()
    }
}

fn event_time(event: &Event<u32>) -> Box<Timestamp> {
    Box::new(
        event
            .meta
            .time
            .as_option()
            .expect("event timestamp is missing")
            .clone(),
    )
}

async fn next(stream: &mut EventStream<u32>) -> Result<Event<u32>, ConnectError> {
    timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("event stream timed out")
        .expect("event stream ended early")
}

async fn collect(mut stream: EventStream<u32>) -> Vec<Event<u32>> {
    let mut events = Vec::new();
    while let Some(event) = timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("finite event stream timed out")
    {
        events.push(event.unwrap());
    }
    events
}

#[tokio::test(flavor = "current_thread")]
async fn cursors_are_strictly_ordered_and_replay_in_order() {
    let table = EventTable::new(capacity(8));
    let first = table.upsert("a", 1).unwrap();
    let second = table.upsert("b", 2).unwrap();
    let third = table.upsert("c", 3).unwrap();

    assert!(first.meta.event_id < second.meta.event_id);
    assert!(second.meta.event_id < third.meta.event_id);
    Ulid::from_string(&first.meta.event_id).unwrap();

    let events = collect(table.read(&events_options()).unwrap()).await;
    let ids = events
        .iter()
        .map(|event| event.meta.event_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            first.meta.event_id.as_str(),
            second.meta.event_id.as_str(),
            third.meta.event_id.as_str()
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn replay_rejects_unknown_and_evicted_cursors() {
    let table = EventTable::new(capacity(2));
    let evicted = table.upsert("a", 1).unwrap();
    let retained = table.upsert("b", 2).unwrap();
    let latest = table.upsert("c", 3).unwrap();

    for cursor in [evicted.meta.event_id, Ulid::nil().to_string()] {
        let error = table
            .read(&after_cursor(&cursor))
            .err()
            .expect("replay cursor was accepted");
        assert_eq!(error.code, ErrorCode::OutOfRange);
    }

    let events = collect(table.read(&after_cursor(&retained.meta.event_id)).unwrap()).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].meta.event_id, latest.meta.event_id);
}

#[tokio::test(flavor = "current_thread")]
async fn time_bounds_and_max_events_apply_together() {
    let table = EventTable::new(capacity(8));
    let first = table.upsert("a", 1).unwrap();
    std::thread::sleep(Duration::from_millis(2));
    table.upsert("b", 2).unwrap();
    std::thread::sleep(Duration::from_millis(2));
    let third = table.upsert("c", 3).unwrap();
    let options = ReadOptions {
        view: View::VIEW_EVENTS.into(),
        start: Some(Start::AfterTime(event_time(&first))),
        end: Some(End::BeforeTime(event_time(&third))),
        max_events: 1,
        ..Default::default()
    };

    let events = collect(table.read(&options).unwrap()).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].meta.id, "b");
    assert_eq!(events[0].data, Some(2));
}

#[tokio::test(flavor = "current_thread")]
async fn deletion_removes_live_value_and_omits_event_data() {
    let table = EventTable::new(capacity(8));
    table.upsert("a", 1).unwrap();
    let deleted = table.remove("a").unwrap().unwrap();

    assert!(deleted.data.is_none());
    assert_eq!(deleted.meta.op, Op::OP_DELETED);
    assert_eq!(table.get("a"), None);
    assert!(table.is_empty());
    assert!(table.remove("a").unwrap().is_none());

    let events = collect(table.read(&events_options()).unwrap()).await;
    assert_eq!(events.len(), 2);
    assert!(events[1].data.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_live_transition_respects_limit_without_loss() {
    let table = EventTable::new(capacity(8));
    let original = table.upsert("a", 1).unwrap();
    let options = ReadOptions {
        end: Some(End::Live(true)),
        max_events: 3,
        ..Default::default()
    };
    let mut stream = table.read(&options).unwrap();
    let updated = table.upsert("a", 2).unwrap();
    let created = table.upsert("b", 3).unwrap();

    let snapshot = next(&mut stream).await.unwrap();
    assert_eq!(snapshot.meta.op, Op::OP_SNAPSHOT);
    assert_eq!(snapshot.meta.event_id, original.meta.event_id);
    assert_eq!(snapshot.data, Some(1));

    let update = next(&mut stream).await.unwrap();
    assert_eq!(update.meta.op, Op::OP_UPDATED);
    assert_eq!(update.meta.event_id, updated.meta.event_id);
    assert_eq!(update.data, Some(2));

    let create = next(&mut stream).await.unwrap();
    assert_eq!(create.meta.op, Op::OP_CREATED);
    assert_eq!(create.meta.event_id, created.meta.event_id);
    assert_eq!(create.data, Some(3));
    assert!(
        timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("live stream ignored max_events")
            .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lagging_live_subscriber_gets_terminal_replay_error() {
    let table = EventTable::new(capacity(1));
    let options = ReadOptions {
        view: View::VIEW_EVENTS.into(),
        end: Some(End::Live(true)),
        ..Default::default()
    };
    let mut stream = table.read(&options).unwrap();
    table.upsert("a", 1).unwrap();
    table.upsert("b", 2).unwrap();

    let error = next(&mut stream).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::OutOfRange);
}
