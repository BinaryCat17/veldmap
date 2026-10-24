//! theme.rs — how the application looks: the palette and the roles.
//!
//! A role, not a decoration: the markup asks for a "tab", "chip", "list row"
//! or "menu panel", and only this file knows how they look. A color written
//! anywhere else in the markup is a bug: the set is finite and lives here.
//!
//! Renderers of foreign surfaces (globe.wgsl) are not markup: the one drawing
//! decides how their content looks, and the contract deliberately has no color
//! for it. They keep their own constants consistent with this palette — when
//! changing it, look there too.

use crate::proto::ui_service::{
    Border, Background, Color, Padding, ProgressBarStyle, Scrollbar, Shadow,
    SliderStyle, TextInputStyle, WidgetStyle, Wrapping, Container, Element, Length, container,
    icon, space, text,
};

// --- Glyphs ---
//
// The icon vocabulary (the Icons font) is one per application: tab, menu and
// table label one entity with one mark, and a changed glyph must not split
// them apart.
//
// The font is Nerd Fonts Symbols, and most marks here come from the Font
// Awesome block (U+F000–F2FF). Those absent from it entirely — satellite,
// crossed eye, layers — come from Material Design Icons: same family, another
// block, hence the five-character codes.
pub mod glyph {
    pub const PLUS: &str = "\u{f067}";
    pub const CLOSE: &str = "\u{f00d}";
    pub const FOLDER: &str = "\u{f07b}";
    pub const SEARCH: &str = "\u{f002}";
    pub const DOWNLOAD: &str = "\u{f019}";
    pub const IMAGE: &str = "\u{f1c5}";
    pub const GLOBE: &str = "\u{f0ac}";
    pub const FILE: &str = "\u{f016}";
    pub const ENTER: &str = "\u{f061}";
    pub const PAUSE: &str = "\u{f04c}";
    pub const PLAY: &str = "\u{f04b}";
    pub const EYE: &str = "\u{f06e}";
    pub const EYE_OFF: &str = "\u{f0209}";
    pub const TRASH: &str = "\u{f1f8}";
    pub const LAYERS: &str = "\u{f0328}";
    /// Scene outline on the globe: a polygon with vertices.
    pub const OUTLINE: &str = "\u{f0559}";
    /// Aim the camera: a crosshair. Not a photo camera, though it is about the
    /// camera: a photo camera would read as a scene in this list, and there are
    /// dozens of those in the column.
    pub const FOCUS: &str = "\u{f05b}";
    /// Screen split: two columns side by side.
    pub const SPLIT: &str = "\u{f0db}";
    pub const MORE: &str = "\u{f141}";
    pub const CARET: &str = "\u{f0d7}";
    /// The same triangle in the closed position. A pair to CARET, not a shared
    /// arrow: expanding and going deeper are different actions, and their marks
    /// differ (going deeper is shown by the ENTER arrow).
    pub const CARET_RIGHT: &str = "\u{f0da}";
    /// Batch-selection box: empty and ticked.
    pub const BOX: &str = "\u{f096}";
    pub const BOX_TICKED: &str = "\u{f046}";
    /// A scene is a logical unit, neither a file nor a folder: the decoder
    /// assembles it (see `RowKind::Product`).
    pub const SATELLITE: &str = "\u{f0471}";
    pub const UP: &str = "\u{f062}";
    pub const DOWN: &str = "\u{f063}";
    pub const LEFT: &str = "\u{f053}";
    pub const RIGHT: &str = "\u{f054}";
    pub const TICK: &str = "\u{f00c}";
}

// --- Palette ---
//
// Three groups: paper (background and surfaces), ink (text) and accents
// (state). Within a group the colors go from quiet to loud.

pub const PAGE: Color = Color::rgb8(0xFB, 0xF8, 0xF1);
/// Chrome: tab strip, table header, footer — everything that is not content.
pub const CHROME: Color = Color::rgb8(0xF1, 0xEA, 0xDA);
/// Backdrop of grouping strips: table header, pagination row.
pub const SHELF: Color = Color::rgb8(0xF5, 0xEF, 0xE0);
/// Surface of a controlled element: button, field, menu panel. These colors go
/// no further than roles — outsiders have no use for them, they ask for a role.
const SURFACE: Color = Color::rgb8(0xFF, 0xFD, 0xF8);
/// Hover — shared by buttons, menu items and chips.
const HOVER: Color = Color::rgb8(0xF1, 0xEA, 0xDA);
/// Hover on a list row: weaker than the button one, there are many rows.
const ROW_HOVER: Color = Color::rgb8(0xF7, 0xF2, 0xE5);
/// Dimmed row: hidden layer, a scene as a bare outline. Not [`SHELF`] — that
/// one draws the header and the line between groups in the same list, and a row
/// filled with it would read as a section heading. This one dims, that one
/// groups.
const ROW_DIM: Color = Color::rgb8(0xF4, 0xF1, 0xEA);
const ROW_DIM_HOVER: Color = Color::rgb8(0xEE, 0xEA, 0xE1);

pub const LINE: Color = Color::rgb8(0xDC, 0xD3, 0xBF);
/// Divider inside content — quieter than a frame.
pub const LINE_SOFT: Color = Color::rgb8(0xE3, 0xDC, 0xCB);
/// Line under a list row: visible only next to a neighboring row.
pub const LINE_ROW: Color = Color::rgb8(0xEF, 0xE8, 0xD8);
/// Frame of a clickable: more noticeable than a divider.
pub const LINE_STRONG: Color = Color::rgb8(0xC9, 0xBF, 0xA6);

pub const INK: Color = Color::rgb8(0x2E, 0x2A, 0x22);
pub const INK_SOFT: Color = Color::rgb8(0x3D, 0x38, 0x2C);
pub const INK_MUTED: Color = Color::rgb8(0x5C, 0x56, 0x48);
pub const INK_DIM: Color = Color::rgb8(0x6B, 0x64, 0x55);
/// Service text: units, format column, counters.
pub const INK_FAINT: Color = Color::rgb8(0x94, 0x8B, 0x78);

pub const ACCENT: Color = Color::rgb8(0x4A, 0x7A, 0x3F);
/// Caption of the "done" state: the same green, but readable as text.
pub const ACCENT_TEXT: Color = Color::rgb8(0x3D, 0x65, 0x34);
/// What fills the place where a dragged tab will drop. Transparent, not solid:
/// what lies underneath stays visible — otherwise the promise would cover what
/// it is about.
pub const DROP_HINT: Color = ACCENT.with_alpha(0.22);
/// A working bar with no share: the same accent at half strength. The whole
/// track is filled with it — there is no other way to say "in progress" — and
/// half strength is the only thing that distinguishes it from a genuinely full
/// bar.
pub const ACCENT_HALF: Color = ACCENT.with_alpha(0.35);
/// Fill of the selected: active pagination page, chosen menu item, highlighted
/// row.
const ACCENT_WASH: Color = Color::rgb8(0xDC, 0xE7, 0xD2);
const ACCENT_WASH_HOVER: Color = Color::rgb8(0xD2, 0xE0, 0xC6);
/// Fill of the marked — the same green at half strength. Deliberately weaker
/// than the selected: one thing on screen is selected, while half a list can be
/// marked, and an equal fill would erase the difference between "this is the
/// main one now" and "these are the ones I picked".
const ACCENT_TINT: Color = Color::rgb8(0xEE, 0xF3, 0xE7);
/// Backdrop of the warning icon and its edge — the same ochre as `WARN`,
/// diluted down to paper. Its own pair, not a diluted accent: this icon stands
/// in a row of greens, and a green tint would read as "almost done".
const WARN_WASH: Color = Color::rgb8(0xF6, 0xE8, 0xD2);
const WARN_LINE: Color = Color::rgb8(0xE0, 0xC2, 0x92);
const ACCENT_TINT_HOVER: Color = Color::rgb8(0xE5, 0xED, 0xDA);
const ACCENT_LINE: Color = Color::rgb8(0xC3, 0xD6, 0xB4);

pub const WARN: Color = Color::rgb8(0xC4, 0x79, 0x1F);
pub const WARN_TEXT: Color = Color::rgb8(0x8A, 0x62, 0x15);
pub const DANGER: Color = Color::rgb8(0xB4, 0x50, 0x3C);
/// Caption of a failure: the same brick, but readable as text — like
/// `ACCENT_TEXT` for done and `WARN_TEXT` for interrupted. The dot and the
/// caption sit side by side, and a caption typed in the dot's color fades on a
/// light background.
pub const DANGER_TEXT: Color = Color::rgb8(0x8C, 0x3E, 0x2F);
/// Track of the progress bar — under any of its colors.
const TRACK: Color = Color::rgb8(0xE7, 0xDF, 0xCB);

// --- Sizes ---
//
// Metrics that more than one place depends on: their consistency is the grid.

/// Corner rounding for everything small: buttons, chips, fields.
const RADIUS: f32 = 7.0;
/// Rounding of the square icon button and menu item — slightly smaller, they
/// are smaller.
const RADIUS_SMALL: f32 = 6.0;
/// Height of a list row. It determines how many rows fit on a page.
pub const ROW_HEIGHT: f32 = 30.0;
/// Thickness of a hairline — everything laid out in strips is separated by it
/// (see [`hairline`]).
pub const HAIRLINE: f32 = 1.0;
/// On-screen pitch of a list row: the row itself plus the line under it. The one
/// scrolling to a row computes with it — and without a shared name it would
/// silently drift from the markup.
pub const ROW_PITCH: f32 = ROW_HEIGHT + HAIRLINE;
/// Height of a control in the toolbar: filter field and chips.
pub const CONTROL_HEIGHT: f32 = 31.0;
/// Side of a square button in a list row.
pub const ROW_BUTTON: f32 = 23.0;
/// Content inset from the screen edges.
pub const GUTTER: f32 = 16.0;
/// Height of a single-line chrome bar: status line, caption under an area. One
/// for all — bars of the same height read as one language.
pub const BAR_HEIGHT: f32 = 30.0;
/// Height of a screen heading: the list title and its caption. Also one for all
/// — headings of different heights read as different screens.
pub const HEADING_HEIGHT: f32 = 52.0;

// --- Text sizes ---

pub const TEXT_TITLE: f32 = 18.0;
pub const TEXT_BODY: f32 = 13.5;
pub const TEXT_LABEL: f32 = 12.5;
pub const TEXT_SMALL: f32 = 12.0;
/// Table header: uppercase and smaller than the rest.
pub const TEXT_HEADER: f32 = 11.0;
/// Monospace in a list row — file name, size.
pub const TEXT_MONO: f32 = 12.5;
/// Format column: quieter and smaller, a label, not a value.
pub const TEXT_TAG: f32 = 10.0;

// --- Style assembly ---

/// A widget's look in one state.
fn face(background: Color, text: Color, border: Border) -> WidgetStyle {
    WidgetStyle {
        background: Some(Background::Color(background)),
        text_color: Some(text),
        border,
        shadow: None,
    }
}

fn outline(color: Color, radius: f32) -> Border {
    Border { color, width: 1.0, radius }
}

/// Shared start of all clickable roles: a box with a rest look and a hover look.
///
/// Content is centered. A button box is almost always larger than its caption —
/// from a fixed size or from a foreign height in a row — and a glyph pinned to
/// its corner reads as a layout bug. For those whose content already stretches
/// across the whole box (a list row, a menu item), centering changes nothing,
/// so no case arises to call it otherwise.
///
/// `pressed` and `disabled` are never named: pressing is visible by what it
/// does, and unavailability is told by the content (a pale arrow at the edge of
/// pagination). Unnamed states degenerate to rest — see `Interaction` in
/// types.proto.
fn clickable<M>(content: impl Into<Element<M>>, rest: WidgetStyle, hovered: WidgetStyle) -> Container<M> {
    container(content).style(rest).hovered(hovered).center_x().center_y()
}

// --- Roles ---

/// A tab. The active one takes the page color, continuing what is shown below;
/// the rest blend into the chrome.
pub fn tab<M>(content: impl Into<Element<M>>, active: bool) -> Container<M> {
    let border = Border::with_radius(RADIUS);
    let (rest, hovered) = if active {
        (face(PAGE, INK, border), face(PAGE, INK, border))
    } else {
        (face(Color::TRANSPARENT, INK_DIM, border), face(HOVER, INK, border))
    };
    clickable(content, rest, hovered).padding(Padding { top: 4.0, bottom: 4.0, left: 10.0, right: 8.0 })
}

/// Icon button on chrome: the "+" on the tab strip, the tab close cross.
pub fn chrome_icon<M>(content: impl Into<Element<M>>) -> Container<M> {
    clickable(
        content,
        face(Color::TRANSPARENT, INK_DIM, Border::with_radius(RADIUS_SMALL)),
        face(TRACK, INK, Border::with_radius(RADIUS_SMALL)),
    )
    .padding(4.0)
}

/// Surface button: filter chip, "up", pagination page, row button.
/// `raised` — an open chip or the chosen page: it stays pressed.
pub fn surface_button<M>(content: impl Into<Element<M>>, raised: bool) -> Container<M> {
    let border = outline(if raised { LINE_STRONG } else { LINE }, RADIUS);
    let background = if raised { HOVER } else { SURFACE };
    clickable(content, face(background, INK_MUTED, border), face(HOVER, INK, border))
}

/// Chrome bar: status line, caption under an area. A role, not a local
/// assembly: there are two such bars, and written separately they drift apart
/// in gap and inset.
pub fn chrome_bar<M>(content: impl Into<Element<M>>) -> Container<M> {
    container(content)
        .background(CHROME)
        .width(Length::Fill)
        .height(Length::Fixed(BAR_HEIGHT))
        .padding(Padding { top: 0.0, bottom: 0.0, left: GUTTER, right: GUTTER })
}

/// Spacer pushing neighbors to the edges. Written as a single role: `Fill`
/// inside `Shrink` collapses to zero (see README about lacking space), and a
/// wrapper that prevents this is needed at every such place.
pub fn spacer<M>() -> Container<M> {
    container(space::<M>(Length::Fill, Length::Fixed(0.0))).width(Length::Fill)
}

/// A place with nothing to show, and a short phrase saying why. One role for all
/// empty states: they say the same thing and must look the same — otherwise an
/// empty list and an empty half read as different misfortunes.
///
/// Failures land here too: the reason is the same short phrase in place of
/// content. It arrives from anywhere and may contain no spaces at all — a path,
/// a URL, a key. Hence glyph-level wrapping: the default breaks lines only at
/// spaces, and such a reason would run past the allotted space (see `Wrapping`
/// in types.proto).
pub fn empty<M>(what: &str) -> Container<M> {
    container(
        text::<M>(what.to_string())
            .size(TEXT_BODY)
            .color(INK_DIM)
            .wrapping(Wrapping::WrapWordOrGlyph),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x()
    .center_y()
    .padding(Padding::new(20.0))
}

/// Icon of a list row. The size is assigned here: the row is the same in the
/// table and in the layer list, and different sizes would read as different
/// things.
pub fn row_glyph<M>(glyph: &str, color: Color) -> Element<M> {
    icon::<M>(glyph).size(13.0).color(color).into()
}

/// With which face a square icon button stands.
///
/// An enum, not a set of flags: the faces say different things. "Stays pressed"
/// is about the lever itself (an expanded menu, an enabled mode), "lit" is about
/// the world: what the button is about is already done and visible on the globe,
/// "dangerous" is about the consequence. Folding the first two into one bool
/// would give an icon that on a scene lying on the globe looks like an expanded
/// menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconTone {
    Rest,
    /// Stays pressed: an expanded menu, an enabled mode.
    Raised,
    /// Irreversible — deletion. The same brick as the "Delete" menu item
    /// (see [`menu_item`]): the same action named with different colors would
    /// read as two different ones. At rest in the muted tone ([`DANGER_TEXT`]) —
    /// for the same reason every caption in this color is muted — and full under
    /// the cursor.
    Danger,
    /// Lit: the scene lies on the globe and is visible.
    Lit,
    /// Lit at half strength: done, but not visible right now — the layer is
    /// still assembling or hidden, the outline is still coming from the
    /// catalogue or will not arrive at all. The same green as
    /// [`IconTone::Lit`], filled weaker: a state of the same kind, and speaking
    /// of it in another color would mean a second vocabulary.
    Half,
    /// Tried, and it failed: the icon is clickable — the attempt could break
    /// over the network — but it glows with a warning, and the reason stands by
    /// as a tooltip.
    ///
    /// Its own color, not a disabled look: "disabled" says "no point in trying",
    /// and saying that about what may well succeed on the second try would take
    /// an action away from the person. And not rest either: a rest icon invites
    /// a click promising nothing — while here there is something to say before
    /// clicking.
    Warn,
    /// Disabled: there is nothing to press, but the icon keeps its place. The
    /// same mark and the same box, but in the line color, not ink — for the
    /// same reason the unsettable check box differs (see [`row_check`]): a
    /// full-strength icon promises a click everywhere it stands.
    Idle,
}

/// Square button with an icon in a list row: the main action, "on the globe",
/// menu. A role, not a local assembly — both the table and the layer list build
/// it, and once they drift, two buttons that look alike read as two different
/// things.
///
/// The size is assigned here: it is a property of the role, not of whoever
/// places it.
pub fn row_button_icon<M>(glyph: &str, tone: IconTone) -> Container<M> {
    let lit = |fill: Color, ink: Color| {
        let border = outline(ACCENT_LINE, RADIUS);
        (face(fill, ink, border), face(ACCENT_WASH, ACCENT_TEXT, border))
    };
    let (rest, hovered) = match tone {
        IconTone::Danger => {
            let border = outline(LINE, RADIUS);
            (face(SURFACE, DANGER_TEXT, border), face(HOVER, DANGER, border))
        }
        IconTone::Rest | IconTone::Raised => {
            let raised = tone == IconTone::Raised;
            let border = outline(if raised { LINE_STRONG } else { LINE }, RADIUS);
            let background = if raised { HOVER } else { SURFACE };
            (face(background, INK_SOFT, border), face(HOVER, INK, border))
        }
        IconTone::Lit => lit(ACCENT_WASH, ACCENT_TEXT),
        IconTone::Half => lit(ACCENT_TINT, ACCENT_TEXT),
        IconTone::Warn => {
            let border = outline(WARN_LINE, RADIUS);
            (face(WARN_WASH, WARN_TEXT, border), face(WARN_WASH, WARN, border))
        }
        // Same under the cursor: a button that reacts to hover but not to a
        // click promises more than it does.
        IconTone::Idle => {
            let face = face(SURFACE, LINE_STRONG, outline(LINE_SOFT, RADIUS));
            (face.clone(), face)
        }
    };
    let ink = rest.text_color.unwrap_or(INK_SOFT);
    clickable(icon::<M>(glyph).size(11.0).color(ink), rest, hovered)
        .width(Length::Fixed(ROW_BUTTON))
        .height(Length::Fixed(ROW_BUTTON))
}

/// Batch-selection box: in a list row and in its header. A role, not a local
/// assembly — it stands in two places, and once they drift, it would read as
/// two different things.
///
/// It fills its whole column: the column is narrow (see `table::CHECK`) and has
/// no padding — otherwise only a few dots of the box would remain to hit.
/// Check box. `None` — nothing to mark: drawn with the same mark, but in the
/// line color, not ink. The difference is needed because it is not clickable
/// everywhere, while a full-strength box promises a click everywhere it stands.
pub fn row_check<M>(marked: Option<bool>) -> Container<M> {
    let (glyph, color) = match marked {
        Some(true) => (glyph::BOX_TICKED, ACCENT),
        Some(false) => (glyph::BOX, INK_FAINT),
        None => (glyph::BOX, LINE_STRONG),
    };
    chrome_icon(icon::<M>(glyph).size(12.0).color(color))
        // Padding narrower than usual: the column is narrow, and four dots on
        // each side would cut into the mark itself.
        .padding(2.0)
        .width(Length::Fill)
        .height(Length::Fixed(ROW_BUTTON))
}

/// Button with a caption in a chrome bar: under the globe area, in the layer
/// list heading. Also a role: bars of the same height read as one language, and
/// so do the buttons in them.
pub fn bar_button<M>(label: &str) -> Container<M> {
    surface_button(
        text::<M>(label.to_string()).size(TEXT_SMALL).single_line(),
        false,
    )
    .padding(Padding { top: 3.0, bottom: 3.0, left: 8.0, right: 8.0 })
}

/// The current pagination page — the only button with an accent fill.
pub fn page_button<M>(content: impl Into<Element<M>>, current: bool) -> Container<M> {
    let step = Padding { top: 0.0, bottom: 0.0, left: 7.0, right: 7.0 };
    if !current {
        return surface_button(content, false).padding(step);
    }
    let border = outline(ACCENT_LINE, RADIUS_SMALL);
    clickable(content, face(ACCENT_WASH, INK, border), face(ACCENT_WASH, INK, border)).padding(step)
}

/// How a row is set apart from its neighbors.
///
/// An enum, not a set of flags, and here is why: the renderer picks the box
/// state whole, not mixing it into rest (see `Interaction` in
/// ui-service/types.proto), so "marked and under the cursor" cannot be
/// expressed by the protocol — whoever names the color must fold them into one
/// color. Since the folding happens here anyway, the precedence is named here
/// too: the highlight is one per screen and outranks the marks, of which a list
/// can hold dozens.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum RowTint {
    #[default]
    Plain,
    /// The layer is hidden: it is on the globe but not visible. Not "disabled" —
    /// everything is still done to it, it is simply not visible right now.
    Dim,
    /// Ticked with the box — gathered for a batch action. The fill says nothing
    /// about the globe: outline and display are shown by the row icons.
    Marked,
    /// The main one now: the scene selected on the globe or the row a jump led
    /// to (see `state::Highlight`).
    Picked,
}

/// A "rest — under cursor" pair for row fill.
///
/// Hover is derived from rest, not taken shared: the shared ROW_HOVER is
/// lighter than the accent fill, and a highlighted row would lose its highlight
/// under the cursor — from the outside it looks like "the selection is knocked
/// off by hovering".
fn row_faces(tint: RowTint) -> (Color, Color) {
    match tint {
        RowTint::Plain => (Color::TRANSPARENT, ROW_HOVER),
        RowTint::Dim => (ROW_DIM, ROW_DIM_HOVER),
        RowTint::Marked => (ACCENT_TINT, ACCENT_TINT_HOVER),
        RowTint::Picked => (ACCENT_WASH, ACCENT_WASH_HOVER),
    }
}

/// A list row. A plain one has no background of its own — it lies on the page;
/// it is visible only under the cursor and by the line below.
pub fn row_button<M>(content: impl Into<Element<M>>, tint: RowTint) -> Container<M> {
    let (rest, hovered) = row_faces(tint);
    clickable(
        content,
        face(rest, INK_SOFT, Border::default()),
        face(hovered, INK_SOFT, Border::default()),
    )
    .padding(Padding { top: 0.0, bottom: 0.0, left: GUTTER, right: GUTTER })
}

/// A layer list row — the same fill, but not a button: the opacity slider lies
/// inside the row, and a clickable wrapper around it would take dragging away
/// from it.
pub fn layer_row<M>(content: impl Into<Element<M>>, tint: RowTint) -> Container<M> {
    container(content).background(row_faces(tint).0)
}

/// A dropdown menu item. `danger` — deletion: only it is signed with color —
/// here and on the batch button (see [`IconTone::Danger`]) — because only it is
/// irreversible.
pub fn menu_item<M>(content: impl Into<Element<M>>, selected: bool, danger: bool) -> Container<M> {
    let text = if danger { DANGER } else { INK };
    let background = if selected { HOVER } else { Color::TRANSPARENT };
    clickable(
        content,
        face(background, text, Border::with_radius(RADIUS_SMALL)),
        face(HOVER, text, Border::with_radius(RADIUS_SMALL)),
    )
    .padding(Padding { top: 6.0, bottom: 6.0, left: 9.0, right: 9.0 })
}

/// A path segment. Clickable, but not looking like a button: a frame and
/// background on every segment would turn the path line into a row of buttons,
/// while it is one line.
pub fn crumb<M>(content: impl Into<Element<M>>) -> Container<M> {
    clickable(
        content,
        face(Color::TRANSPARENT, ACCENT, Border::with_radius(4.0)),
        face(HOVER, ACCENT, Border::with_radius(4.0)),
    )
    .padding(Padding { top: 2.0, bottom: 2.0, left: 3.0, right: 3.0 })
}

/// Box of a control: filter field, path line. The same surface as the buttons
/// nearby — but there is nothing to press, so it is not a button.
pub fn control_box() -> WidgetStyle {
    face(SURFACE, INK_DIM, outline(LINE, RADIUS))
}

/// A popup panel: menus, dropdowns and icon tooltips. The shadow separates it
/// from what it hangs over — without it the panel reads as part of the page.
/// The tooltip is not here by oversight: it hangs the same way and must be
/// separated by the same means, otherwise two popups over one list would read
/// as things of different kinds.
pub fn panel() -> WidgetStyle {
    WidgetStyle {
        shadow: Some(Shadow { color: Color::rgb8(0x50, 0x42, 0x28).with_alpha(0.18), offset: (0.0, 8.0), blur: 24.0 }),
        ..face(SURFACE, INK, outline(LINE, 8.0))
    }
}

/// Input field inside a box: the box provides the frame and background, so the
/// field itself has none — otherwise the frame would be drawn twice, one inside
/// the other.
pub fn field() -> TextInputStyle {
    let bare = face(Color::TRANSPARENT, INK, Border::default());
    TextInputStyle {
        focused: bare.clone(),
        hovered: bare.clone(),
        active: bare,
        placeholder: INK_DIM,
        selection: ACCENT_WASH,
    }
}

pub fn scrollbar() -> Scrollbar {
    Scrollbar {
        width: 8.0,
        scroller_width: 6.0,
        margin: 2.0,
        rail: Some(Background::Color(Color::TRANSPARENT)),
        scroller: Some(Background::Color(LINE)),
        radius: 3.0,
    }
}

/// Progress bar: the color says the same as the caption beside it — running or
/// interrupted.
pub fn progress(color: Color) -> ProgressBarStyle {
    ProgressBarStyle {
        track: Some(Background::Color(TRACK)),
        bar: Some(Background::Color(color)),
        border: Border::with_radius(3.0),
    }
}

/// Slider: a track that looks like the progress bar's — they stand in one list
/// under each other — and a round handle on it. It differs not by the track's
/// look but by the handle being dragged.
pub fn slider() -> SliderStyle {
    SliderStyle {
        filled: Some(Background::Color(ACCENT)),
        rest: Some(Background::Color(TRACK)),
        rail_width: 5.0,
        rail_border: Border::with_radius(3.0),
        handle: Some(Background::Color(SURFACE)),
        handle_radius: 6.0,
        handle_border_width: 1.5,
        handle_border_color: ACCENT,
    }
}

// --- Ready-made bits ---

/// A zero-size empty place — it fills what there is nothing to say about: a
/// valueless cell in the table grid, the mark slot of a menu item without one.
/// The place stays with it, so neighbors do not shift.
pub fn nothing<M>() -> Element<M> {
    space::<M>(Length::Fixed(0.0), Length::Fixed(0.0)).into()
}

/// A full-width hairline: chrome strips and list rows are separated by it. A
/// separate widget, not a border: a border in this protocol is one for all four
/// sides, while only one is needed.
pub fn hairline<M>(color: Color) -> Element<M> {
    container(space::<M>(Length::Fill, Length::Fixed(HAIRLINE)))
        .background(color)
        .width(Length::Fill)
        .height(Length::Fixed(HAIRLINE))
        .into()
}

/// Boundary between panels: the same hairline, but grabbed with the mouse.
///
/// The grip strip is wider than the line: the cursor cannot hit a single pixel,
/// and the boundary is the only thing panels measure space with. It highlights
/// under the cursor — the cursor does not change in this protocol, and there is
/// nothing else to say "you can drag here".
pub fn divider<M: veld_ui_service_wrap::UiMessage>(
    axis: crate::proto::ui_service::DividerAxis,
    on_drag: impl FnOnce(f32) -> M,
    on_release: M,
) -> veld_ui_service_wrap::Divider<M> {
    const GRIP: f32 = 7.0;
    veld_ui_service_wrap::divider(axis, on_drag)
        .on_release(on_release)
        .thickness(GRIP)
        .line(HAIRLINE)
        .color(LINE, ACCENT)
}

/// The same line across — a divider inside a row.
pub fn vline<M>(color: Color) -> Element<M> {
    container(space::<M>(Length::Fixed(HAIRLINE), Length::Fill))
        .background(color)
        .width(Length::Fixed(HAIRLINE))
        .height(Length::Fill)
        .into()
}

/// Status dot: the color says the same as the caption — but is seen before it
/// is read.
pub fn dot<M>(color: Color) -> Container<M> {
    const SIZE: f32 = 7.0;
    container(space::<M>(Length::Fixed(SIZE), Length::Fixed(SIZE)))
        .style(WidgetStyle {
            background: Some(Background::Color(color)),
            border: Border::with_radius(SIZE / 2.0),
            ..Default::default()
        })
        .width(Length::Fixed(SIZE))
        .height(Length::Fixed(SIZE))
}
