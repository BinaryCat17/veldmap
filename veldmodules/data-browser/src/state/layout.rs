use super::listing::choice;

use super::types::ViewId;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct PaneId(u64);

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PaneId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(PaneId)
    }
}

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    Row,
    Column,
}

impl Axis {
    fn sides(self) -> (Side, Side) {
        match self {
            Axis::Row => (Side::Left, Side::Right),
            Axis::Column => (Side::Above, Side::Below),
        }
    }

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
    Side {
        Right = "right", "Move right";
        Left  = "left",  "Move left";
        Above = "above", "Move up";
        Below = "below", "Move down";
    }
}

impl Side {
    pub fn axis(self) -> Axis {
        match self {
            Side::Left | Side::Right => Axis::Row,
            Side::Above | Side::Below => Axis::Column,
        }
    }

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

pub enum Node {
    Leaf(Pane),
    Split(Box<Split>),
}

pub struct Pane {
    pub id: PaneId,
    pub active: Option<ViewId>,
}

pub struct Split {
    pub id: SplitId,
    pub axis: Axis,
    pub fraction: f32,
    pub first: Node,
    pub second: Node,
}

pub struct Layout {
    root: Node,
    next: u64,
}

impl Layout {
    pub fn new() -> Self {
        Self { root: Node::Leaf(Pane { id: PaneId(1), active: None }), next: 2 }
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    pub fn first(&self) -> PaneId {
        self.root.edge(Side::Left)
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.pane(pane).is_some()
    }

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

    pub fn split(&mut self, pane: PaneId, side: Side) -> Option<PaneId> {
        if !self.contains(pane) {
            return None;
        }
        let id = PaneId(self.next);
        let split = SplitId(self.next + 1);
        self.next += 2;

        let at = self.root.find_mut(pane).expect("pane is in place — checked above");
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

    pub fn remove(&mut self, pane: PaneId) -> Option<PaneId> {
        let parent = self.root.parent_of(pane)?;
        let stub = Node::Leaf(Pane { id: pane, active: None });
        let Node::Split(split) = std::mem::replace(parent, stub) else {
            unreachable!("parent_of returns only a split")
        };
        let Split { axis, first, second, .. } = *split;

        let gone_first = matches!(&first, Node::Leaf(leaf) if leaf.id == pane);
        let kept = if gone_first { second } else { first };
        let (head, tail) = axis.sides();
        let took = kept.edge(if gone_first { head } else { tail });
        *parent = kept;
        Some(took)
    }

    pub fn neighbour(&self, pane: PaneId, side: Side) -> Option<PaneId> {
        self.root.neighbour(pane, side)
    }

    pub fn collapse(&mut self, keep: PaneId) -> PaneId {
        let id = if self.contains(keep) { keep } else { self.first() };
        let active = self.active(id);
        self.root = Node::Leaf(Pane { id, active });
        id
    }

    pub fn share(&self, pane: PaneId) -> (f32, f32) {
        self.share_of(&|node| matches!(node, Node::Leaf(leaf) if leaf.id == pane))
    }

    pub fn split_share(&self, split: SplitId) -> (f32, f32) {
        self.share_of(&|node| matches!(node, Node::Split(found) if found.id == split))
    }

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

    pub fn axis_of(&self, split: SplitId) -> Option<Axis> {
        self.root.split(split).map(|found| found.axis)
    }

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

    pub fn set_fraction(&mut self, split: SplitId, fraction: f32) {
        if let Some(found) = self.root.split_mut(split) {
            found.fraction = fraction.clamp(0.0, 1.0);
        }
    }

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

    fn holds(&self, pane: PaneId) -> bool {
        let Node::Split(split) = self else { return false };
        let leaf = |node: &Node| matches!(node, Node::Leaf(leaf) if leaf.id == pane);
        leaf(&split.first) || leaf(&split.second)
    }

    fn edge(&self, side: Side) -> PaneId {
        match self {
            Node::Leaf(leaf) => leaf.id,
            Node::Split(split) if split.axis == side.axis() => match side.head() {
                true => split.first.edge(side),
                false => split.second.edge(side),
            },
            Node::Split(split) => split.first.edge(side),
        }
    }

    fn neighbour(&self, pane: PaneId, side: Side) -> Option<PaneId> {
        let Node::Split(split) = self else { return None };
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
        let (mine, theirs) = match side.head() {
            true => (&split.second, &split.first),
            false => (&split.first, &split.second),
        };
        mine.pane(pane).is_some().then(|| theirs.edge(side.opposite()))
    }
}
