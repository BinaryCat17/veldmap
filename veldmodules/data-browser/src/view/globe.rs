use veld_ui_service_wrap::{column, row, viewport};

use crate::proto::ui_service::{container, mono, text, Alignment, Element, Length};
use crate::module::components::{self, format};
use crate::module::state::{GlobeState, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};

const HINT: &str = "Drag to rotate, wheel to zoom at the cursor, click to select";

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

const BUTTON_PAD: f32 = 8.0 * 2.0;

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

struct Subject<'a> {
    label: &'a str,
    key: &'a str,
    folder: bool,
}

pub fn view(state: &State, view: ViewId, globe: &GlobeState) -> Element<Msg> {
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

fn caption(state: &State, view: ViewId, globe: &GlobeState) -> Element<Msg> {
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
    if !state.overlays.is_empty() || !state.outlined.is_empty() {
        trailing.push(theme::bar_button("Remove from globe").on_press(Msg::GlobeClear).into());
    }

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
