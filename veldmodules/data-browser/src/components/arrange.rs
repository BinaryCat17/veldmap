use crate::module::components::{format, last_segment, Row};
use crate::module::state::listing::{Filter, Grouping, ListingState};

pub const PAGE_SIZE: usize = 20;

pub enum Line<'a> {
    Group { title: String, meta: String, depth: usize },
    Entry { row: &'a Row, depth: usize },
    Waiting { depth: usize },
}

pub struct Arranged<'a> {
    pub lines: Vec<Line<'a>>,
    pub total: usize,
    pub pages: usize,
    pub page: usize,
    pub counts: Vec<usize>,
}

impl Arranged<'_> {
    pub fn range(&self) -> String {
        if self.total == 0 {
            return String::new();
        }
        let from = self.page * PAGE_SIZE + 1;
        let to = ((self.page + 1) * PAGE_SIZE).min(self.total);
        format!("{}–{} of {}", from, to, self.total)
    }

    pub fn shown(&self) -> impl Iterator<Item = &Row> {
        self.lines.iter().filter_map(|line| match line {
            Line::Entry { row, .. } => Some(*row),
            Line::Group { .. } | Line::Waiting { .. } => None,
        })
    }

    pub fn marks(&self) -> impl Iterator<Item = &Row> {
        self.shown().filter(|row| row.choosable())
    }

    pub fn all_marked(&self, listing: &ListingState) -> bool {
        let mut marks = self.marks().peekable();
        marks.peek().is_some()
            && marks.all(|row| listing.selected.contains_key(row.choice_key()))
    }
}

pub fn filtered<'a>(rows: &'a [Row], listing: &ListingState) -> Vec<&'a Row> {
    let query = listing.query.to_lowercase();
    let mut passing: Vec<&Row> =
        rows.iter().filter(|row| passes(row, &query, listing.filter)).collect();
    sort(&mut passing, listing);
    passing
}

pub fn page_of(rows: &[Row], listing: &ListingState, key: &str) -> Option<usize> {
    let at = filtered(rows, listing).iter().position(|row| row.named(key))?;
    Some(at / PAGE_SIZE)
}

pub fn arrange<'a>(rows: &'a [Row], listing: &ListingState) -> Arranged<'a> {
    use crate::module::state::listing::Choice;

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

fn passes(row: &Row, query: &str, filter: Filter) -> bool {
    matches_query(row, query) && filter.matches(&row.status)
}

fn matches_query(row: &Row, query: &str) -> bool {
    query.is_empty() || row.title.to_lowercase().contains(query)
}

fn sort(rows: &mut [&Row], listing: &ListingState) {
    use crate::module::state::listing::Sorting;
    rows.sort_by(|left, right| {
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

    let paths: Vec<Vec<&str>> = rows.iter().map(|row| segments(row.folder(), grouping)).collect();

    let mut lines = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let path = &paths[index];
        let common = match index.checked_sub(1) {
            Some(before) => path.iter().zip(&paths[before]).take_while(|(next, previous)| next == previous).count(),
            None => 0,
        };

        for (depth, segment) in path.iter().enumerate().skip(common) {
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

fn expand<'a>(row: &'a Row, listing: &ListingState, depth: usize, lines: &mut Vec<Line<'a>>) {
    if !listing.expanded.contains(row.key()) {
        return;
    }
    if row.loading {
        lines.push(Line::Waiting { depth: depth + 1 });
    }
    for child in &row.children {
        lines.push(Line::Entry { row: child, depth: depth + 1 });
        expand(child, listing, depth + 1, lines);
    }
}

fn segments(folder: &str, grouping: Grouping) -> Vec<&str> {
    match grouping {
        Grouping::Folder => match last_segment(folder) {
            "" => Vec::new(),
            name => vec![name],
        },
        Grouping::Tree => folder.split('/').filter(|name| !name.is_empty()).collect(),
        Grouping::None => Vec::new(),
    }
}
