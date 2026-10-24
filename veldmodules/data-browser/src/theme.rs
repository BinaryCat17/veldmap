use crate::proto::ui_service::{
    Border, Background, Color, Padding, ProgressBarStyle, Scrollbar, Shadow,
    SliderStyle, TextInputStyle, WidgetStyle, Wrapping, Container, Element, Length, container,
    icon, space, text,
};

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
    pub const OUTLINE: &str = "\u{f0559}";
    pub const FOCUS: &str = "\u{f05b}";
    pub const SPLIT: &str = "\u{f0db}";
    pub const MORE: &str = "\u{f141}";
    pub const CARET: &str = "\u{f0d7}";
    pub const CARET_RIGHT: &str = "\u{f0da}";
    pub const BOX: &str = "\u{f096}";
    pub const BOX_TICKED: &str = "\u{f046}";
    pub const SATELLITE: &str = "\u{f0471}";
    pub const UP: &str = "\u{f062}";
    pub const DOWN: &str = "\u{f063}";
    pub const LEFT: &str = "\u{f053}";
    pub const RIGHT: &str = "\u{f054}";
    pub const TICK: &str = "\u{f00c}";
}

pub const PAGE: Color = Color::rgb8(0xFB, 0xF8, 0xF1);
pub const CHROME: Color = Color::rgb8(0xF1, 0xEA, 0xDA);
pub const SHELF: Color = Color::rgb8(0xF5, 0xEF, 0xE0);
const SURFACE: Color = Color::rgb8(0xFF, 0xFD, 0xF8);
const HOVER: Color = Color::rgb8(0xF1, 0xEA, 0xDA);
const ROW_HOVER: Color = Color::rgb8(0xF7, 0xF2, 0xE5);
const ROW_DIM: Color = Color::rgb8(0xF4, 0xF1, 0xEA);
const ROW_DIM_HOVER: Color = Color::rgb8(0xEE, 0xEA, 0xE1);

pub const LINE: Color = Color::rgb8(0xDC, 0xD3, 0xBF);
pub const LINE_SOFT: Color = Color::rgb8(0xE3, 0xDC, 0xCB);
pub const LINE_ROW: Color = Color::rgb8(0xEF, 0xE8, 0xD8);
pub const LINE_STRONG: Color = Color::rgb8(0xC9, 0xBF, 0xA6);

pub const INK: Color = Color::rgb8(0x2E, 0x2A, 0x22);
pub const INK_SOFT: Color = Color::rgb8(0x3D, 0x38, 0x2C);
pub const INK_MUTED: Color = Color::rgb8(0x5C, 0x56, 0x48);
pub const INK_DIM: Color = Color::rgb8(0x6B, 0x64, 0x55);
pub const INK_FAINT: Color = Color::rgb8(0x94, 0x8B, 0x78);

pub const ACCENT: Color = Color::rgb8(0x4A, 0x7A, 0x3F);
pub const ACCENT_TEXT: Color = Color::rgb8(0x3D, 0x65, 0x34);
pub const DROP_HINT: Color = ACCENT.with_alpha(0.22);
pub const ACCENT_HALF: Color = ACCENT.with_alpha(0.35);
const ACCENT_WASH: Color = Color::rgb8(0xDC, 0xE7, 0xD2);
const ACCENT_WASH_HOVER: Color = Color::rgb8(0xD2, 0xE0, 0xC6);
const ACCENT_TINT: Color = Color::rgb8(0xEE, 0xF3, 0xE7);
const WARN_WASH: Color = Color::rgb8(0xF6, 0xE8, 0xD2);
const WARN_LINE: Color = Color::rgb8(0xE0, 0xC2, 0x92);
const ACCENT_TINT_HOVER: Color = Color::rgb8(0xE5, 0xED, 0xDA);
const ACCENT_LINE: Color = Color::rgb8(0xC3, 0xD6, 0xB4);

pub const WARN: Color = Color::rgb8(0xC4, 0x79, 0x1F);
pub const WARN_TEXT: Color = Color::rgb8(0x8A, 0x62, 0x15);
pub const DANGER: Color = Color::rgb8(0xB4, 0x50, 0x3C);
pub const DANGER_TEXT: Color = Color::rgb8(0x8C, 0x3E, 0x2F);
const TRACK: Color = Color::rgb8(0xE7, 0xDF, 0xCB);

const RADIUS: f32 = 7.0;
const RADIUS_SMALL: f32 = 6.0;
pub const ROW_HEIGHT: f32 = 30.0;
pub const HAIRLINE: f32 = 1.0;
pub const ROW_PITCH: f32 = ROW_HEIGHT + HAIRLINE;
pub const CONTROL_HEIGHT: f32 = 31.0;
pub const ROW_BUTTON: f32 = 23.0;
pub const GUTTER: f32 = 16.0;
pub const BAR_HEIGHT: f32 = 30.0;
pub const HEADING_HEIGHT: f32 = 52.0;

pub const TEXT_TITLE: f32 = 18.0;
pub const TEXT_BODY: f32 = 13.5;
pub const TEXT_LABEL: f32 = 12.5;
pub const TEXT_SMALL: f32 = 12.0;
pub const TEXT_HEADER: f32 = 11.0;
pub const TEXT_MONO: f32 = 12.5;
pub const TEXT_TAG: f32 = 10.0;

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

fn clickable<M>(content: impl Into<Element<M>>, rest: WidgetStyle, hovered: WidgetStyle) -> Container<M> {
    container(content).style(rest).hovered(hovered).center_x().center_y()
}

pub fn tab<M>(content: impl Into<Element<M>>, active: bool) -> Container<M> {
    let border = Border::with_radius(RADIUS);
    let (rest, hovered) = if active {
        (face(PAGE, INK, border), face(PAGE, INK, border))
    } else {
        (face(Color::TRANSPARENT, INK_DIM, border), face(HOVER, INK, border))
    };
    clickable(content, rest, hovered).padding(Padding { top: 4.0, bottom: 4.0, left: 10.0, right: 8.0 })
}

pub fn chrome_icon<M>(content: impl Into<Element<M>>) -> Container<M> {
    clickable(
        content,
        face(Color::TRANSPARENT, INK_DIM, Border::with_radius(RADIUS_SMALL)),
        face(TRACK, INK, Border::with_radius(RADIUS_SMALL)),
    )
    .padding(4.0)
}

pub fn surface_button<M>(content: impl Into<Element<M>>, raised: bool) -> Container<M> {
    let border = outline(if raised { LINE_STRONG } else { LINE }, RADIUS);
    let background = if raised { HOVER } else { SURFACE };
    clickable(content, face(background, INK_MUTED, border), face(HOVER, INK, border))
}

pub fn chrome_bar<M>(content: impl Into<Element<M>>) -> Container<M> {
    container(content)
        .background(CHROME)
        .width(Length::Fill)
        .height(Length::Fixed(BAR_HEIGHT))
        .padding(Padding { top: 0.0, bottom: 0.0, left: GUTTER, right: GUTTER })
}

pub fn spacer<M>() -> Container<M> {
    container(space::<M>(Length::Fill, Length::Fixed(0.0))).width(Length::Fill)
}

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

pub fn row_glyph<M>(glyph: &str, color: Color) -> Element<M> {
    icon::<M>(glyph).size(13.0).color(color).into()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconTone {
    Rest,
    Raised,
    Danger,
    Lit,
    Half,
    Warn,
    Idle,
}

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

pub fn row_check<M>(marked: Option<bool>) -> Container<M> {
    let (glyph, color) = match marked {
        Some(true) => (glyph::BOX_TICKED, ACCENT),
        Some(false) => (glyph::BOX, INK_FAINT),
        None => (glyph::BOX, LINE_STRONG),
    };
    chrome_icon(icon::<M>(glyph).size(12.0).color(color))
        .padding(2.0)
        .width(Length::Fill)
        .height(Length::Fixed(ROW_BUTTON))
}

pub fn bar_button<M>(label: &str) -> Container<M> {
    surface_button(
        text::<M>(label.to_string()).size(TEXT_SMALL).single_line(),
        false,
    )
    .padding(Padding { top: 3.0, bottom: 3.0, left: 8.0, right: 8.0 })
}

pub fn page_button<M>(content: impl Into<Element<M>>, current: bool) -> Container<M> {
    let step = Padding { top: 0.0, bottom: 0.0, left: 7.0, right: 7.0 };
    if !current {
        return surface_button(content, false).padding(step);
    }
    let border = outline(ACCENT_LINE, RADIUS_SMALL);
    clickable(content, face(ACCENT_WASH, INK, border), face(ACCENT_WASH, INK, border)).padding(step)
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum RowTint {
    #[default]
    Plain,
    Dim,
    Marked,
    Picked,
}

fn row_faces(tint: RowTint) -> (Color, Color) {
    match tint {
        RowTint::Plain => (Color::TRANSPARENT, ROW_HOVER),
        RowTint::Dim => (ROW_DIM, ROW_DIM_HOVER),
        RowTint::Marked => (ACCENT_TINT, ACCENT_TINT_HOVER),
        RowTint::Picked => (ACCENT_WASH, ACCENT_WASH_HOVER),
    }
}

pub fn row_button<M>(content: impl Into<Element<M>>, tint: RowTint) -> Container<M> {
    let (rest, hovered) = row_faces(tint);
    clickable(
        content,
        face(rest, INK_SOFT, Border::default()),
        face(hovered, INK_SOFT, Border::default()),
    )
    .padding(Padding { top: 0.0, bottom: 0.0, left: GUTTER, right: GUTTER })
}

pub fn layer_row<M>(content: impl Into<Element<M>>, tint: RowTint) -> Container<M> {
    container(content).background(row_faces(tint).0)
}

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

pub fn crumb<M>(content: impl Into<Element<M>>) -> Container<M> {
    clickable(
        content,
        face(Color::TRANSPARENT, ACCENT, Border::with_radius(4.0)),
        face(HOVER, ACCENT, Border::with_radius(4.0)),
    )
    .padding(Padding { top: 2.0, bottom: 2.0, left: 3.0, right: 3.0 })
}

pub fn control_box() -> WidgetStyle {
    face(SURFACE, INK_DIM, outline(LINE, RADIUS))
}

pub fn panel() -> WidgetStyle {
    WidgetStyle {
        shadow: Some(Shadow { color: Color::rgb8(0x50, 0x42, 0x28).with_alpha(0.18), offset: (0.0, 8.0), blur: 24.0 }),
        ..face(SURFACE, INK, outline(LINE, 8.0))
    }
}

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

pub fn progress(color: Color) -> ProgressBarStyle {
    ProgressBarStyle {
        track: Some(Background::Color(TRACK)),
        bar: Some(Background::Color(color)),
        border: Border::with_radius(3.0),
    }
}

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

pub fn nothing<M>() -> Element<M> {
    space::<M>(Length::Fixed(0.0), Length::Fixed(0.0)).into()
}

pub fn hairline<M>(color: Color) -> Element<M> {
    container(space::<M>(Length::Fill, Length::Fixed(HAIRLINE)))
        .background(color)
        .width(Length::Fill)
        .height(Length::Fixed(HAIRLINE))
        .into()
}

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

pub fn vline<M>(color: Color) -> Element<M> {
    container(space::<M>(Length::Fixed(HAIRLINE), Length::Fill))
        .background(color)
        .width(Length::Fixed(HAIRLINE))
        .height(Length::Fill)
        .into()
}

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
