//! Element и билдеры виджетов: типобезопасная сборка дерева proto::Widget.
//! Стили и геометрия — в style.rs.

use crate::proto;
use super::style::{Alignment, Background, Color, Length, Padding};

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
/// В разметке оно объявляется парой строк (`proto::Handler`) — именем метода и
/// значением; ui-service возвращает имя эхом, смысла его не зная. Трейт держит
/// перевод туда и обратно в одном месте, и этим связывает два конца: у
/// отправителя тип сообщения проверяет компилятор, а у получателя `decode`
/// отвечает `None` на всё, чего в наборе нет, — вместо молчаливого
/// несовпадения двух списков строк.
///
/// Разбирается при этом не значение из разметки, а то, что приехало: у события
/// с данными нагрузку подставляет рендерер (см. [`Payload`]).
pub trait UiMessage: Sized {
    /// Имя метода и значение. Имя должно быть устойчивым — по нему сообщение
    /// возвращается обратно.
    fn encode(&self) -> (String, String);
    /// Кого это сообщение адресует: вкладку, строку списка — то, чего нельзя
    /// вывести из имени метода, когда одинаковых виджетов на экране много.
    ///
    /// Отдельно от `encode`, потому что нагрузку у поля ввода, области и
    /// ползунка подменяет рендерер, а адресата — никто (см. `Handler.key`).
    /// Пусто — сообщение адресата не несёт, и это законно: у «открыть новую
    /// вкладку» его и нет.
    fn key(&self) -> String {
        String::new()
    }
    /// Обратный разбор. `None` — такого сообщения в наборе нет или нагрузка
    /// не того вида.
    fn decode(event: &proto::UiEventResponse) -> Option<Self>;
}

/// Обработчик из сообщения: имя метода, нагрузка и адресат — всё, что виджет
/// отдаёт обратно. Собирается здесь и только здесь, чтобы ни один построитель
/// не забыл про адресата.
fn handler<M: UiMessage>(message: &M) -> proto::Handler {
    let (method, value) = message.encode();
    proto::Handler { method, value, key: message.key() }
}

/// То же для виджета, чью нагрузку изготавливает рендерер: из сообщения-образца
/// берутся только имя метода и адресат, а объявленное значение выбрасывается —
/// его всё равно подменят.
fn substituted<M: UiMessage>(sample: &M) -> proto::Handler {
    let (method, _) = sample.encode();
    proto::Handler { method, value: String::new(), key: sample.key() }
}

/// Чтение нагрузки события. Вид её у каждого метода свой и заранее известен
/// тому, кто объявил обработчик, поэтому спрашивают здесь именно тот, которого
/// ждут: «не тот вид» неотличимо от «нет нагрузки» и в обоих случаях означает
/// одно — разобрать нечего.
pub trait Payload {
    /// Строка: названная разметкой либо подставленная рендерером (набранный
    /// текст). Пусто — нагрузка другого вида.
    fn value(&self) -> &str;
    fn pointer(&self) -> Option<&proto::PointerEvent>;
    fn size(&self) -> Option<&proto::ViewportSize>;
    /// Что и куда бросили. Пусто — нагрузка другого вида.
    fn drop(&self) -> Option<&proto::DropEvent>;
}

impl Payload for proto::UiEventResponse {
    fn value(&self) -> &str {
        match &self.payload {
            Some(proto::ui_event_response::Payload::Value(value)) => value,
            _ => "",
        }
    }

    fn pointer(&self) -> Option<&proto::PointerEvent> {
        match &self.payload {
            Some(proto::ui_event_response::Payload::Pointer(pointer)) => Some(pointer),
            _ => None,
        }
    }

    fn size(&self) -> Option<&proto::ViewportSize> {
        match &self.payload {
            Some(proto::ui_event_response::Payload::Size(size)) => Some(size),
            _ => None,
        }
    }

    fn drop(&self) -> Option<&proto::DropEvent> {
        match &self.payload {
            Some(proto::ui_event_response::Payload::Drop(drop)) => Some(drop),
            _ => None,
        }
    }
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

/// Умолчание протокола, а не билдера: этим же числом ui-service чинит кегль,
/// присланный негодным. Одно на обоих — общим файлом (см. `typography.rs`).
use super::typography::DEFAULT_TEXT_SIZE;

sizing!(Column, Row, Text, Container, Scrollable, ProgressBar, Image, Viewport);
padded!(Column, Row, Container);
parent!(Column, Row);

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
                size: DEFAULT_TEXT_SIZE,
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
    /// Логическое имя шрифта — одно из `style::FONT_*`. Пусто — дефолтный.
    /// Литералом не называть: имена объявлены в одном месте, и под ними же
    /// ui-service кладёт файлы шрифтов (см. `style::FONT_UI`).
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
                size: DEFAULT_TEXT_SIZE,
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
        self.widget.on_input = Some(substituted(&message(String::new())));
        self
    }
    /// Что модуль получит по Enter.
    pub fn on_submit(mut self, message: M) -> Self
    where
        M: UiMessage,
    {
        self.widget.on_submit = Some(handler(&message));
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
        self.interaction().on_press = Some(handler(&message));
        self
    }

    /// Коробку можно взять мышью и перенести. Нагрузка — то, чем владелец
    /// разметки её называет: она вернётся ему в событии броска.
    pub fn draggable(mut self, payload: impl Into<String>) -> Self {
        self.widget.drag = Some(proto::Drag { payload: payload.into() });
        self
    }

    /// В коробку можно бросить перенесённое.
    ///
    /// Сообщение принимается способом его собрать, а не готовым, — как у поля
    /// ввода: что принесли и в какой край попали, знает только рендерер. Адресат
    /// при этом уезжает именем зоны (`UiEventResponse.key`), и зон в разметке
    /// потому может быть сколько угодно.
    pub fn on_drop(
        mut self,
        message: impl FnOnce(proto::DropEvent) -> M,
        edges: bool,
        highlight: Color,
    ) -> Self
    where
        M: UiMessage,
    {
        self.widget.drop = Some(proto::Drop {
            on_drop: Some(substituted(&message(proto::DropEvent::default()))),
            edges,
            highlight: Some(highlight.to_proto()),
        });
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
                name: String::new(),
                scroll_to: None,
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

    /// Имя области — им её адресует наводка. Списков на экране несколько, и
    /// без имени наводка увела бы их все разом.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.widget.name = name.into();
        self
    }

    /// Куда встать по вертикали и по чьей просьбе. `None` — не наводить.
    /// Применяется один раз на просьбу (см. `Scrollable.scroll_to` в
    /// types.proto).
    pub fn scroll_to(mut self, aim: Option<proto::ScrollTo>) -> Self {
        self.widget.scroll_to = aim;
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

pub struct Divider<M> {
    widget: proto::Divider,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Divider<M> {
    /// Граница с обязательным обработчиком: без него это просто линия, а её
    /// рисует контейнер (см. `hairline` у клиентов).
    ///
    /// Сообщение принимается способом его собрать, а не готовым, — как у
    /// ползунка: сдвиг подставит рендерер. Адресат при этом уезжает именем
    /// границы (`UiEventResponse.key`), и границ в раскладке потому может быть
    /// сколько угодно.
    pub fn new(axis: proto::DividerAxis, on_drag: impl FnOnce(f32) -> M) -> Self
    where
        M: UiMessage,
    {
        Self {
            widget: proto::Divider {
                axis: axis as i32,
                thickness: 1.0,
                line: 1.0,
                color: None,
                active: None,
                on_drag: Some(substituted(&on_drag(0.0))),
                on_release: None,
            },
            _marker: std::marker::PhantomData,
        }
    }

    /// Ширина полосы, за которую берутся, и толщина видимой линии.
    pub fn thickness(mut self, thickness: f32) -> Self {
        self.widget.thickness = thickness;
        self
    }

    pub fn line(mut self, line: f32) -> Self {
        self.widget.line = line;
        self
    }

    /// Границу отпустили — конец перетаскивания одним событием.
    pub fn on_release(mut self, message: M) -> Self
    where
        M: UiMessage,
    {
        self.widget.on_release = Some(handler(&message));
        self
    }

    /// Цвет линии в покое и когда за границу можно взяться.
    pub fn color(mut self, color: Color, active: Color) -> Self {
        self.widget.color = Some(color.to_proto());
        self.widget.active = Some(active.to_proto());
        self
    }
}

impl<M> From<Divider<M>> for Element<M> {
    fn from(d: Divider<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Divider(d.widget)),
            ..Default::default()
        }
        .into()
    }
}

pub fn divider<M: UiMessage>(
    axis: proto::DividerAxis,
    on_drag: impl FnOnce(f32) -> M,
) -> Divider<M> {
    Divider::new(axis, on_drag)
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
    /// Открытость приходит вместе с панелью, а не отдельным доводом, и панель
    /// приходит замыканием: закрытая не строится вовсе. Иначе меню каждой
    /// строки списка собиралось бы, ехало по шине и разворачивалось в виджеты
    /// на каждом кадре — при том, что на экране его нет.
    ///
    /// Состояние меню при этом принадлежит клиенту: сервис его не помнит и сам
    /// не переключает.
    pub fn new(anchor: impl Into<Element<M>>, open: bool,
               panel: impl FnOnce() -> Element<M>) -> Self {
        Self {
            widget: proto::Popover {
                anchor: Some(Box::new(anchor.into().widget)),
                panel: open.then(|| Box::new(panel().widget)),
                open,
                ..Default::default()
            },
            _marker: std::marker::PhantomData,
        }
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
        self.widget.on_dismiss = Some(handler(&message));
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

pub fn popover<M>(anchor: impl Into<Element<M>>, open: bool,
                  panel: impl FnOnce() -> Element<M>) -> Popover<M> {
    Popover::new(anchor, open, panel)
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

/// Место, которое модуль рисует своим рендерером (см. `Viewport` в types.proto).
///
/// Заполняет отведённое ему место целиком и перерисовывается каждый кадр, а
/// взамен рассказывает владельцу две вещи: сколько места ему досталось и что
/// делает над ним указатель.
pub struct Viewport<M> {
    widget: proto::Viewport,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Viewport<M> {
    /// Без текстуры: место занимается, но остаётся пустым — столько времени,
    /// сколько владельцу нужно, чтобы ответить на первый `on_resized`.
    pub fn new() -> Self {
        Self {
            widget: proto::Viewport {
                texture: None,
                width: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
                height: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
                on_resized: None,
                on_pointer: None,
            },
            _marker: std::marker::PhantomData,
        }
    }

    /// Что показывать. Владелец текстуры обязан выдать рендереру право чтения
    /// (`grant_read` на «ui-service») — тем же обрядом, что у превью.
    pub fn texture(mut self, handle: veldsdk::proto::core::ResourceHandle) -> Self {
        self.widget.texture = Some(handle);
        self
    }

    /// Что модуль получит, когда области достанется новый размер. Как и у поля
    /// ввода, принимается способ собрать сообщение, а не готовое: нагрузку
    /// подставит рендерер.
    ///
    /// Адресат сообщения при этом сохраняется и уезжает именем области
    /// (`UiEventResponse.key`): у сообщения, которое несёт ещё и вкладку, он
    /// назван её `key()`, а объявленное значение выбрасывается — его всё равно
    /// подменит рендерер (см. [`substituted`]).
    pub fn on_resized(mut self, message: impl FnOnce(proto::ViewportSize) -> M) -> Self
    where
        M: UiMessage,
    {
        self.widget.on_resized = Some(substituted(&message(proto::ViewportSize::default())));
        self
    }

    /// Что модуль получит на движение, нажатие, отпускание и прокрутку.
    pub fn on_pointer(mut self, message: impl FnOnce(proto::PointerEvent) -> M) -> Self
    where
        M: UiMessage,
    {
        self.widget.on_pointer = Some(substituted(&message(proto::PointerEvent::default())));
        self
    }
}

pub struct Slider<M> {
    widget: proto::Slider,
    _marker: std::marker::PhantomData<M>,
}

impl<M> Slider<M> {
    /// Ползунок с обязательным обработчиком: без него он неотличим от полосы
    /// прогресса, но при этом ловит указатель.
    ///
    /// Сообщение принимается способом его собрать, а не готовым, — как у поля
    /// ввода: число подставит рендерер. Адресат сообщения при этом уезжает
    /// именем ползунка (`UiEventResponse.key`), и ползунок в списке тем себя и
    /// называет: разметка знает, чья это строка, а рендерер — какое число.
    pub fn new(
        range: std::ops::RangeInclusive<f32>,
        value: f32,
        on_change: impl FnOnce(f32) -> M,
    ) -> Self
    where
        M: UiMessage,
    {
        let change = substituted(&on_change(value));
        Self {
            widget: proto::Slider {
                range_start: *range.start(),
                range_end: *range.end(),
                value,
                step: 0.0,
                on_change: Some(change),
                on_release: None,
                width: Some(proto::Length { value: Some(proto::length::Value::Fill(true)) }),
                height: 0.0,
                style: None,
            },
            _marker: std::marker::PhantomData,
        }
    }

    pub fn step(mut self, step: f32) -> Self {
        self.widget.step = step;
        self
    }

    /// Что придёт, когда ручку отпустят. В отличие от `on_change`, сообщение
    /// готовое: нагрузку рендерер здесь не подставляет — говорить ему нечего,
    /// кроме «отпустили».
    pub fn on_release(mut self, message: M) -> Self
    where
        M: UiMessage,
    {
        self.widget.on_release = Some(handler(&message));
        self
    }

    /// Ширина — как у всех, а вот высота своя и в точках: у ползунка это не
    /// место в раскладке, а рост дорожки с ручкой, и `Length::Fill` для неё
    /// означал бы ручку во всю колонку.
    pub fn width(mut self, w: Length) -> Self {
        self.widget.width = Some(w.to_proto());
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.widget.height = height;
        self
    }

    pub fn style(mut self, style: super::style::SliderStyle) -> Self {
        self.widget.style = Some(style.to_proto());
        self
    }
}

impl<M> From<Slider<M>> for Element<M> {
    fn from(s: Slider<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Slider(s.widget)),
            ..Default::default()
        }.into()
    }
}

pub fn slider<M: UiMessage>(
    range: std::ops::RangeInclusive<f32>,
    value: f32,
    on_change: impl FnOnce(f32) -> M,
) -> Slider<M> {
    Slider::new(range, value, on_change)
}

impl<M> Default for Viewport<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M> From<Viewport<M>> for Element<M> {
    fn from(v: Viewport<M>) -> Self {
        proto::Widget {
            r#type: Some(proto::widget::Type::Viewport(v.widget)),
            ..Default::default()
        }.into()
    }
}

pub fn viewport<M>() -> Viewport<M> {
    Viewport::new()
}
