//! The globe tab: an area across the whole screen and a caption under it.
//!
//! The tab has almost no content of its own — everything visible is drawn by
//! another module. Here is only the place for it and what is around.

use veld_ui_service_wrap::{column, row, viewport};

use crate::proto::ui_service::{container, mono, text, Alignment, Element, Length};
use crate::module::components::{self, format};
use crate::module::state::{GlobeState, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};


/// How the globe is controlled. A constant, not a string at the place of use:
/// [`hint_fits`] measures its width, and the two must not diverge.
const HINT: &str = "Drag to rotate, wheel to zoom at the cursor, click to select";

/// Whether the hint fits into the bar together with everything that outranks
/// it.
///
/// Outranking it are the area size and the "Remove from globe" button: the
/// first tells where one looks, the second does the work, while the hint only
/// recalls how the globe is rotated. The rule is the same as for the scene name
/// ([`picked_chars`]): the bar gives the buttons their room first.
fn hint_fits(width: f32, clearable: bool) -> bool {
    let mut taken = format::text_width(HINT, theme::TEXT_LABEL)
        + format::text_width("viewport not sized yet", theme::TEXT_LABEL)
        + crate::module::view::BAR_SPACING
        + theme::GUTTER * 2.0;
    if clearable {
        taken += format::text_width("Remove from globe", theme::TEXT_SMALL)
            + BUTTON_PAD
            + crate::module::view::BAR_SPACING;
    }
    taken <= width
}

/// The button padding on both sides — the same that `theme::bar_button` sets.
const BUTTON_PAD: f32 = 8.0 * 2.0;

/// How many characters of the name fit into a bar of width `width`.
///
/// Computed, not assigned as a number: the client shortens the name
/// (`format::ellipsize`) — this protocol has no way to tell a text to "shrink"
/// — and named with a spare it would push the button past the edge, the only
/// thing this bar is looked at for. A constant number cannot express it: the
/// bar can be whole-screen or half.
///
/// The buttons are measured by their labels — they have no width of their own,
/// they are their labels — and all three are counted even when not all are
/// shown: a spare margin shortens the name, a shortage pushes the button out.
fn picked_chars(width: f32) -> usize {
    const LABELS: [&str; 3] = ["View", "In catalogue", "Remove from globe"];
    let buttons: f32 = LABELS
        .iter()
        .map(|label| {
            format::text_width(label, theme::TEXT_SMALL)
                + BUTTON_PAD
                + crate::module::view::BAR_SPACING
        })
        .sum();
    format::mono_fit((width - buttons - theme::GUTTER * 2.0).max(0.0), theme::TEXT_SMALL)
}

/// The scene the bar talks about: what to sign it with, what to address it by,
/// and which way to view it. Three fields, not a tuple: the name and the key of
/// a scene look alike, and nothing keeps one from swapping them in a tuple.
struct Subject<'a> {
    label: &'a str,
    key: &'a str,
    folder: bool,
}

pub fn view(state: &State, view: ViewId, globe: &GlobeState) -> Element<Msg> {
    // The area gets its texture from us: we allocated it in answer to the
    // previous on_resized. On the first frame it is not there yet — the place is
    // taken empty, and that is fine: it arrives with the next event.
    let mut area = viewport::<Msg>()
        .width(Length::Fill)
        .height(Length::Fill)
        .on_resized(move |size| Msg::In(view, ViewMsg::GlobeResized(size)))
        .on_pointer(move |pointer| Msg::In(view, ViewMsg::GlobePointer(pointer)));
    if let Some(surface) = &globe.surface {
        area = area.texture(surface.handle());
    }

    column![
        container(area).width(Length::Fill).height(Length::Fill),
        theme::hairline(theme::LINE),
        caption(state, view, globe),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// The bar under the area: what controls the globe on the left, what is on the
/// globe on the right.
///
/// A named scene displaces the area size: while nothing is selected or laid
/// over, the size is the only thing one can say about an unnamed area, and once
/// a scene is named, it is the answer to the question "what am I looking at".
/// Next to the name goes what is done with it while looking at the globe: view
/// the scene itself and find it in the catalogue. Overlays are "removed" all at
/// once; one by one they are removed in "Shown", here is the button for the
/// case when that tab is not open.
fn caption(state: &State, view: ViewId, globe: &GlobeState) -> Element<Msg> {
    // What to name: the outline picked by a click, and without one — the top
    // layer on the globe. Usually it is one and the same scene; they diverge
    // when a neighboring outline was clicked, and then the name answers with the
    // pick — it is fresher.
    //
    // The topmost of those visible: there can be several layers, only one can be
    // named in a single line, and one still gathering or hidden would name what
    // is not on the globe.
    // The picked one is sought among the layers too: a scene may have no outline
    // at all — the catalogue knows geometry not about everything, and the mark
    // could have been taken off it — and then there is nothing left to name it
    // by. A silent fallback to the top layer is worst here: the bar names not
    // the scene that was just pointed at, and its buttons lead away from it too.
    let picked = state.picked_key();
    let layer_named = |key: &str| {
        state.overlays.iter().find(move |overlay| overlay.identifier == key).map(|overlay| {
            Subject { label: &overlay.label, key: &overlay.identifier, folder: overlay.folder }
        })
    };
    let subject: Option<Subject<'_>> = state
        .picked()
        .map(|outlined| Subject {
            label: &outlined.label,
            key: &outlined.key,
            folder: outlined.folder,
        })
        .or_else(|| layer_named(picked))
        .or_else(|| {
            state
                .overlays
                .iter()
                .rev()
                .find(|overlay| overlay.on_globe() && !overlay.hidden)
                .map(|overlay| Subject {
                    label: &overlay.label,
                    key: &overlay.identifier,
                    folder: overlay.folder,
                })
        });

    let named = subject.is_some();
    let mut trailing: Vec<Element<Msg>> = Vec::new();
    match subject {
        Some(subject) => {
            // The name takes everything left of the bar: the layout gives the
            // buttons their room first, and no name will push them past the
            // edge.
            trailing.push(
                container(
                    mono::<Msg>(format::ellipsize(subject.label, picked_chars(state.pane_width(view))))
                        .size(theme::TEXT_SMALL)
                        .color(theme::INK_SOFT)
                        .single_line(),
                )
                .width(Length::Fill)
                .align_x(Alignment::End)
                .into(),
            );
            trailing.push(
                theme::bar_button("View")
                    .on_press(Msg::In(
                        view,
                        components::preview_of(&state.library, subject.key, subject.folder),
                    ))
                    .into(),
            );
            trailing.push(
                theme::bar_button("In catalogue")
                    .on_press(Msg::In(view, ViewMsg::InCatalog(subject.key.to_string())))
                    .into(),
            );
        }
        None => {
            let label = match &globe.surface {
                Some(surface) => format!("{}×{}", surface.width, surface.height),
                None => "viewport not sized yet".to_string(),
            };
            trailing.push(
                text::<Msg>(label).size(theme::TEXT_LABEL).color(theme::INK_FAINT).single_line().into(),
            );
        }
    }
    // Outlines count on par with rasters: an outlined globe too has "something
    // lying on it", and it is to be taken off with the same lever.
    if !state.overlays.is_empty() || !state.outlined.is_empty() {
        trailing.push(theme::bar_button("Remove from globe").on_press(Msg::GlobeClear).into());
    }

    // The hint yields its place to the named scene — as does the area size
    // above: what is done with the scene outranks how the globe is rotated. It
    // yields to the button too if the two do not fit into the bar. It yields
    // entirely, not by shrinking: a stub "Drag to rot…" teaches nothing and
    // still takes room from the only thing this bar is looked at for.
    let mut parts: Vec<Element<Msg>> = Vec::new();
    if !named {
        if hint_fits(state.pane_width(view), trailing.len() > 1) {
            parts.push(
                text::<Msg>(HINT.to_string())
                    .size(theme::TEXT_LABEL)
                    .color(theme::INK_DIM)
                    .single_line()
                    .into(),
            );
        }
        // The spacer is needed only here: with a named scene the rest of the bar
        // is taken by its name, and a second stretching element would take away
        // half of its place.
        parts.push(theme::spacer().into());
    }
    parts.extend(trailing);

    theme::chrome_bar(
        row(parts)
            .spacing(crate::module::view::BAR_SPACING)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_items(Alignment::Center),
    )
    .into()
}
