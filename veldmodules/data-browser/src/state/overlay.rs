//! Image overlays on the globe: what is being assembled and what is already shown.
//!
//! There are many overlays, and the order in the set is the layer order from
//! bottom to top, the same order the globe understands it in (see its
//! `Overlays`). The one shown last lies on top of the rest; the on-screen list
//! goes in reverse order — new ones on top — and reverses its view, not its state.
//!
//! Building one overlay goes in three beats — rasters from the provider, opening
//! each with a resource, handing ownership to the globe — and all of this survives
//! the handlers, so it lives here rather than in local variables.

use veldsdk::proto::core::ResourceHandle;

use crate::proto::globe::{OverlayRaster, UtmFrame, Variable};
use super::ViewId;

pub struct OverlayState {
    /// The overlay's product; also the overlay's key at the globe.
    pub identifier: String,
    pub label: String,
    /// Whether the product is stored as a folder in the repository: it selects
    /// the way the product is viewed (see `handlers::preview`).
    pub folder: bool,
    /// The search view the product was taken from: the overlay lives its life —
    /// leaves with the product from the results and with the tab closing. `None` —
    /// the product was restored by key (catalogue, downloads) and does not depend
    /// on results: only "remove" takes it down.
    pub source: Option<ViewId>,
    /// The footprint quad (UL, UR, LR, LL) — a fallback binding for products
    /// without a UTM frame. `None` — the product had no geometry at all.
    pub quad: Option<[(f64, f64); 4]>,
    /// The quad is not the contour's corners but its bounding box: the contour
    /// itself is more complex than a quad. A raster must not be fitted by it, and
    /// the globe does not — it waits for the binding from the raster (see
    /// `Overlay.rough` in the contract).
    pub rough: bool,
    /// Where to look to see the whole image. Computed once, from the product at
    /// hand at the moment of showing: asking the catalogue again for the sake of
    /// aiming would be a network request for what is already known. `None` — the
    /// product had no geometry, and there is nothing to aim at.
    pub focus: Option<crate::module::footprint::Frame>,
    /// Layer opacity, 0..1.
    pub opacity: f32,
    /// The layer is hidden: it stays in the list and in the globe's set together
    /// with its resources, but is not drawn. That is how it differs from "remove":
    /// bringing it back opens nothing anew.
    pub hidden: bool,
    /// The rasters-request correlation while the answer is in flight. Remembered
    /// so the request can be taken into account when the layer is removed: else the
    /// answer of a removed layer would be recognized as a layer placed under the
    /// same key again, and would start a second assembly on top of the first.
    pub imagery: Option<String>,
    /// The opening of rasters with resources; appears with the provider's answer
    /// and lives until ownership is handed to the globe.
    pub assembly: Option<Assembly>,
    /// Tile mining progress as the globe reported it. These numbers have no
    /// meaning of their own here — they are displayed, and that is all.
    pub progress: Progress,
    /// What the layer will not have, although the layer itself landed: one of its
    /// rasters failed to describe. Next to the progress, but not progress — its
    /// limit, and it is spoken when the progress is over and there is nothing more
    /// to say.
    pub trouble: String,
    /// File keys the provider answered the rasters request with, by position in
    /// the answer: preview, detailed, and spares after it. The globe knows no names
    /// and calls the one lying detailed by its number (`OverlayProgress.detailed`)
    /// — here the number becomes a file name.
    pub files: Vec<String>,
    /// The name of the file lying detailed right now. Empty — there is no detailed
    /// one or the globe has not spoken about it yet.
    pub detailed: Option<String>,
    /// What is said about the detailed raster — its refusal and limit when detail
    /// is decided by it (`OverlayProgress.detailed_trouble`); `trouble` has none of
    /// this. The file name is set right before this and only before this.
    pub detailed_trouble: String,
    /// Which variable of the file lying detailed is shown
    /// (`OverlayProgress.detailed_variable`): for a file of many variables the
    /// choice is made by the tiler, and in the layer row this is the only place
    /// where it is named. Empty — there is no detailed one, it is not described, or
    /// it has a single variable (an image).
    pub variable: Option<Variable>,
    /// Variables of the file lying detailed, to choose from
    /// (`OverlayProgress.detailed_variables`), from the last report carrying them.
    pub variables: Vec<Variable>,
    /// The number of the one lying detailed (`OverlayProgress.detailed`) — the
    /// named variable refers to it.
    pub detailed_ordinal: Option<u32>,
    /// The file the viewer asked to place as the image (`ImageryRequest.wanted`):
    /// it lands detailed, the provider's choice lands as its spare. Empty — the
    /// provider's choice.
    pub wanted_file: Option<String>,
    /// The variable named by the viewer: file number and path. Goes to the globe
    /// in `OverlayRaster.variable` of that raster; cleared when another file (a
    /// spare) lands detailed — what was named about the previous one does not
    /// refer to it.
    pub variable_wanted: Option<(u32, String)>,
    /// Rasters already handed to the globe, and the frame that went along with
    /// them. Ownership over them is gone — these are the names by which the set is
    /// re-sent in full when its composition or display changes: the globe
    /// recognizes the overlay as the same and does not reopen it (see its
    /// `Overlays`).
    pub rasters: Vec<OverlayRaster>,
    pub utm: Option<UtmFrame>,
}

impl OverlayState {
    pub fn new(
        identifier: String,
        label: String,
        folder: bool,
        source: Option<ViewId>,
        quad: Option<([(f64, f64); 4], bool)>,
        focus: Option<crate::module::footprint::Frame>,
    ) -> Self {
        Self {
            identifier,
            label,
            folder,
            source,
            quad: quad.map(|(points, _)| points),
            rough: quad.is_some_and(|(_, rough)| rough),
            focus,
            opacity: 1.0,
            hidden: false,
            imagery: None,
            assembly: None,
            progress: Progress::default(),
            trouble: String::new(),
            files: Vec::new(),
            detailed: None,
            detailed_trouble: String::new(),
            variable: None,
            variables: Vec::new(),
            detailed_ordinal: None,
            wanted_file: None,
            variable_wanted: None,
            rasters: Vec::new(),
            utm: None,
        }
    }

    /// The overlay has reached the globe. Until then it is being assembled:
    /// asking for rasters or opening them.
    pub fn on_globe(&self) -> bool {
        !self.rasters.is_empty()
    }
}

/// Tile mining progress of a layer — what the globe reported about it (see its
/// `OverlayProgress`).
///
/// An image does not land on the globe at once: the pyramid is filled from the
/// top down, and from "icon clicked" to a detailed picture tens of seconds pass.
/// On the globe this is invisible — there is either already something or there is
/// not — but the layer list stands right before the eyes, and that is where such
/// a label belongs.
#[derive(Clone, Copy, PartialEq, Default)]
pub struct Progress {
    /// Cells of the latest order mined and how many there are in it.
    pub ready: u32,
    pub total: u32,
    /// The layer's share of the way, 0..1. The globe counts it in pyramid steps —
    /// the panorama does not move it (see its `OverlayProgress`).
    pub share: f32,
    /// Work is not over. Separate from the share: a share can be one even for a
    /// layer that is about to be asked for more detail.
    pub working: bool,
    /// Which pyramid step is being filled and how many there are in total —
    /// exactly what the share counts off.
    pub step: u32,
    pub steps: u32,
    /// The layer is on the globe, but there is nothing to draw from it yet: the
    /// raster is still being described. We have no answer to this and cannot have
    /// one — "lies on the globe" and "visible on the globe" are indistinguishable
    /// from here — so the globe says it (see its `OverlayProgress.blank`).
    pub blank: bool,
    /// Bytes of the source read out of how many — what the globe counted fit for
    /// display. A zero denominator means "nothing to measure with", not "zero
    /// bytes", and it has four reasons: the pyramid disentangles them, not us (see
    /// `OverlayProgress.pass_read` in the globe's contract).
    ///
    /// These numbers have no meaning of their own here — they are displayed, and
    /// that is all.
    pub pass: (u64, u64),
}

/// What the bar under a row answers to the question "how much longer".
///
/// An enumeration, not a share with zero standing in for the answer: "work is
/// going, but there is no share" and "nothing done yet" are different things, and
/// recorded as one number they render the same — that is, not at all. An empty
/// three-dot track is exactly "not at all", and every showing on the globe starts
/// from it: while the raster is being described, there is nothing to count in it,
/// and this lasts tens of seconds.
#[derive(Clone, Copy, PartialEq)]
pub enum Pace {
    /// Work is over — the bar has nothing to take off from.
    Idle,
    /// Work is going, but there is no share: rasters are opening or the raster is
    /// being described.
    Unknown,
    /// Share of the way, 0..1.
    Share(f32),
}

impl Progress {
    /// Progress in parts, from senior to junior. Empty — there is nothing to say:
    /// everything this view needs is already on the globe.
    ///
    /// Parts, not a ready-made phrase, because it has two places and they are of
    /// different widths: two parts fit into a column cell, all of them into a
    /// tooltip. Assembled separately, the phrase would diverge between them in
    /// words, not just in length.
    ///
    /// Each part explains what the neighbors do not. The step is the measuring
    /// unit of the bar: without it the viewer sees that the bar moved and does not
    /// know whether by a step or by a quarter of a tile. Bytes are the only thing
    /// moving while the step stands still on one cell: a sequential source is read
    /// in full, and per cell nothing is done all that time. Cells are what is
    /// driving right now inside the step.
    ///
    /// The order here is the seniority: when squeezed, the label drops parts from
    /// the tail (see `components::table::fit_label`).
    pub fn parts(&self) -> Vec<String> {
        if !self.working {
            return Vec::new();
        }
        let mut said = Vec::new();
        if self.steps > 0 {
            said.push(format!("step {}/{}", self.step + 1, self.steps));
        }
        if self.pass.1 > 0 {
            said.push(crate::module::components::format::progress(self.pass.0, self.pass.1));
        }
        said.push(match self.total {
            // Nothing to ask for: the raster is being described or the step is
            // already closed.
            0 => "preparing…".to_string(),
            total => format!("tiles {}/{}", self.ready, total),
        });
        said
    }

    /// The same progress as one phrase — for the tooltip, where there is room for
    /// everything. `None` — there is nothing to say.
    pub fn said(&self) -> Option<String> {
        let parts = self.parts();
        match parts.is_empty() {
            true => None,
            false => Some(parts.join(" · ")),
        }
    }

    /// What draws the bar under a list row (see [`Pace`]).
    ///
    /// The share is cancelled not by its value but by `blank`: a share can be zero
    /// for an honestly started step, and for a one-level pyramid there are zero
    /// steps even when the picture has long been in place. Derived from numbers,
    /// "no share" would be confused now with one, now with the other.
    pub fn pace(&self) -> Pace {
        match (self.working, self.blank) {
            (false, _) => Pace::Idle,
            (true, true) => Pace::Unknown,
            (true, false) => Pace::Share(self.share),
        }
    }
}

/// What the file being opened is to the overlay. The raster is what will be
/// shown; the coordinates are a neighboring file in which Sentinel-3 holds the
/// latitude and longitude of its pixels. They must be distinguished: into the
/// overlay they travel in different fields, yet they arrive in the very same
/// open reply.
#[derive(Clone, Copy, PartialEq)]
pub enum Part {
    Raster,
    Geolocation,
}

/// Overlay assembly: the frame from the provider and the rasters being opened.
pub struct Assembly {
    pub utm: Option<UtmFrame>,
    /// File keys of the provider's answer by position — they land in
    /// `OverlayState::files` together with the rasters, in `finish`: before the set
    /// is replaced the globe reports on the old one, and its numbers would be read
    /// against the new list.
    pub files: Vec<String>,
    /// Correlations of the openings still awaited. A list, not a table: the table
    /// for this topic is one and it lives in State (see `State::opens`) — here is
    /// only what takes the assembly off it when the overlay is removed. Else the
    /// answer of an aborted assembly would be recognized as the next assembly of
    /// the same key.
    pub opens: Vec<String>,
    /// Opened files (role, raster number in the provider's answer, and what kind
    /// of file it is → resource); ownership stays with us until handover to the
    /// globe. The role is already translated into the one the globe understands —
    /// the translation is done once, at the boundary (see
    /// handlers::overlay::role_for_globe). The number is so that the coordinates
    /// find their raster when there are several detailed ones.
    pub collected: Vec<(crate::proto::globe::OverlayRole, u32, Part, ResourceHandle)>,
}
