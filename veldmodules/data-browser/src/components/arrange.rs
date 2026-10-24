//! components/arrange.rs — what to show and in what order.
//!
//! Filtering, sorting, paging, and grouping live in one place without a single
//! widget: these are rules, not presentation, and they are checked against
//! data, not by eye. The layout receives a ready-made list of rows (see `table`).

use crate::module::components::{format, last_segment, Row};
use crate::module::state::listing::{Filter, Grouping, ListingState};

/// How many entries fit on a page. The number is fixed, not derived from the
/// window height: a page whose content changes on window resize loses the spot
/// the user was standing on.
pub const PAGE_SIZE: usize = 20;

/// A table row after grouping.
pub enum Line<'a> {
    /// Group header: the folder and how many of its entries are shown.
    Group { title: String, meta: String, depth: usize },
    Entry { row: &'a Row, depth: usize },
    /// An expanded folder whose content is still on its way. A row of its own,
    /// not a fake entry: an entry has a key, a kind, and actions, while a wait
    /// has nothing but the space it occupies.
    Waiting { depth: usize },
}

/// The list ready for display, along with everything to know about its pages.
pub struct Arranged<'a> {
    pub lines: Vec<Line<'a>>,
    /// How many entries passed the filter — the page range label is based on it.
    pub total: usize,
    pub pages: usize,
    /// The page being shown: the one requested, but no further than the last.
    pub page: usize,
    /// How many entries sit in each state (per `Filter::ALL`) — the filter
    /// menu counters. Counted before filtering: otherwise "All" would show the
    /// same number as the selected state.
    pub counts: Vec<usize>,
}

impl Arranged<'_> {
    /// "1–20 of 36". An empty list gets an empty label: it has no range.
    pub fn range(&self) -> String {
        if self.total == 0 {
            return String::new();
        }
        let from = self.page * PAGE_SIZE + 1;
        let to = ((self.page + 1) * PAGE_SIZE).min(self.total);
        format!("{}–{} of {}", from, to, self.total)
    }

    /// The rows shown on this page, expanded subrows included. Group headers
    /// are not here: they are labels above the rows, not rows themselves.
    ///
    /// They answer every "what is visible" question: which columns to set up,
    /// what the header checkbox will mark, whether everything is already
    /// marked. There is deliberately no second answer to this question — were
    /// they to diverge, the column and its content would show different things.
    pub fn shown(&self) -> impl Iterator<Item = &Row> {
        self.lines.iter().filter_map(|line| match line {
            Line::Entry { row, .. } => Some(*row),
            Line::Group { .. } | Line::Waiting { .. } => None,
        })
    }

    /// The rows of this page that can be chosen — exactly what the header
    /// checkbox acts on.
    ///
    /// The page, not the whole filter: the checkbox stands above the visible
    /// area, and marking a hundred rows that are not on screen would also mean
    /// asking the catalogue for a hundred products — one per footprint.
    ///
    /// Rows, not keys: a choice also carries the row kind (a snapshot or a file
    /// on its own), and it could not be restored from a single key.
    pub fn marks(&self) -> impl Iterator<Item = &Row> {
        self.shown().filter(|row| row.choosable())
    }

    /// Whether everything visible is marked. Nothing to mark counts as not
    /// marked: the checkbox then offers to mark, and pressing it changes
    /// nothing.
    pub fn all_marked(&self, listing: &ListingState) -> bool {
        let mut marks = self.marks().peekable();
        marks.peek().is_some()
            && marks.all(|row| listing.selected.contains_key(row.choice_key()))
    }
}

/// The rows that passed the filter, in display order.
///
/// A shared start for the display and for every action over the whole list:
/// "mark all" marks exactly what is visible, and navigating to a row looks up
/// its page in the same order. A second copy of this filter would diverge from
/// the first exactly where it shows the most.
pub fn filtered<'a>(rows: &'a [Row], listing: &ListingState) -> Vec<&'a Row> {
    let query = listing.query.to_lowercase();
    let mut passing: Vec<&Row> =
        rows.iter().filter(|row| passes(row, &query, listing.filter)).collect();
    sort(&mut passing, listing);
    passing
}

/// Which page the row with this key stands on. `None` — the row did not pass
/// the filter, and there is nowhere to lead to it.
pub fn page_of(rows: &[Row], listing: &ListingState, key: &str) -> Option<usize> {
    let at = filtered(rows, listing).iter().position(|row| row.named(key))?;
    Some(at / PAGE_SIZE)
}

pub fn arrange<'a>(rows: &'a [Row], listing: &ListingState) -> Arranged<'a> {
    use crate::module::state::listing::Choice;

    // The menu counters are computed before filtering by state but after
    // filtering by name: otherwise "All" would show the same number as the
    // selected state.
    let query = listing.query.to_lowercase();
    let counts = Filter::ALL
        .iter()
        .map(|filter| rows.iter().filter(|row| passes(row, &query, *filter)).count())
        .collect();

    let passing = filtered(rows, listing);
    let total = passing.len();
    let pages = total.div_ceil(PAGE_SIZE).max(1);
    let page = listing.page.min(pages - 1);
    let shown = passing
        .into_iter()
        .skip(page * PAGE_SIZE)
        .take(PAGE_SIZE)
        .collect();

    Arranged { lines: group(shown, listing), total, pages, page, counts }
}

/// Whether a row passes the filter: by the typed name and the named state.
///
/// One rule for the display and for the menu counters — they are computed by
/// the same filter, each with its own state (see [`arrange`]) — while two
/// phrasings of the same thing would one day write a number into the menu that
/// differs from the list.
fn passes(row: &Row, query: &str, filter: Filter) -> bool {
    matches_query(row, query) && filter.matches(&row.status)
}

/// The name filter matches by substring, case-insensitive: the user remembers
/// a piece of the name, not its beginning. `query` is already lowercase.
fn matches_query(row: &Row, query: &str) -> bool {
    query.is_empty() || row.title.to_lowercase().contains(query)
}

fn sort(rows: &mut [&Row], listing: &ListingState) {
    use crate::module::state::listing::Sorting;
    rows.sort_by(|left, right| {
        // Under grouping, folder order outranks any selected order: otherwise
        // rows of one folder spread apart, and the header above them opens once
        // for each.
        let by_folder = match listing.grouping {
            Grouping::None => std::cmp::Ordering::Equal,
            _ => left.folder().cmp(right.folder()),
        };
        by_folder.then_with(|| match listing.sorting {
            Sorting::Newest => right.date.cmp(&left.date).then_with(|| left.title.cmp(&right.title)),
            Sorting::Name => left.title.cmp(&right.title),
            Sorting::Size => right.size.cmp(&left.size).then_with(|| left.title.cmp(&right.title)),
        })
    });
}

/// Lays the entries out into groups. Both groupings are one and the same walk:
/// "by folder" has a single step (the whole folder at once), "tree" has as many
/// as there are segments in the path. A header opens where the path diverges
/// from the previous one — the rows already stand by folder by then (see
/// `sort`).
fn group<'a>(rows: Vec<&'a Row>, listing: &ListingState) -> Vec<Line<'a>> {
    let grouping = listing.grouping;
    if grouping == Grouping::None {
        let mut lines = Vec::new();
        for row in rows {
            lines.push(Line::Entry { row, depth: 0 });
            expand(row, listing, 0, &mut lines);
        }
        return lines;
    }

    // Paths are computed once per row: they are compared both against the
    // previous row and against all the following ones — recomputing them for
    // every comparison would parse the same path a dozen times over.
    let paths: Vec<Vec<&str>> = rows.iter().map(|row| segments(row.folder(), grouping)).collect();

    let mut lines = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let path = &paths[index];
        let common = match index.checked_sub(1) {
            Some(before) => path.iter().zip(&paths[before]).take_while(|(next, previous)| next == previous).count(),
            None => 0,
        };

        for (depth, segment) in path.iter().enumerate().skip(common) {
            // The counter sits only on the deepest step: on intermediate ones
            // it would count the whole subtree, not what is shown here.
            let meta = if depth + 1 == path.len() {
                let same = paths[index..].iter().take_while(|other| *other == path).count();
                format!("{} {}", same, format::plural(same, ["file", "files", "files"]))
            } else {
                String::new()
            };
            lines.push(Line::Group { title: (*segment).to_string(), meta, depth });
        }

        lines.push(Line::Entry { row, depth: path.len() });
        expand(row, listing, path.len(), &mut lines);
    }
    lines
}

/// The content of an expanded row — as subrows under it. One step deeper, the
/// same `Line::Entry`: a subrow is a row like any other, and drawing it as a
/// separate kind would mean a second table.
///
/// All the way down, not one level: an expanded folder inside an expanded one
/// is routine, and there is no second rule for it.
///
/// Filtering and sorting do not reach them: the content shows what the row is
/// made of rather than taking part in the list alongside it — otherwise
/// "showing 20 of 36" would count different things depending on what is
/// expanded.
fn expand<'a>(row: &'a Row, listing: &ListingState, depth: usize, lines: &mut Vec<Line<'a>>) {
    if !listing.expanded.contains(row.key()) {
        return;
    }
    // The answer is still on its way — and it says so. Empty space under an
    // expanded folder reads as "there is nothing here", which is untrue right
    // up to the end of the listing.
    if row.loading {
        lines.push(Line::Waiting { depth: depth + 1 });
    }
    for child in &row.children {
        lines.push(Line::Entry { row: child, depth: depth + 1 });
        expand(child, listing, depth + 1, lines);
    }
}

/// The steps this folder opens.
fn segments(folder: &str, grouping: Grouping) -> Vec<&str> {
    match grouping {
        // The header is named after the folder itself, with no path shown: the
        // path is the same for every row of the kind and does not earn space in
        // the header. Root rows have no folder at all — no header opens for
        // them.
        Grouping::Folder => match last_segment(folder) {
            "" => Vec::new(),
            name => vec![name],
        },
        Grouping::Tree => folder.split('/').filter(|name| !name.is_empty()).collect(),
        Grouping::None => Vec::new(),
    }
}
