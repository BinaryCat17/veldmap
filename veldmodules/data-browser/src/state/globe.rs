//! What belongs to the globe: the tab that shows it, and what is outlined on it.
//!
//! No camera here: it belongs to whoever draws. Here is exactly what that one
//! cannot know — the slot allocated for it, the gesture in progress, and the link
//! of outlines to the snapshots they came from.

use veldsdk::surface::Delegated;

use crate::proto::data_provider::Ring;

#[derive(Default)]
pub struct GlobeState {
    /// The texture under the view: ours, given to the globe for writing and to
    /// ui-service for reading. Released together with the tab — `Delegated` owns
    /// it.
    pub surface: Option<Delegated>,
    /// A gesture with the button held. `None` — the button is not pressed, and
    /// there is nothing to track.
    pub dragging: Option<Drag>,
}

pub struct Drag {
    /// Where the pointer was when we last computed the offset.
    pub last: (f32, f32),
    /// How far it has travelled since the press, in pixels. By it a click is
    /// distinguished from a drag — otherwise a hand tremor on release would select
    /// a snapshot.
    pub travelled: f32,
}

/// A snapshot outlined on the globe.
///
/// The set of them is assembled from requests (`State::outlines`, see
/// handlers::outline) and lives in the module state, not the tab's: the globe is
/// one, and what is outlined on it is an application-wide property. What is
/// already drawn lies here: a request the catalog has not answered yet does not
/// make it in.
pub struct Outlined {
    /// The provider key — the snapshot is requested to be outlined by it, and a
    /// click on the globe recognizes it by the same key.
    pub key: String,
    /// What to sign it with in the bar under the globe.
    pub label: String,
    /// Whether the snapshot lies in the catalog's storage: it selects the way to
    /// view it (see `handlers::preview`).
    pub folder: bool,
    /// The acquisition geometry. There may be several rings — the catalog gives
    /// out a snapshot across the 180th meridian cut into pieces.
    pub rings: Vec<Ring>,
}
