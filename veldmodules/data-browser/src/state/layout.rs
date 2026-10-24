//! Window layout: a tree of panes.
//!
//! A pane is a place that holds tabs. Which tabs, the tree does not know: the
//! view has a `pane` field (see `View`), and "which pane holds the tab" has
//! exactly one answer. The tree answers a different question — where a pane
//! stands and how much space it gets.
//!
//! A tree, not a set of halves: "split in two" is not the only gesture used to
//! compose a screen. A list on the left, the globe top right, a snapshot under
//! it — that is two splits nested one inside the other, and a pair of halves
//! cannot express it. The node is therefore the same in both positions: a leaf
//! with tabs, or a split into two nodes with an axis and a share. Everything
//! else — moving a tab, a drop, the border share, persistence — reduces to a
//! few operations on it and so lives here entirely.

use super::listing::choice;

use super::types::ViewId;

/// Pane identifier. Unique within a session and never reused — for the same
/// reason as [`ViewId`]: layout events name the pane ("plus" of this pane, a
/// drop into it), and by the time the event arrives the pane may already be
/// gone — a reused number would hand the event to a stranger.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct PaneId(u64);

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A pane returns its number as a string in `UiEventResponse.key` — parsing it
/// back happens where the click is handled.
impl std::str::FromStr for PaneId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(PaneId)
    }
}

/// Split identifier — named by the border dragged with the mouse. From the
/// same counter as [`PaneId`]: numbers in the tree never repeat, so "pane 3"
/// and "split 3" never meet in the log.
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

/// Along which direction the children of a split stand. Named after the layout
/// macros a split is built with (`row!` and `column!`) — otherwise "horizontal
/// split" would need constant clarification: whether the border in it is
/// horizontal or the arrangement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    /// Children side by side; the border between them is vertical.
    Row,
    /// Children one under another; the border is horizontal.
    Column,
}

impl Axis {
    /// The sides of this axis: on the first child's side and on the second's.
    fn sides(self) -> (Side, Side) {
        match self {
            Axis::Row => (Side::Left, Side::Right),
            Axis::Column => (Side::Above, Side::Below),
        }
    }

    /// The side the second child stands on — it is how a split is recreated
    /// when the layout is assembled from a saved one.
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
    /// The side a pane stands on. The same word names the move direction
    /// ("move right") and the edge grabbed by a drop.
    ///
    /// The order here is the order of items in the move menu; the label lives
    /// next to the side itself because the side and how it is shown to a person
    /// are one thing and must not drift apart.
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

    /// Whether it is on the first child's side. One question answers both
    /// "where a new pane stands" and "which branch to search for a neighbour".
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

/// Tree node: a pane or a split in two.
pub enum Node {
    Leaf(Pane),
    /// Boxed because the type is recursive.
    Split(Box<Split>),
}

pub struct Pane {
    pub id: PaneId,
    /// The tab shown in this pane. `None` — the pane is empty; this happens in
    /// exactly one case: it is the last pane and everything in it was closed
    /// (an emptied pane that has neighbours is removed from the tree).
    pub active: Option<ViewId>,
}

pub struct Split {
    pub id: SplitId,
    pub axis: Axis,
    /// The first child's share, from 0 to 1. The second gets the rest — keeping
    /// two shares would mean keeping a "sum to one" rule that would eventually
    /// be broken.
    pub fraction: f32,
    pub first: Node,
    pub second: Node,
}

pub struct Layout {
    root: Node,
    next: u64,
}

impl Layout {
    /// One pane filling the screen — every window starts with it.
    pub fn new() -> Self {
        Self { root: Node::Leaf(Pane { id: PaneId(1), active: None }), next: 2 }
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    /// The first pane of the traversal — the one at the start.
    pub fn first(&self) -> PaneId {
        self.root.edge(Side::Left)
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.pane(pane).is_some()
    }

    /// How many panes there are. It decides whether there is anything to
    /// collapse.
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

    /// Creates a pane on the named side of this one and returns it. The former
    /// stays in place with everything in it: the screen is split so that a
    /// second stands beside what is shown, not so the shown one moves away.
    pub fn split(&mut self, pane: PaneId, side: Side) -> Option<PaneId> {
        if !self.contains(pane) {
            return None;
        }
        let id = PaneId(self.next);
        let split = SplitId(self.next + 1);
        self.next += 2;

        let at = self.root.find_mut(pane).expect("pane is in place — checked above");
        // A split takes the leaf's place and the leaf itself becomes its child,
        // so it is taken out whole. The stub is the same pane without a tab, and
        // it lives exactly until the assignment below (the same trick as in
        // [`Self::remove`]): a second leaf with a new number would make the tree
        // number non-unique for that instant.
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

    /// Removes an emptied pane; what split the parent with it takes its place.
    /// Returns the pane that received the freed space — the focus moves to it.
    /// `None` — the pane is the last one: there is nowhere to remove it, and an
    /// empty one is legal.
    pub fn remove(&mut self, pane: PaneId) -> Option<PaneId> {
        let parent = self.root.parent_of(pane)?;
        // The surviving branch takes the split's place, so the split is taken
        // out whole: the stub lives exactly until the assignment below.
        let stub = Node::Leaf(Pane { id: pane, active: None });
        let Node::Split(split) = std::mem::replace(parent, stub) else {
            unreachable!("parent_of returns only a split")
        };
        let Split { axis, first, second, .. } = *split;

        let gone_first = matches!(&first, Node::Leaf(leaf) if leaf.id == pane);
        let kept = if gone_first { second } else { first };
        // The focus moves to the pane that received the freed space — that is,
        // the one that stood right next to the removed one.
        let (head, tail) = axis.sides();
        let took = kept.edge(if gone_first { head } else { tail });
        *parent = kept;
        Some(took)
    }

    /// The neighbouring pane on the named side. `None` — there is nothing on
    /// this side.
    ///
    /// "Neighbouring" is closest along the edge: at a split along the same axis
    /// the neighbour lies in its other branch, taking the leaf that stands right
    /// at the border. Along the border itself several panes can stand, and there
    /// is no "the one" among them — the first is taken (see [`Node::edge`]).
    pub fn neighbour(&self, pane: PaneId, side: Side) -> Option<PaneId> {
        self.root.neighbour(pane, side)
    }

    /// Collapses everything into one pane — the named one, if it is still alive.
    /// Tabs are not lost in the process: `State` moves them, while the tree
    /// knows only places.
    pub fn collapse(&mut self, keep: PaneId) -> PaneId {
        let id = if self.contains(keep) { keep } else { self.first() };
        let active = self.active(id);
        self.root = Node::Leaf(Pane { id, active });
        id
    }

    /// The share of the window given to a pane — in width and in height. It
    /// measures the space for list columns: counting by the whole window is
    /// wrong, and by a half is untrue unless there are exactly two panes.
    pub fn share(&self, pane: PaneId) -> (f32, f32) {
        self.share_of(&|node| matches!(node, Node::Leaf(leaf) if leaf.id == pane))
    }

    /// The same for a split — the window share under it with both children at
    /// once. Through it a border drag in pixels becomes a share change (see
    /// `State::divide`).
    pub fn split_share(&self, split: SplitId) -> (f32, f32) {
        self.share_of(&|node| matches!(node, Node::Split(found) if found.id == split))
    }

    /// The window share under the first node matching the condition. One walk
    /// for both questions: shares multiply along the path, and computing them
    /// two ways would eventually disagree.
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

    /// What axis the named split divides along. `None` — it is already gone: the
    /// layout was rearranged while the event was in flight.
    pub fn axis_of(&self, split: SplitId) -> Option<Axis> {
        self.root.split(split).map(|found| found.axis)
    }

    /// The split whose direct child this pane is. `None` — the pane is alone in
    /// the whole window; there is nobody to share its space with.
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

    /// Sets the share directly — this is how a layout is assembled from a saved
    /// one. Live dragging goes through [`Self::nudge`]: there the share changes
    /// by a delta, and its limits are its own.
    pub fn set_fraction(&mut self, split: SplitId, fraction: f32) {
        if let Some(found) = self.root.split_mut(split) {
            found.fraction = fraction.clamp(0.0, 1.0);
        }
    }

    /// Moves the border: the first child's share changes by `by`. Neither child
    /// becomes narrower than `min` — such a pane already shows neither tabs nor
    /// their content, and the space can only be given back by the same border,
    /// which then cannot be grabbed.
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

    /// The node this pane occupies in the tree.
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

    /// The split whose direct child this pane is.
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

    /// Whether the pane lies directly in this split as a separate leaf.
    fn holds(&self, pane: PaneId) -> bool {
        let Node::Split(split) = self else { return false };
        let leaf = |node: &Node| matches!(node, Node::Leaf(leaf) if leaf.id == pane);
        leaf(&split.first) || leaf(&split.second)
    }

    /// The pane standing at the named edge of this subtree.
    fn edge(&self, side: Side) -> PaneId {
        match self {
            Node::Leaf(leaf) => leaf.id,
            // Along its own axis the near branch stands at the edge.
            Node::Split(split) if split.axis == side.axis() => match side.head() {
                true => split.first.edge(side),
                false => split.second.edge(side),
            },
            // Across the axis both stand at the edge, and there is no "the one"
            // among them: the first is taken.
            Node::Split(split) => split.first.edge(side),
        }
    }

    fn neighbour(&self, pane: PaneId, side: Side) -> Option<PaneId> {
        let Node::Split(split) = self else { return None };
        // Deeper is closer: the neighbour is found at the smallest split that
        // separates on this side, so children are asked before self.
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
        // A right neighbour belongs to one who is itself on the left: the pane
        // must lie in the branch opposite the named side.
        let (mine, theirs) = match side.head() {
            true => (&split.second, &split.first),
            false => (&split.first, &split.second),
        };
        mine.pane(pane).is_some().then(|| theirs.edge(side.opposite()))
    }
}
