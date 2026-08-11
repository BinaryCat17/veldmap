use crate::proto::ui_service as proto;
use crate::module::renderer::GpuRenderer;
use iced_widget::{column, row, text, button, container, scrollable, progress_bar, stack, tooltip, Space};
use iced_core::{Element, Theme, Length, Color, alignment, Size, Font, font::Family};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct UiMessage {
    pub method: String,
    pub value: String,
}

/// Все дети одного рода — условие, при котором колонку можно диффить по
/// ключам (см. вызов). Сравнивается именно вид виджета: у одного рода детей
/// подменить состояние нечем.
fn same_kind(children: &[proto::Widget]) -> bool {
    let kind = |widget: &proto::Widget| widget.r#type.as_ref().map(std::mem::discriminant);
    let Some(first) = children.first().map(kind) else { return true };
    children.iter().all(|child| kind(child) == first)
}

/// Ключ ребёнка для keyed-колонки. `keyed::Column` требует `Copy + PartialEq`,
/// поэтому имя из разметки сворачивается в число.
///
/// Безымянный ребёнок получает ключ от своей позиции — то есть ровно прежнее
/// поведение для него одного. Позиционные ключи солятся, чтобы не совпасть с
/// хэшем строки: совпадение означало бы, что состояние достаётся чужому.
fn child_key(widget: &proto::Widget, index: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if widget.key.is_empty() {
        "\u{0}position".hash(&mut hasher);
        index.hash(&mut hasher);
    } else {
        widget.key.hash(&mut hasher);
    }
    hasher.finish()
}

/// `iced_core::Font::family` требует `&'static str`, а логическое имя шрифта
/// приходит рантаймовой строкой из proto (через wasm ABI). Каждое различное
/// имя один раз "утекает" в статическую память и переиспользуется — набор
/// имён на практике фиксирован (задан в коде плагинов), так что утечка ограничена.
fn intern_font_family(name: &str) -> &'static str {
    static INTERNED: Mutex<Option<HashMap<String, &'static str>>> = Mutex::new(None);
    let mut guard = INTERNED.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(existing) = map.get(name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    map.insert(name.to_string(), leaked);
    leaked
}

pub fn convert_layout(layout: &proto::Layout) -> Element<'static, UiMessage, Theme, GpuRenderer> {
    if let Some(root) = &layout.root {
        convert_widget(root)
    } else {
        column([]).into()
    }
}

fn convert_widget(widget: &proto::Widget) -> Element<'static, UiMessage, Theme, GpuRenderer> {
    match &widget.r#type {
        Some(proto::widget::Type::Column(c)) => {
            // Колонка с именованными детьми сопоставляет состояние по имени, а
            // не по позиции (см. Widget.key в types.proto). Разные виджеты на
            // оба случая, потому что keyed-вариант в iced отдельный.
            //
            // Однородность детей — условие безопасности, а не придирка:
            // keyed-колонка диффит их без проверки типа, и виджет, встав на
            // место виджета другого рода, получит его состояние и уронит
            // downcast. Обычная колонка такой подменой не смущается, поэтому
            // разнородный список ей и достаётся.
            if c.children.iter().any(|child| !child.key.is_empty()) && same_kind(&c.children) {
                let keys = c.children.iter().enumerate()
                    .map(|(index, child)| child_key(child, index))
                    .collect();
                let children = c.children.iter().map(convert_widget).collect();
                let mut col = iced_widget::keyed::Column::from_vecs(keys, children)
                    .spacing(c.spacing)
                    .padding(convert_padding(&c.padding));

                if let Some(align) = convert_alignment(c.align_items()) {
                    col = col.align_items(align);
                }
                return col.width(convert_length(&c.width))
                          .height(convert_length(&c.height))
                          .into();
            }

            let mut col = column(c.children.iter().map(convert_widget))
                .spacing(c.spacing)
                .padding(convert_padding(&c.padding));

            if let Some(align) = convert_alignment(c.align_items()) {
                col = col.align_x(align);
            }
            col.width(convert_length(&c.width))
               .height(convert_length(&c.height))
               .into()
        }
        Some(proto::widget::Type::Row(r)) => {
            let mut rw = row(r.children.iter().map(convert_widget))
                .spacing(r.spacing)
                .padding(convert_padding(&r.padding));
            
            if let Some(align) = convert_alignment(r.align_items()) {
                rw = rw.align_y(align);
            }
            rw.width(convert_length(&r.width))
              .height(convert_length(&r.height))
              .into()
        }
        Some(proto::widget::Type::Text(t)) => {
            let mut size = t.size;
            if size <= 0.0 {
                veldsdk::log::error!(target: "handlers", "Text widget has invalid size: {}. Resetting to 16.0", size);
                size = 16.0;
            }
            let mut txt = text(t.content.clone())
                .size(size)
                .width(convert_length(&t.width))
                .height(convert_length(&t.height))
                .align_x(convert_horizontal_alignment(t.horizontal_alignment()))
                .align_y(convert_vertical_alignment(t.vertical_alignment()))
                .shaping(match t.shaping() {
                    proto::Shaping::ShapeAdvanced => iced_core::text::Shaping::Advanced,
                    proto::Shaping::ShapeBasic => iced_core::text::Shaping::Basic,
                })
                .wrapping(match t.wrapping() {
                    proto::Wrapping::NoWrap => iced_core::text::Wrapping::None,
                    proto::Wrapping::WrapGlyph => iced_core::text::Wrapping::Glyph,
                    proto::Wrapping::WrapWordOrGlyph => iced_core::text::Wrapping::WordOrGlyph,
                    proto::Wrapping::WrapWord => iced_core::text::Wrapping::Word,
                });

            if let Some(color) = &t.color {
                txt = txt.color(convert_color_value(color));
            }
            txt = txt.font(convert_font(&t.font_family, t.weight()));
            txt.into()
        }
        Some(proto::widget::Type::Button(b)) => {
            let content = if let Some(child) = &b.child {
                convert_widget(child)
            } else {
                iced_widget::Space::new().into()
            };

            let mut btn = button(content)
                .width(convert_length(&b.width))
                .height(convert_length(&b.height))
                .padding(convert_padding(&b.padding));
            
            // Нажимаемость — это наличие обработчика: iced сам рисует кнопку
            // без него состоянием Disabled.
            if let Some(h) = &b.on_press {
                if !h.method.is_empty() {
                    btn = btn.on_press(UiMessage { method: h.method.clone(), value: h.value.clone() });
                }
            }

            match &b.style {
                Some(style) => {
                    let active = style.active.clone().unwrap_or_default();
                    let hovered = style.hovered.clone().unwrap_or_default();
                    let pressed = style.pressed.clone().unwrap_or_default();
                    let disabled = style.disabled.clone().unwrap_or_default();

                    // Перевод в стиль iced идёт внутри замыкания, а не рядом:
                    // цвет текста в стиле может быть не задан, и тогда его
                    // берёт тема, — а тему видно только здесь.
                    btn = btn.style(move |theme: &Theme, status| {
                         let style = match status {
                             iced_widget::button::Status::Active => &active,
                             iced_widget::button::Status::Hovered => &hovered,
                             iced_widget::button::Status::Pressed => &pressed,
                             iced_widget::button::Status::Disabled => &disabled,
                         };
                         convert_button_style(style, theme.palette().text)
                    });
                }
                // Стиль не задан — рисуем темой.
                None => btn = btn.style(iced_widget::button::primary),
            }

            btn.into()
        }
        Some(proto::widget::Type::TextInput(t)) => {
            let mut size = t.size;
            if size <= 0.0 {
                veldsdk::log::error!(target: "handlers", "TextInput widget has invalid size: {}. Resetting to 16.0", size);
                size = 16.0;
            }
            let mut input = iced_widget::text_input(&t.placeholder, &t.value)
                .width(convert_length(&t.width))
                .padding(convert_padding(&t.padding))
                .font(convert_font(&t.font_family, proto::FontWeight::WeightNormal))
                .size(size);

            if let Some(style) = &t.style {
                let active = style.active.clone().unwrap_or_default();
                let hovered = style.hovered.clone().unwrap_or_default();
                let focused = style.focused.clone().unwrap_or_default();
                let placeholder = convert_color(&style.placeholder);
                let selection = convert_color(&style.selection);

                input = input.style(move |theme: &Theme, status| {
                    let state = match status {
                        iced_widget::text_input::Status::Active => &active,
                        iced_widget::text_input::Status::Hovered => &hovered,
                        iced_widget::text_input::Status::Focused { .. } => &focused,
                        // Выключенным поле не бывает: без `on_input` оно
                        // только для чтения, и рисовать его иначе не за что.
                        iced_widget::text_input::Status::Disabled => &active,
                    };
                    let base = convert_widget_style(state);
                    let value = base.text_color.unwrap_or(theme.palette().text);
                    iced_widget::text_input::Style {
                        background: base.background.unwrap_or(iced_core::Background::Color(Color::TRANSPARENT)),
                        border: base.border,
                        // Иконка полю не задаётся ни разу — своей она рисуется
                        // цветом подсказки, как и placeholder.
                        icon: placeholder,
                        placeholder,
                        value,
                        selection,
                    }
                });
            }

            if let Some(h) = &t.on_input {
                if !h.method.is_empty() {
                    let method = h.method.clone();
                    // Значение подставит рендерер — набранный текст; из
                    // разметки едет только имя метода.
                    input = input.on_input(move |v| UiMessage { method: method.clone(), value: v });
                }
            }

            if let Some(h) = &t.on_submit {
                if !h.method.is_empty() {
                    input = input.on_submit(UiMessage { method: h.method.clone(), value: h.value.clone() });
                }
            }

            input.into()
        }
        Some(proto::widget::Type::Container(c)) => {
            let mut cont = container(if let Some(child) = &c.child { convert_widget(child) } else { iced_widget::Space::new().into() })
                .padding(convert_padding(&c.padding))
                .width(convert_length(&c.width))
                .height(convert_length(&c.height))
                .max_width(convert_length_val(&c.max_width))
                .max_height(convert_length_val(&c.max_height))
                .clip(c.clip);

            if let Some(ax) = convert_alignment(c.align_x()) { cont = cont.align_x(ax); }
            if let Some(ay) = convert_alignment(c.align_y()) { cont = cont.align_y(ay); }

            if let Some(style) = &c.style {
                let style = convert_widget_style(style);
                cont = cont.style(move |_theme: &Theme| style);
            }

            cont.into()
        }
        Some(proto::widget::Type::Scrollable(s)) => {
            let content = if let Some(child) = &s.content { convert_widget(child) } else { Space::new().into() };
            let mut bar = iced_widget::scrollable::Scrollbar::default();
            if let Some(geometry) = &s.scrollbar {
                if geometry.width > 0.0 {
                    bar = bar.width(geometry.width);
                }
                if geometry.scroller_width > 0.0 {
                    bar = bar.scroller_width(geometry.scroller_width);
                }
                bar = bar.margin(geometry.margin);
            }
            let direction = match s.direction() {
                proto::ScrollDirection::ScrollHorizontal => iced_widget::scrollable::Direction::Horizontal(bar),
                proto::ScrollDirection::ScrollBoth => iced_widget::scrollable::Direction::Both { vertical: bar, horizontal: bar },
                proto::ScrollDirection::ScrollVertical => iced_widget::scrollable::Direction::Vertical(bar),
            };
            let mut scroll = scrollable(content)
                .direction(direction)
                .width(convert_length(&s.width))
                .height(convert_length(&s.height));

            if let Some(look) = &s.scrollbar {
                let rail = convert_rail(look);
                // Клиент называет только дорожку — остальное (подложка, вид
                // авто-прокрутки) остаётся темы: пересказывать её целиком ради
                // двух цветов значит завести второй набор умолчаний.
                //
                // Дорожка выглядит одинаково в покое и под курсором: она узкая,
                // и подсветка на ней читается как рябь, а не как отклик.
                scroll = scroll.style(move |theme: &Theme, status| iced_widget::scrollable::Style {
                    vertical_rail: rail,
                    horizontal_rail: rail,
                    ..iced_widget::scrollable::default(theme, status)
                });
            }

            scroll.into()
        }
        Some(proto::widget::Type::ProgressBar(p)) => {
            // length/girth, а не width/height: полоса умеет быть вертикальной,
            // и «длина» у неё вдоль своей оси. Горизонтальная — по умолчанию,
            // так что длина здесь ширина, толщина — высота.
            let mut bar = progress_bar(p.range_start..=p.range_end, p.value)
                .length(convert_length(&p.width))
                .girth(convert_length(&p.height));

            if let Some(style) = &p.style {
                let style = iced_widget::progress_bar::Style {
                    background: style.track.as_ref().map(convert_background)
                        .unwrap_or(iced_core::Background::Color(Color::TRANSPARENT)),
                    bar: style.bar.as_ref().map(convert_background)
                        .unwrap_or(iced_core::Background::Color(Color::TRANSPARENT)),
                    border: convert_border(&style.border),
                };
                bar = bar.style(move |_theme: &Theme| style);
            }

            bar.into()
        }
        Some(proto::widget::Type::Stack(s)) => {
            stack(s.children.iter().map(convert_widget))
                .width(convert_length(&s.width))
                .height(convert_length(&s.height))
                .into()
        }
        Some(proto::widget::Type::Tooltip(t)) => {
            let content = if let Some(c) = &t.content {
                convert_widget(c)
            } else {
                Space::new().into()
            };

            let position = match t.position() {
                proto::TooltipPosition::TooltipTop => iced_widget::tooltip::Position::Top,
                proto::TooltipPosition::TooltipBottom => iced_widget::tooltip::Position::Bottom,
                proto::TooltipPosition::TooltipLeft => iced_widget::tooltip::Position::Left,
                proto::TooltipPosition::TooltipRight => iced_widget::tooltip::Position::Right,
            };

            let mut label = text(t.tooltip.clone());
            if t.text_size > 0.0 {
                label = label.size(t.text_size);
            }
            // Цвет надписи — из стиля подсказки: рисует её контейнер оверлея,
            // и свой цвет текста он передаёт содержимому сам.
            let mut hint = tooltip(content, label, position).gap(t.gap).padding(t.padding);
            if let Some(style) = &t.style {
                let style = convert_widget_style(style);
                hint = hint.style(move |_theme: &Theme| style);
            }
            hint.into()
        }
        Some(proto::widget::Type::Popover(p)) => {
            let anchor = p.anchor.as_ref().map_or_else(|| Space::new().into(), |w| convert_widget(w));
            let panel = p.panel.as_ref().map_or_else(|| Space::new().into(), |w| convert_widget(w));
            let mut popover = crate::module::popover::Popover::new(anchor, panel)
                .open(p.open)
                .gap(p.gap)
                // У панели два края, а не три: центрировать её относительно
                // якоря незачем — меню равняется по той стороне, с которой
                // ему хватает места.
                .align(match p.align_x() {
                    proto::Alignment::End => crate::module::popover::Align::End,
                    proto::Alignment::Start | proto::Alignment::Center => crate::module::popover::Align::Start,
                });
            if let Some(handler) = &p.on_dismiss
                && !handler.method.is_empty()
            {
                popover = popover.on_dismiss(UiMessage {
                    method: handler.method.clone(),
                    value: handler.value.clone(),
                });
            }
            popover.into()
        }
        Some(proto::widget::Type::Image(img)) => {
            let handle = img.handle.clone().unwrap_or_default();
            veldsdk::log::debug!(target: "handlers", "WgpuImage: handle.id = {}, handle.size = {}", handle.id, handle.size);
            WgpuImageWidget {
                handle,
                width: convert_length(&img.width),
                height: convert_length(&img.height),
            }.into()
        }
        Some(proto::widget::Type::Space(s)) => {
            Space::new().width(convert_length(&s.width)).height(convert_length(&s.height)).into()
        }
        // Виджет без ветки конвертации молча стал бы пустой колонкой, поэтому
        // сюда попадает только `type: None` — сообщение без содержимого.
        _ => column([]).into(),
    }
}

fn convert_length(len: &Option<proto::Length>) -> Length {
    match len {
        Some(l) => match &l.value {
            Some(proto::length::Value::Fixed(f)) => Length::Fixed(*f),
            Some(proto::length::Value::Fill(_)) => Length::Fill,
            Some(proto::length::Value::Shrink(_)) => Length::Shrink,
            Some(proto::length::Value::Portion(p)) => Length::FillPortion(*p as u16),
            None => Length::Shrink,
        },
        None => Length::Shrink,
    }
}

fn convert_length_val(len: &Option<proto::Length>) -> f32 {
    match len {
        Some(l) => match &l.value {
            Some(proto::length::Value::Fixed(f)) => *f,
            _ => f32::INFINITY,
        },
        None => f32::INFINITY,
    }
}

fn convert_padding(p: &Option<proto::Padding>) -> iced_core::Padding {
    match p {
        Some(p) => iced_core::Padding {
            top: p.top,
            right: p.right,
            bottom: p.bottom,
            left: p.left,
        },
        None => iced_core::Padding::ZERO,
    }
}

fn convert_horizontal_alignment(a: proto::Alignment) -> iced_core::alignment::Horizontal {
    match a {
        proto::Alignment::Start => iced_core::alignment::Horizontal::Left,
        proto::Alignment::Center => iced_core::alignment::Horizontal::Center,
        proto::Alignment::End => iced_core::alignment::Horizontal::Right,
    }
}

fn convert_vertical_alignment(a: proto::Alignment) -> iced_core::alignment::Vertical {
    match a {
        proto::Alignment::Start => iced_core::alignment::Vertical::Top,
        proto::Alignment::Center => iced_core::alignment::Vertical::Center,
        proto::Alignment::End => iced_core::alignment::Vertical::Bottom,
    }
}

fn convert_alignment(a: proto::Alignment) -> Option<alignment::Alignment> {
    match a {
        proto::Alignment::Start => Some(alignment::Alignment::Start),
        proto::Alignment::Center => Some(alignment::Alignment::Center),
        proto::Alignment::End => Some(alignment::Alignment::End),
    }
}

fn convert_color(c: &Option<proto::Color>) -> Color {
    match c {
        Some(c) => convert_color_value(c),
        None => Color::BLACK,
    }
}

fn convert_color_value(c: &proto::Color) -> Color {
    Color::from_rgba(c.r, c.g, c.b, c.a)
}

/// Внешний вид виджета в одном состоянии. Носитель здесь контейнер, потому что
/// его стиль в iced — самый общий: те же поля, что у кнопки и у подсказки, но
/// цвет текста необязателен, а «не сказано» и означает «как обычный текст».
fn convert_widget_style(style: &proto::WidgetStyle) -> iced_widget::container::Style {
    iced_widget::container::Style {
        background: style.background.as_ref().map(convert_background),
        text_color: style.text_color.as_ref().map(convert_color_value),
        border: convert_border(&style.border),
        shadow: convert_shadow(&style.shadow),
        // Виджет не прилипает к пиксельной сетке: его границы приходят из
        // раскладки клиента, и подтягивание их к целым пикселям разъезжается
        // с уже посчитанной геометрией соседей.
        snap: false,
    }
}

/// То же для кнопки: цвет текста у неё обязателен, и незаданный берётся у темы
/// (`inherited_text`).
fn convert_button_style(style: &proto::WidgetStyle, inherited_text: Color) -> iced_widget::button::Style {
    let base = convert_widget_style(style);
    iced_widget::button::Style {
        background: base.background,
        text_color: base.text_color.unwrap_or(inherited_text),
        border: base.border,
        shadow: base.shadow,
        snap: base.snap,
    }
}

fn convert_shadow(shadow: &Option<proto::Shadow>) -> iced_core::Shadow {
    match shadow {
        Some(s) => iced_core::Shadow {
            color: convert_color(&s.color),
            offset: iced_core::Vector::new(s.offset_x, s.offset_y),
            blur_radius: s.blur,
        },
        None => iced_core::Shadow::default(),
    }
}

/// Дорожка полосы прокрутки вместе с бегунком.
fn convert_rail(bar: &proto::Scrollbar) -> iced_widget::scrollable::Rail {
    let border = iced_core::Border { radius: bar.radius.into(), ..Default::default() };
    iced_widget::scrollable::Rail {
        background: bar.rail.as_ref().map(convert_background),
        border,
        scroller: iced_widget::scrollable::Scroller {
            background: bar.scroller.as_ref().map(convert_background)
                .unwrap_or(iced_core::Background::Color(Color::TRANSPARENT)),
            border,
        },
    }
}

/// Шрифт текста: логическое имя семейства (пустое — дефолтное) и начертание.
fn convert_font(family: &str, weight: proto::FontWeight) -> Font {
    Font {
        family: if family.is_empty() {
            Font::DEFAULT.family
        } else {
            Family::Name(intern_font_family(family))
        },
        weight: match weight {
            proto::FontWeight::WeightNormal => iced_core::font::Weight::Normal,
            proto::FontWeight::WeightMedium => iced_core::font::Weight::Medium,
            proto::FontWeight::WeightBold => iced_core::font::Weight::Bold,
        },
        ..Font::DEFAULT
    }
}

fn convert_background(bg: &proto::Background) -> iced_core::Background {
    match &bg.r#type {
        Some(proto::background::Type::Color(c)) => iced_core::Background::Color(convert_color(&Some(c.clone()))),
        None => iced_core::Background::Color(Color::TRANSPARENT),
    }
}

fn convert_border(b: &Option<proto::Border>) -> iced_core::Border {
    match b {
        Some(b) => iced_core::Border {
            color: convert_color(&b.color),
            width: b.width,
            radius: b.radius.into(),
        },
        None => iced_core::Border::default(),
    }
}

struct WgpuImageWidget {
    handle: veldsdk::proto::core::ResourceHandle,
    width: Length,
    height: Length,
}

impl<'a, Message, Theme> iced_widget::core::Widget<Message, Theme, GpuRenderer> for WgpuImageWidget {
    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(
        &mut self,
        _tree: &mut iced_widget::core::widget::Tree,
        _renderer: &GpuRenderer,
        limits: &iced_widget::core::layout::Limits,
    ) -> iced_widget::core::layout::Node {
        iced_widget::core::layout::Node::new(limits.resolve(self.width, self.height, Size::ZERO))
    }

    fn draw(
        &self,
        _tree: &iced_widget::core::widget::Tree,
        renderer: &mut GpuRenderer,
        _theme: &Theme,
        _style: &iced_widget::core::renderer::Style,
        layout: iced_widget::core::layout::Layout<'_>,
        _cursor: iced_widget::core::mouse::Cursor,
        _viewport: &iced_widget::core::Rectangle,
    ) {
        renderer.draw_wgpu_image(contain(layout.bounds(), self.handle.id), self.handle.id);
    }
}

/// Вписывает текстуру в отведённое место с сохранением пропорций.
///
/// Размеры спрашиваются у хоста: текстура чужая (её владелец — плагин,
/// выдавший нам read-грант), и её пропорции больше взять неоткуда. Если
/// размеры недоступны — рисуем на всё место.
fn contain(bounds: iced_core::Rectangle, texture_id: u64) -> iced_core::Rectangle {
    let Some((w, h)) = veldsdk::abi::resource_texture_size(texture_id) else { return bounds };
    if w == 0 || h == 0 || bounds.width <= 0.0 || bounds.height <= 0.0 { return bounds; }

    let scale = (bounds.width / w as f32).min(bounds.height / h as f32);
    let (width, height) = (w as f32 * scale, h as f32 * scale);
    iced_core::Rectangle {
        x: bounds.x + (bounds.width - width) / 2.0,
        y: bounds.y + (bounds.height - height) / 2.0,
        width,
        height,
    }
}

impl<'a, Message, Theme> From<WgpuImageWidget> for Element<'a, Message, Theme, GpuRenderer> {
    fn from(widget: WgpuImageWidget) -> Self {
        Self::new(widget)
    }
}
