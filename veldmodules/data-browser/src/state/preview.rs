//! Preview view. Shows exactly one scene: a second one is a second tab with its
//! own state, not a swap of the content here.
//!
//! The scene itself does not live here: the resource is opened by the library
//! or the provider and ownership passes to the canvas (image-view) along with
//! on_show -- from there the canvas manages the tiles, the camera, and the
//! frame. What stays here is what the layout needs: the place for the canvas,
//! the progress of opening, and the last truth delivered about the display.

use crate::proto::image_view::ViewState;

/// Pointer capture over the canvas: dragging pans.
pub struct Drag {
    pub last: (f32, f32),
}

#[derive(Default)]
pub struct PreviewState {
    /// How the scene is labeled -- on the tab, in the toolbar, and in the log.
    /// Only a label: nothing can be addressed by it.
    pub label: String,
    /// The library record, if the scene is opened from a downloaded file: only
    /// it has a size and a time on the properties panel.
    pub entry: Option<String>,
    /// The place for the canvas frame -- our texture, delegated to image-view by
    /// the same procedure as the globe place.
    pub surface: Option<veldsdk::surface::Delegated>,
    /// The progress of opening the resource (data-library or data-provider).
    /// The display starts with its success; after that the truth about what is
    /// happening arrives via the canvas broadcast.
    pub request: veldsdk::Latest,
    /// A failure to open -- the only error we see, not the canvas.
    pub error: Option<String>,
    /// The last display state from image-view: zoom, source size, production
    /// progress, its error. We keep no copy of this truth of our own.
    pub view_state: Option<ViewState>,
    /// The variable named by the viewer (`ShowRequest.variable`); empty -- the
    /// tiler's choice. Ours, not the canvas's: the canvas says what is shown,
    /// this says what was asked for, and after a variable refusal they diverge.
    pub variable: String,
    /// The file's variables as named by the canvas last time
    /// (`ViewState.variables`). Kept: a canvas that refused the named variable
    /// sends no list, and picking another one is needed exactly then.
    pub variables: Vec<crate::proto::image_view::Variable>,
    /// The frame place has already been reissued on a canvas complaint. One
    /// attempt per complaint: a failure can be momentary (the texture was
    /// swapped while the event was in flight) -- that one is cured on the first
    /// try -- while a persistent one (not enough video memory) is not cured by
    /// a repeat at all, and without this counter the canvas and the layout
    /// owner would chase textures in a circle. Cleared as soon as the canvas
    /// says it has the place.
    pub replaced: bool,
    pub dragging: Option<Drag>,
}

impl PreviewState {
    /// The display failure, whoever owns it: opening or the tile pipeline.
    pub fn failure(&self) -> Option<&str> {
        if let Some(error) = &self.error {
            return Some(error);
        }
        self.view_state
            .as_ref()
            .map(|state| state.error.as_str())
            .filter(|error| !error.is_empty())
    }

    /// Starts a new open request: it becomes current, the previous one does not.
    pub fn begin(&mut self) -> String {
        self.request.begin()
    }

    /// The named variable that the canvas is actually showing; empty -- the
    /// tiler's choice or a named one the canvas refused. The tab is recorded to
    /// disk with it: a recorded refused one would reopen with the same refusal
    /// after a restart, and a fresh tab has no list under it yet.
    pub fn confirmed_variable(&self) -> String {
        let shown = self.view_state.as_ref().and_then(|state| state.variable.as_ref());
        match shown {
            Some(shown) if shown.path == self.variable => self.variable.clone(),
            _ => String::new(),
        }
    }
}
