//! Обход разметки: где на экране элемент, названный не пикселями.
//!
//! Прогон по сценарию называет элемент тем же, чем его называет разметка, —
//! обработчиком нажимаемой коробки или частью видимой надписи. Перевести это в
//! прямоугольник может только тот, кто считает раскладку: сама разметка о
//! координатах не знает вовсе, и спрашивают поэтому нас.
//!
//! Имя доезжает до дерева iced единственным способом — `Id` на коробке внутри
//! кнопки (`converter::naming`), и обход сверяет его на равенство. Прочесть имя
//! обратно из `Id` нельзя, да и незачем: спрашивают всегда про одно.

use std::cell::Cell;

use iced_core::widget::{Id, Operation};
use iced_core::{Rectangle, Size, Vector};

/// Чем называют коробки, пока идёт прогон по сценарию.
///
/// Вид имени задаёт сам вопрос, и иначе нельзя: прочесть имя обратно из `Id`
/// нечем — сверяется оно только на равенство. Значит «этот обработчик с любой
/// нагрузкой» выражается единственным способом: нагрузку в имя не кладут
/// вовсе. Спрашивают по одному вопросу за раз, так что двух видов имён на
/// экране одновременно не бывает.
#[derive(Clone, Copy)]
enum Naming {
    /// Не называть вовсе — обычный запуск.
    Off,
    /// Одним методом: нагрузку в вопросе не назвали, подойдёт любая.
    Method,
    /// Методом с нагрузкой: спросили про определённый элемент.
    WithValue,
}

thread_local! {
    /// Как называют коробки в этом кадре. Взводится вопросом и больше не
    /// гаснет: спросивший однажды спросит и дальше, а обход по дереву без имён
    /// ответил бы «нет такого» вместо «не успели назвать».
    ///
    /// Переменной, а не доводом converter'а: имя вешается в двух местах, а
    /// довод пришлось бы протащить через всю рекурсию разметки. Обычный запуск
    /// за имена не платит — их не делают вовсе.
    static NAMING: Cell<Naming> = const { Cell::new(Naming::Off) };
}

/// Спросили — дальше разметка собирается с именами того вида, какого требует
/// вопрос.
///
/// Взводится при получении вопроса, до сборки разметки: обход идёт по уже
/// собранному дереву, и имена в нём обязаны быть теми, о которых спросили.
pub fn asking(value: &str) {
    NAMING.with(|naming| naming.set(match value.is_empty() {
        true => Naming::Method,
        false => Naming::WithValue,
    }));
}

/// Имя коробки для нынешнего вопроса; `None` — называть не надо.
///
/// Ключ обработчика в имя не входит никогда: им разметка называет
/// вкладку-адресата, а не элемент, и от запуска к запуску он свой.
/// Разделитель неотображаемый — нагрузка бывает и путём, и ключом меню с
/// двоеточиями, и любой печатный знак в ней однажды встретится.
pub fn name(method: &str, value: &str) -> Option<String> {
    match NAMING.with(Cell::get) {
        Naming::Off => None,
        Naming::Method => Some(method.to_string()),
        Naming::WithValue => Some(format!("{}\u{1}{}", method, value)),
    }
}

/// Кем назвали искомое.
pub enum Sought {
    /// Именем нажимаемой коробки.
    Named(Id),
    /// Частью видимой надписи.
    Said(String),
}

/// Место в дереве: что здесь видно и насколько содержимое сдвинуто.
///
/// Сдвиг нужен потому, что область прокрутки отдаёт обходу раскладку своих
/// детей несдвинутой (`scrollable::operate` в iced): строка, уехавшая вверх,
/// назвала бы координаты соседа. Отсечение — потому, что нажать можно только
/// видимое, и найденным считается лишь оно: назвать место уехавшего за край
/// значит пообещать сценарию невыполнимое нажатие.
#[derive(Clone, Copy)]
struct Frame {
    /// Видимая здесь часть экрана; `None` — не видно ничего.
    clip: Option<Rectangle>,
    offset: Vector,
}

/// Обход, считающий подошедших и запоминающий место названного.
pub struct Search {
    sought: Sought,
    /// Который по счёту нужен, считая с единицы; 0 — «должен быть ровно один».
    ordinal: u32,
    /// Кадры родителей, чьих детей сейчас обходят. Нижний — окно целиком.
    frames: Vec<Frame>,
    /// Кадр, заготовленный виджетом, который только что о себе сказал: в него
    /// войдут его дети. Пусто — виджет промолчал (наши обёртки так и делают),
    /// и дети остаются в кадре родителя.
    entering: Option<Frame>,
    found: u32,
    place: Option<Rectangle>,
}

impl Search {
    /// Спрошено ли о чём-нибудь. Пустая надпись подошла бы каждой строке на
    /// экране: `contains("")` истинно всегда, — а вопрос без имени не о чём.
    pub fn asks(sought: &Sought) -> bool {
        match sought {
            Sought::Named(_) => true,
            Sought::Said(said) => !said.is_empty(),
        }
    }

    /// Обход по окну размером `window` (в точках раскладки).
    pub fn new(sought: Sought, ordinal: u32, window: Size) -> Self {
        Self {
            sought,
            ordinal,
            frames: vec![Frame { clip: Some(Rectangle::with_size(window)), offset: Vector::ZERO }],
            entering: None,
            found: 0,
            place: None,
        }
    }

    /// О ком спросили.
    pub fn sought(&self) -> &Sought {
        &self.sought
    }

    /// Сколько элементов подошло под вопрос.
    pub fn found(&self) -> u32 {
        self.found
    }

    /// Место названного — в точках раскладки. `None` — столько их не набралось.
    pub fn place(&self) -> Option<Rectangle> {
        self.place
    }

    fn frame(&self) -> Frame {
        *self.frames.last().expect("нижний кадр — окно, и его не снимают")
    }

    /// Видимая часть виджета: раскладочный прямоугольник, сдвинутый прокруткой
    /// и обрезанный тем, внутри чего он лежит. `None` — не видно.
    fn visible(&self, bounds: Rectangle) -> Option<Rectangle> {
        let frame = self.frame();
        // Вырожденное пересечение `intersection` и сама отдаёт как «нет».
        (bounds - frame.offset).intersection(&frame.clip?)
    }

    /// Подошёл ещё один. Место запоминается у того, кого спросили: названного
    /// номером — у него, безномерного — у первого.
    fn matched(&mut self, seen: Rectangle) {
        self.found += 1;
        if self.found == self.ordinal.max(1) {
            self.place = Some(seen);
        }
    }
}

impl Operation for Search {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        let frame = self.entering.take().unwrap_or_else(|| self.frame());
        self.frames.push(frame);
        operate(self);
        self.frames.pop();
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
        let seen = self.visible(bounds);
        self.entering = Some(Frame { clip: seen, offset: self.frame().offset });

        if let (Sought::Named(name), Some(id), Some(seen)) = (&self.sought, id, seen)
            && id == name
        {
            self.matched(seen);
        }
    }

    fn scrollable(
        &mut self,
        _id: Option<&Id>,
        bounds: Rectangle,
        _content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn iced_core::widget::operation::Scrollable,
    ) {
        // Область видна своим местом, а её содержимое вдобавок съехало на
        // прокрутку: складываем сдвиг для детей и режем их этой областью.
        self.entering = Some(Frame {
            clip: self.visible(bounds),
            offset: self.frame().offset + translation,
        });
    }

    fn text_input(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _state: &mut dyn iced_core::widget::operation::TextInput,
    ) {
        // Поле ввода о себе как о коробке не объявляет вовсе, а надписи у него
        // нет: подсказка рисуется им самим. Без этой ветки поставить каретку
        // по имени было бы нечем — только пикселями.
        if let (Sought::Named(name), Some(id), Some(seen)) = (&self.sought, id, self.visible(bounds))
            && id == name
        {
            self.matched(seen);
        }
    }

    fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, text: &str) {
        if let Sought::Said(said) = &self.sought
            && text.contains(said.as_str())
            && let Some(seen) = self.visible(bounds)
        {
            self.matched(seen);
        }
    }
}
