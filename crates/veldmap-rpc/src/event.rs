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

        let limit = selection.max_events.unwrap_or(usize::MAX);
        let initial = match selection.view {
            View::VIEW_SNAPSHOT | View::VIEW_UNSPECIFIED => state
                .live
                .values()
                .filter(|live| selection.includes(&live.meta))
                .take(limit)
                .map(|live| Event {
                    meta: EventMeta {
                        op: Op::OP_SNAPSHOT.into(),
                        ..live.meta.clone()
                    },
                    data: Some(live.value.clone()),
                })
                .collect::<Vec<_>>(),
            View::VIEW_EVENTS => state
                .history
                .iter()
                .filter(|event| selection.includes(&event.meta))
                .take(limit)
                .cloned()
                .collect::<Vec<_>>(),
        };
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
            if state
                .history
                .binary_search_by(|event| event.meta.event_id.as_str().cmp(cursor.as_str()))
                .is_err()
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
