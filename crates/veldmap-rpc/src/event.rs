use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

use buffa_types::google::protobuf::Timestamp;
use connectrpc::ConnectError;
use futures::Stream;
use tokio::sync::broadcast;
use ulid::{Generator, Ulid};
use veldmap_proto::veldmap::common::v1::__buffa::oneof::read_options::{End, Start};
use veldmap_proto::veldmap::common::v1::read_options::View;
use veldmap_proto::veldmap::common::v1::{EventMeta, Op, ReadOptions};

pub type EventStream<V> = Pin<Box<dyn Stream<Item = Result<Event<V>, ConnectError>> + Send>>;

#[derive(Clone, Debug, PartialEq)]
pub struct Event<V> {
    pub meta: EventMeta,
    pub data: Option<V>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EventError {
    #[error("event cursor space is exhausted")]
    CursorOverflow,
}

impl From<EventError> for ConnectError {
    fn from(error: EventError) -> Self {
        match error {
            EventError::CursorOverflow => ConnectError::internal(error.to_string()),
        }
    }
}

struct LiveValue<V> {
    meta: EventMeta,
    value: V,
}

struct State<V> {
    live: BTreeMap<String, LiveValue<V>>,
    history: VecDeque<Event<V>>,
    generator: Generator,
}

pub struct EventTable<V> {
    state: Mutex<State<V>>,
    history_capacity: usize,
    live_sender: broadcast::Sender<Event<V>>,
}

impl<V> EventTable<V>
where
    V: Clone + Send + Sync + 'static,
{
    #[must_use]
    pub fn new(history_capacity: NonZeroUsize) -> Self {
        let capacity = history_capacity.get();
        let (live_sender, _) = broadcast::channel(capacity);
        Self {
            state: Mutex::new(State {
                live: BTreeMap::new(),
                history: VecDeque::with_capacity(capacity),
                generator: Generator::new(),
            }),
            history_capacity: capacity,
            live_sender,
        }
    }

    pub fn upsert(&self, id: impl Into<String>, value: V) -> Result<Event<V>, EventError> {
        self.upsert_at(id.into(), value, SystemTime::now())
    }

    pub fn remove(&self, id: &str) -> Result<Option<Event<V>>, EventError> {
        self.remove_at(id, SystemTime::now())
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<V> {
        self.lock().live.get(id).map(|live| live.value.clone())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().live.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().live.is_empty()
    }

    pub fn read(&self, options: &ReadOptions) -> Result<EventStream<V>, ConnectError> {
        let selection = Selection::from_options(options)?;
        let state = self.lock();
        selection.validate_cursors(&state)?;

        let mut initial = match selection.view {
            View::VIEW_SNAPSHOT | View::VIEW_UNSPECIFIED => state
                .live
                .values()
                .map(|live| Event {
                    meta: EventMeta {
                        op: Op::OP_SNAPSHOT.into(),
                        ..live.meta.clone()
                    },
                    data: Some(live.value.clone()),
                })
                .collect::<Vec<_>>(),
            View::VIEW_EVENTS => state.history.iter().cloned().collect::<Vec<_>>(),
        };
        initial.retain(|event| selection.includes(&event.meta));
        if let Some(max_events) = selection.max_events {
            initial.truncate(max_events);
        }
        let mut receiver = selection.live.then(|| self.live_sender.subscribe());
        drop(state);

        let stream = async_stream::stream! {
            let mut emitted = 0_usize;
            for event in initial {
                emitted += 1;
                yield Ok(event);
            }

            while !selection.limit_reached(emitted) {
                let Some(receiver) = receiver.as_mut() else {
                    break;
                };
                match receiver.recv().await {
                    Ok(event) => {
                        if !selection.includes(&event.meta) {
                            continue;
                        }
                        emitted += 1;
                        yield Ok(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        yield Err(ConnectError::out_of_range(
                            "live event cursor is no longer retained",
                        ));
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Ok(Box::pin(stream))
    }

    fn upsert_at(&self, id: String, value: V, time: SystemTime) -> Result<Event<V>, EventError> {
        let mut state = self.lock();
        let op = if state.live.contains_key(&id) {
            Op::OP_UPDATED
        } else {
            Op::OP_CREATED
        };
        let event = make_event(&mut state, id.clone(), op, Some(value.clone()), time)?;
        state.live.insert(
            id,
            LiveValue {
                meta: event.meta.clone(),
                value,
            },
        );
        self.record_locked(&mut state, event.clone());
        Ok(event)
    }

    fn remove_at(&self, id: &str, time: SystemTime) -> Result<Option<Event<V>>, EventError> {
        let mut state = self.lock();
        if !state.live.contains_key(id) {
            return Ok(None);
        }
        let event = make_event(&mut state, id.to_owned(), Op::OP_DELETED, None, time)?;
        state.live.remove(id);
        self.record_locked(&mut state, event.clone());
        Ok(Some(event))
    }

    fn record_locked(&self, state: &mut State<V>, event: Event<V>) {
        if state.history.len() == self.history_capacity {
            state.history.pop_front();
        }
        state.history.push_back(event.clone());
        let _ = self.live_sender.send(event);
    }

    fn lock(&self) -> MutexGuard<'_, State<V>> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

fn make_event<V>(
    state: &mut State<V>,
    id: String,
    op: Op,
    data: Option<V>,
    time: SystemTime,
) -> Result<Event<V>, EventError> {
    let event_id = state
        .generator
        .generate_from_datetime(time)
        .map_err(|_| EventError::CursorOverflow)?
        .to_string();
    Ok(Event {
        meta: EventMeta {
            event_id,
            id,
            time: Timestamp::from(time).into(),
            op: op.into(),
            ..Default::default()
        },
        data,
    })
}

struct Selection {
    view: View,
    after_cursor: Option<String>,
    after_time: Option<(i64, i32)>,
    before_cursor: Option<String>,
    before_time: Option<(i64, i32)>,
    live: bool,
    max_events: Option<usize>,
}

impl Selection {
    fn from_options(options: &ReadOptions) -> Result<Self, ConnectError> {
        let view = options.view.as_known().ok_or_else(|| {
            ConnectError::invalid_argument("read options contain an unknown view")
        })?;
        let mut selection = Self {
            view,
            after_cursor: None,
            after_time: None,
            before_cursor: None,
            before_time: None,
            live: false,
            max_events: (options.max_events != 0).then_some(options.max_events as usize),
        };
        match &options.start {
            Some(Start::AfterEventId(cursor)) => selection.after_cursor = Some(cursor.clone()),
            Some(Start::AfterTime(time)) => {
                selection.after_time = Some(valid_time(time)?);
            }
            None => {}
        }
        match &options.end {
            Some(End::BeforeEventId(cursor)) => selection.before_cursor = Some(cursor.clone()),
            Some(End::BeforeTime(time)) => {
                selection.before_time = Some(valid_time(time)?);
            }
            Some(End::Live(live)) => selection.live = *live,
            None => {}
        }
        Ok(selection)
    }

    fn validate_cursors<V>(&self, state: &State<V>) -> Result<(), ConnectError> {
        for cursor in [&self.after_cursor, &self.before_cursor]
            .into_iter()
            .flatten()
        {
            Ulid::from_string(cursor)
                .map_err(|_| ConnectError::invalid_argument("invalid event cursor"))?;
            if !state
                .history
                .iter()
                .any(|event| &event.meta.event_id == cursor)
            {
                return Err(ConnectError::out_of_range(
                    "event cursor is unknown or no longer retained",
                ));
            }
        }
        Ok(())
    }

    fn includes(&self, meta: &EventMeta) -> bool {
        if self
            .after_cursor
            .as_ref()
            .is_some_and(|cursor| meta.event_id <= *cursor)
        {
            return false;
        }
        if self
            .before_cursor
            .as_ref()
            .is_some_and(|cursor| meta.event_id >= *cursor)
        {
            return false;
        }
        let time = meta
            .time
            .as_option()
            .map_or((0, 0), |time| (time.seconds, time.nanos));
        if self.after_time.is_some_and(|after| time <= after) {
            return false;
        }
        if self.before_time.is_some_and(|before| time >= before) {
            return false;
        }
        true
    }

    fn limit_reached(&self, emitted: usize) -> bool {
        self.max_events
            .is_some_and(|max_events| emitted >= max_events)
    }
}

fn valid_time(time: &Timestamp) -> Result<(i64, i32), ConnectError> {
    if !(0..=999_999_999).contains(&time.nanos) {
        return Err(ConnectError::invalid_argument(
            "timestamp nanos must be between 0 and 999999999",
        ));
    }
    Ok((time.seconds, time.nanos))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use connectrpc::ErrorCode;
    use futures::StreamExt;
    use tokio::time::timeout;

    use super::*;

    fn capacity(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).unwrap()
    }

    fn time(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
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
        let first = table.upsert_at("a".into(), 1, time(10)).unwrap();
        let second = table.upsert_at("b".into(), 2, time(10)).unwrap();
        let third = table.upsert_at("c".into(), 3, time(11)).unwrap();

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
        let evicted = table.upsert_at("a".into(), 1, time(1)).unwrap();
        let retained = table.upsert_at("b".into(), 2, time(2)).unwrap();
        let latest = table.upsert_at("c".into(), 3, time(3)).unwrap();

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
        table.upsert_at("a".into(), 1, time(10)).unwrap();
        table.upsert_at("b".into(), 2, time(20)).unwrap();
        table.upsert_at("c".into(), 3, time(30)).unwrap();
        let options = ReadOptions {
            view: View::VIEW_EVENTS.into(),
            start: Some(Start::AfterTime(Box::new(Timestamp::from_unix_secs(10)))),
            end: Some(End::BeforeTime(Box::new(Timestamp::from_unix_secs(30)))),
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
        table.upsert_at("a".into(), 1, time(1)).unwrap();
        let deleted = table.remove_at("a", time(2)).unwrap().unwrap();

        assert!(deleted.data.is_none());
        assert_eq!(deleted.meta.op, Op::OP_DELETED);
        assert_eq!(table.get("a"), None);
        assert!(table.is_empty());
        assert!(table.remove_at("a", time(3)).unwrap().is_none());

        let events = collect(table.read(&events_options()).unwrap()).await;
        assert_eq!(events.len(), 2);
        assert!(events[1].data.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_live_transition_respects_limit_without_loss() {
        let table = EventTable::new(capacity(8));
        let original = table.upsert_at("a".into(), 1, time(1)).unwrap();
        let options = ReadOptions {
            end: Some(End::Live(true)),
            max_events: 3,
            ..Default::default()
        };
        let mut stream = table.read(&options).unwrap();
        let updated = table.upsert_at("a".into(), 2, time(2)).unwrap();
        let created = table.upsert_at("b".into(), 3, time(3)).unwrap();

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
        table.upsert_at("a".into(), 1, time(1)).unwrap();
        table.upsert_at("b".into(), 2, time(2)).unwrap();

        let error = next(&mut stream).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::OutOfRange);
    }
}
