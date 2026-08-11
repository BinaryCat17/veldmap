//! Element и билдеры виджетов: типобезопасная сборка дерева proto::Widget.
//! Стили и геометрия — в style.rs.

use crate::proto;
use super::style::{Alignment, Background, Length, Padding};

pub struct Element<M> {
    pub widget: proto::Widget,
    pub _marker: std::marker::PhantomData<M>,
}

impl<M> From<proto::Widget> for Element<M> {
    fn from(widget: proto::Widget) -> Self { Self { widget, _marker: std::marker::PhantomData } }
}

/// Именование элемента внутри его родителя. Отдельный трейт, а не метод
/// `Element`: имя навешивается на готовый виджет любого рода, и приводить его
/// к `Element` руками ради одного вызова — лишний шаг на каждом call-сайте.
pub trait Keyed<M> {
    /// По имени видно, где в списке на самом деле произошло изменение, и
    /// состояние (позиция скролла, каретка и выделение в поле ввода) не
    /// съезжает к соседу, когда элемент удалили из середины.
    ///
    /// Имя должно быть устойчивым между кадрами и уникальным среди соседей.
    /// Учитывает его пока только `column`, и только у однородных детей;
    /// подменять именем целое поддерево нельзя (см. `Widget.key`).
    fn key(self, key: impl Into<String>) -> Element<M>;
}

impl<M, T: Into<Element<M>>> Keyed<M> for T {
    fn key(self, key: impl Into<String>) -> Element<M> {
        let mut element = self.into();
        element.widget.key = key.into();
        element
    }
}

/// Сообщение разметки: то, что виджет скажет модулю при нажатии или вводе.
///
/// По шине едет пара строк — имя метода и значение (`proto::Handler`);
/// ui-service возвращает их эхом, смысла их не зная. Трейт держит перевод в эту
/// пару и обратно в одном месте, и этим связывает два конца: у отправителя тип
/// сообщения проверяет компилятор, а у получателя `decode` отвечает `None` на
/// всё, чего в наборе нет, — вместо молчаливого несовпадения двух списков строк.
pub trait UiMessage: Sized {
    /// Имя метода и значение. Имя должно быть устойчивым — по нему сообщение
    /// возвращается обратно.
    fn encode(&self) -> (String, String);
    /// Обратный разбор. `None` — такого сообщения в наборе нет или значение
    /// не разбирается.
    fn decode(method: &str, value: &str) -> Option<Self>;
}

// --- Builder Structs ---
//
// Сеттеры, у которых во всех билдерах одно и то же тело, объявляются макросом:
// написанные по разу на виджет, они заводят десяток мест, которые могут
// разойтись. Макрос, а не трейт: трейту нужен доступ к приватному полю чужого
// билдера, то есть тот же список имён — только длиннее.

/// `width`/`height` — у всех, кто просит место у раскладки.
macro_rules! sizing {
    ($($ty:ident),+ $(,)?) => { $(
        impl<M> $ty<M> {
            pub fn width(mut self, w: Length) -> Self {
                self.widget.width = Some(w.to_proto());
                self
            }
            pub fn height(mut self, h: Length) -> Self {
                self.widget.height = Some(h.to_proto());
                self
            }
        }
    )+ };
}

/// Отступ от границ виджета до его содержимого.
macro_rules! padded {
    ($($ty:ident),+ $(,)?) => { $(
        impl<M> $ty<M> {
            pub fn padding(mut self, p: impl Into<Padding>) -> Self {
                self.widget.padding = Some(p.into().to_proto());
                self
            }
        }
    )+ };
}

/// Носители нескольких детей.
macro_rules! parent {
    ($($ty:ident),+ $(,)?) => { $(
        impl<M> $ty<M> {
            pub fn push(mut self, child: impl Into<Element<M>>) -> Self {
                let child: Element<M> = child.into();
                self.widget.children.push(child.widget);
                self
            }
            pub fn extend(mut self, children: impl IntoIterator<Item = Element<M>>) -> Self {
                for child in children {
                    self.widget.children.push(child.widget);
                }
                self
            }
        }
    )+ };
}

sizing!(Column, Row, Stack, Text, Container, Scrollable, ProgressBar, Image);
padded!(Column, Row, Container);
parent!(Column, Row, Stack);

pub struct Column<M> {
    widget: proto::Column,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Column<M> {
    pub fn new() -> Self {
        Self {
            widget: proto::Column {
                width: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                ..Default::default()
            },
            _marker: std::marker::PhantomData
        }
    }
    /// Зазор между детьми.
    pub fn spacing(mut self, s: f32) -> Self {
        self.widget.spacing = s;
        self
    }
    /// Как дети стоят поперёк колонки, то есть по горизонтали.
    pub fn align_items(mut self, align: Alignment) -> Self {
        self.widget.align_items = align as i32;
        self
    }
}

impl<M> From<Column<M>> for Element<M> {
    fn from(c: Column<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Column(c.widget)),
            ..Default::default()
        }.into()
    }
}

pub fn column<M>(children: impl IntoIterator<Item = Element<M>>) -> Column<M> {
    Column::new().extend(children)
}

pub struct Row<M> {
    widget: proto::Row,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Row<M> {
    pub fn new() -> Self {
        Self {
            widget: proto::Row {
                width: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                ..Default::default()
            },
            _marker: std::marker::PhantomData
        }
    }
    /// Зазор между детьми.
    pub fn spacing(mut self, s: f32) -> Self {
        self.widget.spacing = s;
        self
    }
    /// Как дети стоят поперёк строки, то есть по вертикали.
    pub fn align_items(mut self, align: Alignment) -> Self {
        self.widget.align_items = align as i32;
        self
    }
}

impl<M> From<Row<M>> for Element<M> {
    fn from(r: Row<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Row(r.widget)),
            ..Default::default()
        }.into()
    }
}

pub fn row<M>(children: impl IntoIterator<Item = Element<M>>) -> Row<M> {
    Row::new().extend(children)
}

pub struct Text<M> {
    widget: proto::Text,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Text<M> {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            widget: proto::Text {
                content: content.into(),
                size: 16.0,
                color: None,
                horizontal_alignment: 0,
                vertical_alignment: 0,
                width: None,
                height: None,
                shaping: proto::Shaping::ShapeBasic as i32,
                font_family: String::new(),
                wrapping: proto::Wrapping::WrapWord as i32,
                weight: proto::FontWeight::WeightNormal as i32,
            },
            _marker: std::marker::PhantomData
        }
    }
    pub fn size(mut self, size: f32) -> Self {
        self.widget.size = size;
        self
    }
    pub fn color(mut self, color: super::style::Color) -> Self {
        self.widget.color = Some(color.to_proto());
        self
    }
    pub fn horizontal_alignment(mut self, align: Alignment) -> Self {
        self.widget.horizontal_alignment = align as i32;
        self
    }
    pub fn vertical_alignment(mut self, align: Alignment) -> Self {
        self.widget.vertical_alignment = align as i32;
        self
    }
    /// Сложность разбора строки; умолчание — простой (см. `Shaping`).
    pub fn shaping(mut self, shaping: proto::Shaping) -> Self {
        self.widget.shaping = shaping as i32;
        self
    }
    /// Одна строка: подпись, а не абзац. Ширину строка от этого не теряет —
    /// не поместившееся уедет за отведённое место, и обрежет его только
    /// скроллируемый предок. Тексту, который обязан остаться в своей рамке,
    /// нужен не этот метод, а `wrapping(WRAP_WORD_OR_GLYPH)` (см. `Wrapping`).
    pub fn single_line(mut self) -> Self {
        self.widget.wrapping = proto::Wrapping::NoWrap as i32;
        self
    }
    /// Полный выбор поведения при нехватке ширины; `single_line` — частый
    /// случай этого же.
    pub fn wrapping(mut self, wrapping: proto::Wrapping) -> Self {
        self.widget.wrapping = wrapping as i32;
        self
    }
    /// Логическое имя шрифта, как оно было зарегистрировано в `GpuRenderer::new`
    /// на стороне ui-service (например "Icons"). Пусто — используется дефолтный шрифт.
    pub fn font_family(mut self, name: impl Into<String>) -> Self {
        self.widget.font_family = name.into();
        self
    }
    /// Насыщенность начертания — см. `FontWeight` в types.proto.
    pub fn weight(mut self, weight: proto::FontWeight) -> Self {
        self.widget.weight = weight as i32;
        self
    }
}

impl<M> From<Text<M>> for Element<M> {
    fn from(t: Text<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Text(t.widget)),
            ..Default::default()
        }.into()
    }
}

pub fn text<M>(content: impl Into<String>) -> Text<M> {
    Text::new(content)
}

/// Моноширинный текст: имя файла, размер, путь, время. Отдельный билдер по той
/// же причине, что и `icon`, — имя шрифта знает ui-service, а не call-сайт.
/// Однострочность здесь не выбор оформления: колонка такого текста считается по
/// знакам, и перенос сделал бы её ширину неправдой.
pub fn mono<M>(content: impl Into<String>) -> Text<M> {
    Text::new(content).font_family(super::style::FONT_MONO).single_line()
}

/// Глиф иконочного шрифта. Отдельный билдер, а не `text(..).font_family(..)`
/// на каждом call-сайте: имя шрифта — деталь ui-service (он регистрирует его в
/// `GpuRenderer::new`), и знать её вызывающему незачем. Однострочность здесь
/// не выбор оформления, а свойство глифа: переносить в нём нечего.
///
/// Возвращает `Text`, а не готовый `Element`, — цвет и размер иконки
/// дописываются обычной цепочкой.
pub fn icon<M>(glyph: impl Into<String>) -> Text<M> {
    Text::new(glyph).font_family(super::style::FONT_ICONS).single_line()
}

pub struct TextInput<M> {
    widget: proto::TextInput,
    _marker: std::marker::PhantomData<M>,
}

impl<M> TextInput<M> {
    pub fn new(placeholder: &str, value: &str) -> Self {
        Self {
            widget: proto::TextInput {
                placeholder: placeholder.to_string(),
                value: value.to_string(),
                size: 16.0,
                padding: Some(proto::Padding { top: 5.0, bottom: 5.0, left: 10.0, right: 10.0 }),
                ..Default::default()
            },
            _marker: std::marker::PhantomData,
        }
    }
    /// Что модуль получит на каждое изменение текста. Принимает не готовое
    /// сообщение, а способ его собрать: набранное подставит рендерер, а здесь
    /// нужно только имя метода — за ним и зовём конструктор с пустой строкой.
    pub fn on_input(mut self, message: impl FnOnce(String) -> M) -> Self
    where
        M: UiMessage,
    {
        let (method, _) = message(String::new()).encode();
        self.widget.on_input = Some(proto::Handler { method, value: String::new() });
        self
    }
    /// Что модуль получит по Enter.
    pub fn on_submit(mut self, message: M) -> Self
    where
        M: UiMessage,
    {
        let (method, value) = message.encode();
        self.widget.on_submit = Some(proto::Handler { method, value });
        self
    }
    /// Только ширина: высоту поле ввода берёт из размера шрифта и отступов.
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }
    pub fn padding(mut self, p: impl Into<Padding>) -> Self {
        self.widget.padding = Some(p.into().to_proto());
        self
    }
    pub fn size(mut self, size: f32) -> Self {
        self.widget.size = size;
        self
    }
    pub fn style(mut self, style: super::style::TextInputStyle) -> Self {
        self.widget.style = Some(style.to_proto());
        self
    }
    /// Логическое имя шрифта — как у `Text::font_family`.
    pub fn font_family(mut self, name: impl Into<String>) -> Self {
        self.widget.font_family = name.into();
        self
    }
}

impl<M> From<TextInput<M>> for Element<M> {
    fn from(t: TextInput<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::TextInput(t.widget)),
            ..Default::default()
        }.into()
    }
}

pub fn text_input<M>(placeholder: &str, value: &str) -> TextInput<M> {
    TextInput::new(placeholder, value)
}

pub struct Container<M> {
    widget: proto::Container,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Container<M> {
    pub fn new(child: impl Into<Element<M>>) -> Self {
        Self {
            widget: proto::Container {
                child: Some(Box::new(child.into().widget)),
                ..Default::default()
            },
            _marker: std::marker::PhantomData,
        }
    }
    /// Потолок ширины: виджет займёт не больше, сколько бы места ему ни дали.
    /// Ограничить так можно кого угодно — тем `Container` и полезен.
    pub fn max_width(mut self, w: f32) -> Self {
        self.widget.max_width = Some(proto::Length { value: Some(proto::length::Value::Fixed(w)) });
        self
    }
    pub fn max_height(mut self, h: f32) -> Self {
        self.widget.max_height = Some(proto::Length { value: Some(proto::length::Value::Fixed(h)) });
        self
    }
    /// Фон без остального стиля — самый частый случай `style`.
    pub fn background(mut self, background: impl Into<Background>) -> Self {
        let style = self.widget.style.get_or_insert_with(Default::default);
        style.background = Some(background.into().to_proto());
        self
    }
    pub fn style(mut self, style: super::style::WidgetStyle) -> Self {
        self.widget.style = Some(style.to_proto());
        self
    }
    /// Обрезать содержимое по своим границам — см. `Container.clip`.
    pub fn clip(mut self) -> Self {
        self.widget.clip = true;
        self
    }
    pub fn align_x(mut self, align: Alignment) -> Self {
        self.widget.align_x = align as i32;
        self
    }
    pub fn align_y(mut self, align: Alignment) -> Self {
        self.widget.align_y = align as i32;
        self
    }
    pub fn center_x(mut self) -> Self {
        self.widget.align_x = Alignment::Center as i32;
        self
    }
    pub fn center_y(mut self) -> Self {
        self.widget.align_y = Alignment::Center as i32;
        self
    }

    // -- Нажимаемость --
    //
    // Отдельной кнопки в протоколе нет: кнопка — это коробка, на которую можно
    // нажать (см. `Interaction` в types.proto). Любой из методов ниже её и
    // заводит, поэтому назвать можно только то, что отличается от покоя.

    /// Что модуль получит при нажатии. Коробка со стилями, но без сообщения —
    /// выключенная кнопка: рендерер рисует её `disabled` и нажатие не пускает.
    pub fn on_press(mut self, message: M) -> Self
    where
        M: UiMessage,
    {
        let (method, value) = message.encode();
        self.interaction().on_press = Some(proto::Handler { method, value });
        self
    }

    /// Вид под курсором; не назван — как в покое.
    pub fn hovered(mut self, style: super::style::WidgetStyle) -> Self {
        self.interaction().hovered = Some(style.to_proto());
        self
    }

    /// Вид под нажатой кнопкой мыши; не назван — как под курсором.
    pub fn pressed(mut self, style: super::style::WidgetStyle) -> Self {
        self.interaction().pressed = Some(style.to_proto());
        self
    }

    /// Вид выключенной; не назван — как в покое.
    pub fn disabled(mut self, style: super::style::WidgetStyle) -> Self {
        self.interaction().disabled = Some(style.to_proto());
        self
    }

    fn interaction(&mut self) -> &mut proto::Interaction {
        self.widget.interaction.get_or_insert_with(Default::default)
    }
}

impl<M> From<Container<M>> for Element<M> {
    fn from(c: Container<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Container(Box::new(c.widget))),
            ..Default::default()
        }.into()
    }
}

pub fn container<M>(child: impl Into<Element<M>>) -> Container<M> {
    Container::new(child)
}

pub struct Scrollable<M> {
    widget: proto::Scrollable,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Scrollable<M> {
    pub fn new(content: impl Into<Element<M>>) -> Self {
        Self {
            widget: proto::Scrollable {
                content: Some(Box::new(content.into().widget)),
                width: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
                direction: proto::ScrollDirection::ScrollVertical as i32,
                scrollbar: None,
            },
            _marker: std::marker::PhantomData,
        }
    }
    /// Ось прокрутки. Горизонтальная нужна там, где содержимое шире места и
    /// сжимать его нельзя: полоса вкладок, длинная строка команд.
    pub fn direction(mut self, direction: proto::ScrollDirection) -> Self {
        self.widget.direction = direction as i32;
        self
    }
    /// Вид и ширина полосы прокрутки.
    pub fn scrollbar(mut self, bar: super::style::Scrollbar) -> Self {
        self.widget.scrollbar = Some(bar.to_proto());
        self
    }
}

impl<M> From<Scrollable<M>> for Element<M> {
    fn from(s: Scrollable<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Scrollable(Box::new(s.widget))),
            ..Default::default()
        }.into()
    }
}

pub fn scrollable<M>(content: impl Into<Element<M>>) -> Scrollable<M> {
    Scrollable::new(content)
}

pub struct ProgressBar<M> {
    widget: proto::ProgressBar,
    _marker: std::marker::PhantomData<M>,
}

impl<M> ProgressBar<M> {
    pub fn new(range: std::ops::RangeInclusive<f32>, value: f32) -> Self {
        Self {
            widget: proto::ProgressBar {
                range_start: *range.start(),
                range_end: *range.end(),
                value,
                width: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Fixed(12.0)) }),
                style: None,
            },
            _marker: std::marker::PhantomData,
        }
    }
}

impl<M> From<ProgressBar<M>> for Element<M> {
    fn from(p: ProgressBar<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::ProgressBar(p.widget)),
            ..Default::default()
        }.into()
    }
}

impl<M> ProgressBar<M> {
    pub fn style(mut self, style: super::style::ProgressBarStyle) -> Self {
        self.widget.style = Some(style.to_proto());
        self
    }
}

pub fn progress_bar<M>(range: std::ops::RangeInclusive<f32>, value: f32) -> ProgressBar<M> {
    ProgressBar::new(range, value)
}

pub struct Space<M> {
    widget: proto::Space,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Space<M> {
    pub fn new(width: Length, height: Length) -> Self {
        Self {
            widget: proto::Space {
                width: Some(width.to_proto()),
                height: Some(height.to_proto()),
            },
            _marker: std::marker::PhantomData,
        }
    }
}

impl<M> From<Space<M>> for Element<M> {
    fn from(s: Space<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Space(s.widget)),
            ..Default::default()
        }.into()
    }
}

pub fn space<M>(width: Length, height: Length) -> Space<M> {
    Space::new(width, height)
}

pub struct Stack<M> {
    widget: proto::Stack,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Stack<M> {
    pub fn new() -> Self {
        Self {
            widget: proto::Stack {
                width: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Shrink(true)) }),
                ..Default::default()
            },
            _marker: std::marker::PhantomData,
        }
    }
}

impl<M> From<Stack<M>> for Element<M> {
    fn from(s: Stack<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Stack(s.widget)),
            ..Default::default()
        }.into()
    }
}

pub fn stack<M>(children: impl IntoIterator<Item = Element<M>>) -> Stack<M> {
    Stack::new().extend(children)
}

pub struct Tooltip<M> {
    widget: proto::Tooltip,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Tooltip<M> {
    pub fn new(content: impl Into<Element<M>>, label: impl Into<String>, position: proto::TooltipPosition) -> Self {
        Self {
            widget: proto::Tooltip {
                content: Some(Box::new(content.into().widget)),
                tooltip: label.into(),
                position: position as i32,
                gap: 5.0,
                padding: 5.0,
                style: None,
                text_size: 0.0,
            },
            _marker: std::marker::PhantomData,
        }
    }
    /// Зазор между подсказкой и тем, к чему она относится.
    pub fn gap(mut self, gap: f32) -> Self {
        self.widget.gap = gap;
        self
    }
    /// Отступ внутри самой подсказки — одинаковый со всех сторон.
    pub fn padding(mut self, padding: f32) -> Self {
        self.widget.padding = padding;
        self
    }
    /// Вид самой подсказки; `text_color` стиля — цвет её надписи.
    pub fn style(mut self, style: super::style::WidgetStyle) -> Self {
        self.widget.style = Some(style.to_proto());
        self
    }
    pub fn text_size(mut self, size: f32) -> Self {
        self.widget.text_size = size;
        self
    }
}

impl<M> From<Tooltip<M>> for Element<M> {
    fn from(t: Tooltip<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Tooltip(Box::new(t.widget))),
            ..Default::default()
        }.into()
    }
}

pub fn tooltip<M>(content: impl Into<Element<M>>, label: impl Into<String>, position: proto::TooltipPosition) -> Tooltip<M> {
    Tooltip::new(content, label, position)
}

/// Всплывающая у якоря панель — см. `Popover` в types.proto.
pub struct Popover<M> {
    widget: proto::Popover,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Popover<M> {
    pub fn new(anchor: impl Into<Element<M>>, panel: impl Into<Element<M>>) -> Self {
        Self {
            widget: proto::Popover {
                anchor: Some(Box::new(anchor.into().widget)),
                panel: Some(Box::new(panel.into().widget)),
                ..Default::default()
            },
            _marker: std::marker::PhantomData,
        }
    }
    /// Открыта ли панель. Состояние меню принадлежит клиенту — сервис его не
    /// помнит и сам не переключает.
    pub fn open(mut self, open: bool) -> Self {
        self.widget.open = open;
        self
    }
    /// Каким краем панель равняется на якорь.
    pub fn align_x(mut self, align: Alignment) -> Self {
        self.widget.align_x = align as i32;
        self
    }
    pub fn gap(mut self, gap: f32) -> Self {
        self.widget.gap = gap;
        self
    }
    /// Что придёт, когда щёлкнут мимо панели: открытое меню иначе нечем
    /// закрыть.
    pub fn on_dismiss(mut self, message: M) -> Self
    where
        M: UiMessage,
    {
        let (method, value) = message.encode();
        self.widget.on_dismiss = Some(proto::Handler { method, value });
        self
    }
}

impl<M> From<Popover<M>> for Element<M> {
    fn from(p: Popover<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Popover(Box::new(p.widget))),
            ..Default::default()
        }.into()
    }
}

pub fn popover<M>(anchor: impl Into<Element<M>>, panel: impl Into<Element<M>>) -> Popover<M> {
    Popover::new(anchor, panel)
}

pub struct Image<M> {
    widget: proto::WgpuImage,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Image<M> {
    pub fn new(handle: veldsdk::proto::core::ResourceHandle) -> Self {
        Self {
            widget: proto::WgpuImage {
                handle: Some(handle),
                width: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
            },
            _marker: std::marker::PhantomData,
        }
    }
}

impl<M> From<Image<M>> for Element<M> {
    fn from(i: Image<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Image(i.widget)),
            ..Default::default()
        }.into()
    }
}

pub fn image<M>(handle: veldsdk::proto::core::ResourceHandle) -> Image<M> {
    Image::new(handle)
}
