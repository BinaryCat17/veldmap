//! Раскладка окна: дерево панелей.
//!
//! Панель — место, в котором лежат вкладки. Какие именно, дерево не знает: у
//! вида есть поле `pane` (см. `View`), и «в какой панели лежит вкладка» имеет
//! один ответ. Дерево отвечает на другое — где панель стоит и сколько ей места.
//!
//! Дерево, а не набор половин: «поделить надвое» — не единственный жест,
//! которым складывают экран. Список слева, шар справа сверху, снимок под
//! ним — это два деления, вложенных одно в другое, и выразить их парой
//! половин нечем. Узел поэтому один и тот же в обоих положениях: лист с
//! вкладками либо деление на два узла с осью и долей. Всё остальное —
//! перенос, бросок, доля границы, сохранение — сводится к нескольким
//! действиям над ним и потому лежит здесь целиком.

use super::listing::choice;

use super::types::ViewId;

/// Идентификатор панели. Уникален в пределах сессии и не переиспользуется — по
/// той же причине, что и [`ViewId`]: панель называют события разметки («плюс»
/// этой панели, бросок в неё), а к тому времени, как событие доедет, панели
/// может уже не быть — и переиспользованный номер отдал бы событие чужой.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct PaneId(u64);

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Панель возвращает свой номер строкой в `UiEventResponse.key` — разобрать его
/// обратно нужно там же, где принимается нажатие.
impl std::str::FromStr for PaneId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(PaneId)
    }
}

/// Идентификатор деления — его называет граница, которую тянут мышью. Из того
/// же счётчика, что и [`PaneId`]: номера в дереве не повторяются, и «панель 3»
/// с «делением 3» в логе не встретятся.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct SplitId(u64);

impl std::fmt::Display for SplitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for SplitId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(SplitId)
    }
}

/// Вдоль чего стоят дети деления. Названа по тому, чем деление собирается в
/// разметке (`row!` и `column!`), — иначе «горизонтальное деление» пришлось бы
/// каждый раз уточнять: горизонтальна в нём граница или расстановка.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    /// Дети рядом, граница между ними вертикальная.
    Row,
    /// Дети друг под другом, граница горизонтальная.
    Column,
}

impl Axis {
    /// Стороны этой оси: со стороны первого ребёнка и со стороны второго.
    fn sides(self) -> (Side, Side) {
        match self {
            Axis::Row => (Side::Left, Side::Right),
            Axis::Column => (Side::Above, Side::Below),
        }
    }

    /// Сторона, с которой встаёт второй ребёнок, — ею деление и заводят
    /// заново, когда раскладку собирают из сохранённой.
    pub fn tail(self) -> Side {
        self.sides().1
    }

    pub fn key(self) -> &'static str {
        match self {
            Axis::Row => "row",
            Axis::Column => "column",
        }
    }

    pub fn from_key(key: &str) -> Option<Axis> {
        match key {
            "row" => Some(Axis::Row),
            "column" => Some(Axis::Column),
            _ => None,
        }
    }
}

choice! {
    /// Сторона, с которой встаёт панель. Ею же называют направление переноса
    /// («перенести вправо») и край, за который взялись броском.
    ///
    /// Порядок здесь — порядок пунктов в меню переноса; подпись живёт рядом с
    /// самой стороной, потому что сторона и то, как её называют человеку, —
    /// одна вещь, и разъехаться им нельзя.
    Side {
        Right = "right", "Перенести вправо";
        Left  = "left",  "Перенести влево";
        Above = "above", "Перенести вверх";
        Below = "below", "Перенести вниз";
    }
}

impl Side {
    pub fn axis(self) -> Axis {
        match self {
            Side::Left | Side::Right => Axis::Row,
            Side::Above | Side::Below => Axis::Column,
        }
    }

    /// Со стороны первого ребёнка ли она. Одним вопросом отвечает и на «куда
    /// встаёт новая панель», и на «в какой ветке искать соседа».
    fn head(self) -> bool {
        matches!(self, Side::Left | Side::Above)
    }

    pub fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Above => Side::Below,
            Side::Below => Side::Above,
        }
    }

}

/// Узел дерева: панель либо деление надвое.
pub enum Node {
    Leaf(Pane),
    /// В коробке, потому что тип рекурсивный.
    Split(Box<Split>),
}

pub struct Pane {
    pub id: PaneId,
    /// Вкладка, показанная в этой панели. `None` — панель пуста; так бывает
    /// ровно в одном случае: она последняя, и в ней закрыли всё (опустевшая
    /// панель, у которой есть соседи, из дерева убирается).
    pub active: Option<ViewId>,
}

pub struct Split {
    pub id: SplitId,
    pub axis: Axis,
    /// Доля первого ребёнка, от 0 до 1. Второму достаётся остальное — держать
    /// две доли значило бы держать правило «в сумме единица», а его однажды
    /// нарушат.
    pub fraction: f32,
    pub first: Node,
    pub second: Node,
}

pub struct Layout {
    root: Node,
    next: u64,
}

impl Layout {
    /// Одна панель во весь экран — с неё начинается всякое окно.
    pub fn new() -> Self {
        Self { root: Node::Leaf(Pane { id: PaneId(1), active: None }), next: 2 }
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    /// Первая панель обхода — та, что стоит в начале.
    pub fn first(&self) -> PaneId {
        self.root.edge(Side::Left)
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.pane(pane).is_some()
    }

    /// Сколько всего панелей. По нему решается, есть ли что сводить.
    pub fn count(&self) -> usize {
        self.root.count()
    }

    pub fn active(&self, pane: PaneId) -> Option<ViewId> {
        self.root.pane(pane)?.active
    }

    pub fn set_active(&mut self, pane: PaneId, view: Option<ViewId>) {
        if let Some(found) = self.root.pane_mut(pane) {
            found.active = view;
        }
    }

    /// Заводит панель с названной стороны от этой и отдаёт её. Прежняя остаётся
    /// на месте со всем, что в ней лежит: делят экран затем, чтобы рядом с
    /// показанным встало второе, а не затем, чтобы показанное уехало.
    pub fn split(&mut self, pane: PaneId, side: Side) -> Option<PaneId> {
        if !self.contains(pane) {
            return None;
        }
        let id = PaneId(self.next);
        let split = SplitId(self.next + 1);
        self.next += 2;

        let at = self.root.find_mut(pane).expect("панель на месте — проверено выше");
        // Место листа занимает деление, а сам лист становится его ребёнком,
        // поэтому вынимается он целиком. Заглушка — та же панель без вкладки, и
        // живёт она ровно до присваивания ниже (та же уловка, что в
        // [`Self::remove`]): второй лист с новым номером на это мгновение сделал
        // бы номер в дереве неуникальным.
        let old = std::mem::replace(at, Node::Leaf(Pane { id: pane, active: None }));
        let fresh = Node::Leaf(Pane { id, active: None });
        let (first, second) = if side.head() { (fresh, old) } else { (old, fresh) };
        *at = Node::Split(Box::new(Split {
            id: split,
            axis: side.axis(),
            fraction: 0.5,
            first,
            second,
        }));
        Some(id)
    }

    /// Убирает опустевшую панель; на её место встаёт то, что делило с ней
    /// родителя. Отдаёт панель, которой досталось освободившееся место, — ей и
    /// переходит взгляд. `None` — панель последняя: убрать её некуда, и пустая
    /// она законна.
    pub fn remove(&mut self, pane: PaneId) -> Option<PaneId> {
        let parent = self.root.parent_of(pane)?;
        // Место разделения занимает уцелевшая ветка, поэтому вынимается оно
        // целиком: заглушка живёт ровно до присваивания ниже.
        let stub = Node::Leaf(Pane { id: pane, active: None });
        let Node::Split(split) = std::mem::replace(parent, stub) else {
            unreachable!("parent_of отдаёт только деление")
        };
        let Split { axis, first, second, .. } = *split;

        let gone_first = matches!(&first, Node::Leaf(leaf) if leaf.id == pane);
        let kept = if gone_first { second } else { first };
        // Взгляд переходит к той панели, которой досталось освободившееся
        // место, — то есть к стоявшей вплотную к убранной.
        let (head, tail) = axis.sides();
        let took = kept.edge(if gone_first { head } else { tail });
        *parent = kept;
        Some(took)
    }

    /// Соседняя панель с названной стороны. `None` — с этой стороны ничего нет.
    ///
    /// «Соседняя» — ближайшая по краю: у деления вдоль этой же оси сосед лежит
    /// в другой его ветке, и берётся тот её лист, что стоит вплотную к границе.
    /// Вдоль самой границы панелей может стоять несколько, и «той самой» среди
    /// них нет — берётся первая (см. [`Node::edge`]).
    pub fn neighbour(&self, pane: PaneId, side: Side) -> Option<PaneId> {
        self.root.neighbour(pane, side)
    }

    /// Сводит всё в одну панель — названную, если она ещё жива. Вкладки при
    /// этом никуда не деваются: их переносит `State`, а дерево знает только
    /// места.
    pub fn collapse(&mut self, keep: PaneId) -> PaneId {
        let id = if self.contains(keep) { keep } else { self.first() };
        let active = self.active(id);
        self.root = Node::Leaf(Pane { id, active });
        id
    }

    /// Доля окна, доставшаяся панели, — по ширине и по высоте. Ею меряется
    /// место под колонки списка: считать его по всему окну нельзя, а по
    /// половине — неправда, пока панелей не ровно две.
    pub fn share(&self, pane: PaneId) -> (f32, f32) {
        self.share_of(&|node| matches!(node, Node::Leaf(leaf) if leaf.id == pane))
    }

    /// Она же для деления — доля окна под ним обоими детьми сразу. По ней сдвиг
    /// границы в точках становится сдвигом доли (см. `State::divide`).
    pub fn split_share(&self, split: SplitId) -> (f32, f32) {
        self.share_of(&|node| matches!(node, Node::Split(found) if found.id == split))
    }

    /// Доля окна под первым узлом, который узнаёт условие. Обход один на оба
    /// вопроса: доли перемножаются по пути, и считать их двумя способами
    /// значило бы однажды посчитать по-разному.
    fn share_of(&self, hit: &dyn Fn(&Node) -> bool) -> (f32, f32) {
        fn walk(
            node: &Node,
            hit: &dyn Fn(&Node) -> bool,
            share: (f32, f32),
        ) -> Option<(f32, f32)> {
            if hit(node) {
                return Some(share);
            }
            let Node::Split(split) = node else { return None };
            let (head, tail) = (split.fraction, 1.0 - split.fraction);
            let (first, second) = match split.axis {
                Axis::Row => ((share.0 * head, share.1), (share.0 * tail, share.1)),
                Axis::Column => ((share.0, share.1 * head), (share.0, share.1 * tail)),
            };
            walk(&split.first, hit, first).or_else(|| walk(&split.second, hit, second))
        }
        walk(&self.root, hit, (1.0, 1.0)).unwrap_or((1.0, 1.0))
    }

    /// Вдоль чего делит названное деление. `None` — его уже нет: раскладку
    /// успели сложить иначе, пока событие шло.
    pub fn axis_of(&self, split: SplitId) -> Option<Axis> {
        self.root.split(split).map(|found| found.axis)
    }

    /// Деление, у которого эта панель — прямой ребёнок. `None` — панель одна на
    /// всё окно, делить её место не с кем.
    pub fn split_of(&self, pane: PaneId) -> Option<SplitId> {
        fn walk(node: &Node, pane: PaneId) -> Option<SplitId> {
            let Node::Split(split) = node else { return None };
            if node.holds(pane) {
                return Some(split.id);
            }
            walk(&split.first, pane).or_else(|| walk(&split.second, pane))
        }
        walk(&self.root, pane)
    }

    /// Ставит долю прямо — так раскладка собирается из сохранённой. Живое
    /// перетаскивание идёт через [`Self::nudge`]: там доля меняется сдвигом, и
    /// пределы у неё свои.
    pub fn set_fraction(&mut self, split: SplitId, fraction: f32) {
        if let Some(found) = self.root.split_mut(split) {
            found.fraction = fraction.clamp(0.0, 1.0);
        }
    }

    /// Двигает границу: доля первого ребёнка меняется на `by`. Ни один из детей
    /// при этом не становится у́же `min` — панель уже этого не показывает ни
    /// вкладок, ни того, что в них, а вернуть ей место можно только этой же
    /// границей, за которую уже не взяться.
    pub fn nudge(&mut self, split: SplitId, by: f32, min: f32) {
        let Some(found) = self.root.split_mut(split) else { return };
        let edge = min.clamp(0.0, 0.5);
        found.fraction = (found.fraction + by).clamp(edge, 1.0 - edge);
    }
}

impl Node {
    fn count(&self) -> usize {
        match self {
            Node::Leaf(_) => 1,
            Node::Split(split) => split.first.count() + split.second.count(),
        }
    }

    fn pane(&self, id: PaneId) -> Option<&Pane> {
        match self {
            Node::Leaf(leaf) => (leaf.id == id).then_some(leaf),
            Node::Split(split) => split.first.pane(id).or_else(|| split.second.pane(id)),
        }
    }

    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        match self {
            Node::Leaf(leaf) => (leaf.id == id).then_some(leaf),
            Node::Split(split) => {
                let Split { first, second, .. } = &mut **split;
                match first.pane(id).is_some() {
                    true => first.pane_mut(id),
                    false => second.pane_mut(id),
                }
            }
        }
    }

    fn split(&self, id: SplitId) -> Option<&Split> {
        let Node::Split(split) = self else { return None };
        if split.id == id {
            return Some(split);
        }
        split.first.split(id).or_else(|| split.second.split(id))
    }

    fn split_mut(&mut self, id: SplitId) -> Option<&mut Split> {
        let Node::Split(split) = self else { return None };
        if split.id == id {
            return Some(split);
        }
        let Split { first, second, .. } = &mut **split;
        match first.split(id).is_some() {
            true => first.split_mut(id),
            false => second.split_mut(id),
        }
    }

    /// Узел, которым эта панель лежит в дереве.
    fn find_mut(&mut self, pane: PaneId) -> Option<&mut Node> {
        if matches!(self, Node::Leaf(leaf) if leaf.id == pane) {
            return Some(self);
        }
        match self {
            Node::Leaf(_) => None,
            Node::Split(split) => {
                let Split { first, second, .. } = &mut **split;
                match first.pane(pane).is_some() {
                    true => first.find_mut(pane),
                    false => second.find_mut(pane),
                }
            }
        }
    }

    /// Деление, у которого эта панель — прямой ребёнок.
    fn parent_of(&mut self, pane: PaneId) -> Option<&mut Node> {
        if self.holds(pane) {
            return Some(self);
        }
        match self {
            Node::Leaf(_) => None,
            Node::Split(split) => {
                let Split { first, second, .. } = &mut **split;
                match first.pane(pane).is_some() {
                    true => first.parent_of(pane),
                    false => second.parent_of(pane),
                }
            }
        }
    }

    /// Лежит ли панель прямо в этом делении, отдельным листом.
    fn holds(&self, pane: PaneId) -> bool {
        let Node::Split(split) = self else { return false };
        let leaf = |node: &Node| matches!(node, Node::Leaf(leaf) if leaf.id == pane);
        leaf(&split.first) || leaf(&split.second)
    }

    /// Панель, стоящая у названного края этого поддерева.
    fn edge(&self, side: Side) -> PaneId {
        match self {
            Node::Leaf(leaf) => leaf.id,
            // Вдоль своей оси у края стоит ближняя ветка.
            Node::Split(split) if split.axis == side.axis() => match side.head() {
                true => split.first.edge(side),
                false => split.second.edge(side),
            },
            // Поперёк — обе, и «той самой» среди них нет: берётся первая.
            Node::Split(split) => split.first.edge(side),
        }
    }

    fn neighbour(&self, pane: PaneId, side: Side) -> Option<PaneId> {
        let Node::Split(split) = self else { return None };
        // Глубже — ближе: сосед находится у самого мелкого деления, которое
        // разделяет с этой стороны, поэтому дети спрашиваются раньше себя.
        let deeper = split
            .first
            .neighbour(pane, side)
            .or_else(|| split.second.neighbour(pane, side));
        if deeper.is_some() {
            return deeper;
        }
        if split.axis != side.axis() {
            return None;
        }
        // Сосед справа есть у того, кто сам слева: панель должна лежать в
        // ветке, противоположной названной стороне.
        let (mine, theirs) = match side.head() {
            true => (&split.second, &split.first),
            false => (&split.first, &split.second),
        };
        mine.pane(pane).is_some().then(|| theirs.edge(side.opposite()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Экран, сложенный как список слева и два вида справа друг под другом, —
    /// то, ради чего дерево и заведено: парой половин это не выражается.
    fn nested() -> (Layout, PaneId, PaneId, PaneId) {
        let mut layout = Layout::new();
        let left = layout.first();
        let right = layout.split(left, Side::Right).expect("панель жива");
        let under = layout.split(right, Side::Below).expect("панель жива");
        (layout, left, right, under)
    }

    #[test]
    fn split_puts_the_new_pane_on_the_named_side() {
        let (layout, left, right, under) = nested();
        assert_eq!(layout.count(), 3);
        assert_eq!(layout.first(), left);
        assert_eq!(layout.neighbour(left, Side::Right), Some(right));
        assert_eq!(layout.neighbour(right, Side::Below), Some(under));
        assert_eq!(layout.neighbour(under, Side::Above), Some(right));
    }

    /// Соседа ищут по краю, а не по дереву: слева от нижней правой панели
    /// стоит список, хотя общий родитель у них через деление.
    #[test]
    fn neighbour_crosses_nesting() {
        let (layout, left, _, under) = nested();
        assert_eq!(layout.neighbour(under, Side::Left), Some(left));
        assert_eq!(layout.neighbour(left, Side::Left), None);
        assert_eq!(layout.neighbour(left, Side::Above), None);
    }

    /// Убранная панель отдаёт место соседу, а не разваливает дерево.
    #[test]
    fn remove_hands_the_space_to_the_neighbour() {
        let (mut layout, left, right, under) = nested();
        assert_eq!(layout.remove(right), Some(under));
        assert_eq!(layout.count(), 2);
        assert_eq!(layout.neighbour(left, Side::Right), Some(under));

        assert_eq!(layout.remove(under), Some(left));
        assert_eq!(layout.count(), 1);
        // Последняя панель не убирается: убрать её некуда.
        assert_eq!(layout.remove(left), None);
        assert!(layout.contains(left));
    }

    #[test]
    fn collapse_keeps_the_named_pane() {
        let (mut layout, _, _, under) = nested();
        assert_eq!(layout.collapse(under), under);
        assert_eq!(layout.count(), 1);
        assert_eq!(layout.first(), under);
    }

    /// Доли перемножаются по пути: нижняя правая панель — половина ширины и
    /// половина высоты.
    #[test]
    fn share_multiplies_along_the_path() {
        let (layout, left, _, under) = nested();
        assert_eq!(layout.share(left), (0.5, 1.0));
        assert_eq!(layout.share(under), (0.5, 0.5));
    }

    /// Граница двигает долю своего деления и не даёт панелям схлопнуться:
    /// у панели нулевой ширины уже не за что взяться, чтобы вернуть ей место.
    #[test]
    fn nudge_moves_the_border_within_its_limits() {
        let mut layout = Layout::new();
        let left = layout.first();
        layout.split(left, Side::Right);
        let Node::Split(split) = layout.root() else { panic!("экран поделён") };
        let (id, axis) = (split.id, split.axis);
        assert_eq!(axis, Axis::Row);

        layout.nudge(id, 0.25, 0.1);
        assert_eq!(layout.share(left), (0.75, 1.0));

        // Упор: дальше границу не пускает предел, а не арифметика.
        layout.nudge(id, 10.0, 0.1);
        assert_eq!(layout.share(left), (0.9, 1.0));
        layout.nudge(id, -10.0, 0.1);
        assert_eq!(layout.share(left), (0.1, 1.0));
    }

    /// Номера панелей не переиспользуются: событие, приехавшее от закрытой,
    /// иначе досталось бы новой.
    #[test]
    fn pane_ids_are_not_reused() {
        let (mut layout, left, right, under) = nested();
        layout.remove(right);
        let fresh = layout.split(left, Side::Right).expect("панель жива");
        assert_ne!(fresh, right);
        assert_ne!(fresh, under);
    }
}
