//! Handlers for data-browser

pub mod search;
pub mod browse;
pub mod globe;
pub mod library;
pub mod listing;
pub mod nav;
pub mod outline;
pub mod persist;
pub mod overlay;
pub mod preview;
pub mod window;

use veldsdk::graphics::TextureFormat;

/// Open a file with the resource that owns it: a downloaded one — with the
/// library, because the file is at hand; anything else — with the provider, over
/// the network. `local` is the library record name, used when the file lies on
/// disk in full (`LibraryState::local_name`); the tab and the overlay assembly
/// remember it as the way the file was opened. Both openers answer with a single
/// `core.ResourceOpened` per this correlation, and beyond that there is no
/// difference. Waiting is tracked by each caller, so it raises the correlation
/// itself.
pub fn open_resource(identifier: String, local: Option<String>, correlation: &str) {
    match local {
        Some(name) => crate::calls::data_library::on_open(
            &crate::proto::data_library::OpenRequest { name },
            correlation,
        ),
        None => crate::calls::data_provider::on_open(
            &crate::proto::data_provider::OpenRequest { identifier },
            correlation,
        ),
    }
}

/// Format of the slot for a foreign render — both for the view canvas and for the
/// globe. UNORM with sRGB values: the markup samples this texture as an ordinary
/// picture and linearizes it itself, while if the GPU encoded it a second time,
/// the tab would arrive darker than its neighbor. One for both slots, because the
/// rule is one: were they to diverge, they would give exactly this difference, and
/// it would be visible only by comparing them.
pub const SURFACE_FORMAT: TextureFormat = TextureFormat::TexRgba8Unorm;

#[derive(serde::Deserialize)]
pub struct Config {
    /// View in the starting tab: "search" (default), "browse", "downloaded".
    /// There is no preview here: it opens on a concrete file, and no config can
    /// name one.
    ///
    /// Consulted only on the first run: afterwards the window opens the way it was
    /// arranged last time (see handlers::persist).
    pub initial_view: Option<String>,
}
