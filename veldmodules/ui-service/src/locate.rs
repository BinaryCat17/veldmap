//! Обход разметки: где на экране элемент, названный не пикселями.
//!
//! Прогон по сценарию называет элемент тем же, чем его называет разметка, —
//! обработчиком нажимаемой коробки или частью видимой надписи. Перевести это в
//! прямоугольник может только тот, кто считает раскладку: сама разметка о
//! координатах не знает вовсе, и спрашивают поэтому нас.
//!
//! Имя доезжает до дерева iced единственным способом — `Id`, — и вешает его
//! converter в двух местах: на коробку внутри нажимаемой кнопки и на поле
//! ввода. Обход сверяет его на равенство; прочесть имя обратно из `Id` нельзя,
//! да и незачем — спрашивают всегда про одно.

use std::cell::Cell;

use iced_core::widget::{Id, Operation};
use iced_core::{Rectangle, Size, Vector};

/// Чем называют коробки, пока идёт прогон по сценарию.
///
/// Вид имени задаёт сам вопрос, и иначе нельзя: прочесть имя обратно из `Id`
/// нечем — сверяется оно только на равенство. Значит «этот обработчик с любой
/// нагрузкой» выражается единственным способом: нагрузку в имя не кладут
/// вовсе.
///
/// Вид один на все экраны, а незакрытый вопрос — у каждого свой, так что вопрос
/// к экрану, который в это время не рисуется, будет отвечен по чужому виду
/// имён. Ответом при этом будет «не нашлось»: имя искомого спрашивающий
/// складывает сам (`spelling`), и с чужим оно не совпадёт.
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
    /// Как называют коробки. Взводится вопросом и держится до следующего:
    /// спросивший однажды спросит и дальше, а обход по дереву без имён ответил
    /// бы «нет такого» вместо «не успели назвать».
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
        Naming::WithValue => Some(spelling(method, value)),
    }
}

/// Как пишется имя искомого — по самому вопросу, не заглядывая в то, чем
/// названо дерево. Спрашивающему нужно именно это: сложи он имя из общего
/// вида, и вопрос, дождавшийся своего кадра позже другого, был бы отвечен по
/// чужой мерке.
pub fn spelling(method: &str, value: &str) -> String {
    match value.is_empty() {
        true => method.to_string(),
        false => format!("{}\u{1}{}", method, value),
    }
}

/// Метка, которой всплывающая панель объявляет обходу своё место.
///
/// Панель не занимает места в разметке — её показывает оверлей, — и обход
/// увидел бы под ней то, что нажать нельзя: щелчок мимо панели её закрывает, а
/// не жмёт лежащее под ней (см. `popover.rs`). Объявляет она себя сама, до
/// спуска внутрь, потому что снаружи о ней не знает никто.
pub struct Covered;

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
    /// Места подошедших, в порядке обхода. Списком, а не счётом: накрывшая их
    /// панель объявляется уже после того, как они найдены, и снять из-под неё
    /// можно только то, что помнишь.
    matched: Vec<Rectangle>,
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
            matched: Vec::new(),
        }
    }

    /// О ком спросили.
    pub fn sought(&self) -> &Sought {
        &self.sought
    }

    /// Сколько элементов подошло под вопрос.
    pub fn found(&self) -> u32 {
        self.matched.len() as u32
    }

    /// Место названного — в точках раскладки. `None` — столько их не набралось.
    pub fn place(&self) -> Option<Rectangle> {
        let which = self.ordinal.max(1) as usize;
        self.matched.get(which - 1).copied()
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

    /// Подошёл ещё один.
    fn hit(&mut self, seen: Rectangle) {
        self.matched.push(seen);
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
            self.hit(seen);
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
            self.hit(seen);
        }
    }

    fn custom(&mut self, _id: Option<&Id>, bounds: Rectangle, state: &mut dyn std::any::Any) {
        if !state.is::<Covered>() {
            return;
        }
        // Найденное под панелью снимается: нажать его нельзя, пока она открыта,
        // и назвать его найденным значило бы пообещать сценарию невыполнимое.
        // Панель объявляется до спуска внутрь себя, поэтому её собственные
        // пункты, найденные ниже, под этот отбор уже не попадают.
        self.matched.retain(|seen| !bounds.contains(seen.center()));
    }

    fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, text: &str) {
        if let Sought::Said(said) = &self.sought
            && text.contains(said.as_str())
            && let Some(seen) = self.visible(bounds)
        {
            self.hit(seen);
        }
    }
}
