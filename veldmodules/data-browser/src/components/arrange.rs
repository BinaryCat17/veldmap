//! components/arrange.rs — что и в каком порядке показывать.
//!
//! Отбор, сортировка, страница и группировка — в одном месте и без единого
//! виджета: это правила, а не оформление, и проверяются они по данным, а не
//! глазами. Разметка получает уже готовый список строк (см. `table`).

use crate::module::components::{format, Row};
use crate::module::state::listing::{Grouping, ListingState};

/// Сколько записей на странице. Число фиксированное, а не выведенное из высоты
/// окна: страница, меняющая состав при изменении размера окна, теряет место, на
/// котором стоял пользователь.
pub const PAGE_SIZE: usize = 20;

/// Строка таблицы после группировки.
pub enum Line<'a> {
    /// Заголовок группы: папка и сколько в ней показано.
    Group { title: String, meta: String, depth: usize },
    Entry { row: &'a Row, depth: usize },
    /// Раскрытая папка, чьё содержимое ещё едет. Своя строка, а не поддельная
    /// запись: у записи есть ключ, род и действия, а у ожидания нет ничего,
    /// кроме места, которое оно занимает.
    Waiting { depth: usize },
}

/// Готовый к показу список и всё, что нужно знать о его страницах.
pub struct Arranged<'a> {
    pub lines: Vec<Line<'a>>,
    /// Сколько записей прошло отбор — по нему подписан диапазон страницы.
    pub total: usize,
    pub pages: usize,
    /// Страница, которая показана: та, что просили, но не дальше последней.
    pub page: usize,
    /// Сколько записей в каждом состоянии (по `Filter::ALL`) — счётчики меню
    /// отбора. Считаются до отбора: иначе «Все» показывало бы столько же,
    /// сколько выбранное.
    pub counts: Vec<usize>,
}

impl Arranged<'_> {
    /// «1–20 из 36». Пустой список — пустая подпись: диапазона у него нет.
    pub fn range(&self) -> String {
        if self.total == 0 {
            return String::new();
        }
        let from = self.page * PAGE_SIZE + 1;
        let to = ((self.page + 1) * PAGE_SIZE).min(self.total);
        format!("{}–{} из {}", from, to, self.total)
    }

    /// Строки, показанные на этой странице, — вместе с раскрытыми подстроками.
    /// Заголовки групп сюда не идут: они не строки, а подписи над ними.
    ///
    /// По ним отвечают на всякий вопрос «что видно»: какие колонки завести,
    /// что отметит коробочка в шапке, отмечено ли уже всё. Второго ответа на
    /// этот вопрос нет намеренно — разойдясь, колонка и её содержимое
    /// показывали бы разное.
    pub fn shown(&self) -> impl Iterator<Item = &Row> {
        self.lines.iter().filter_map(|line| match line {
            Line::Entry { row, .. } => Some(*row),
            Line::Group { .. } | Line::Waiting { .. } => None,
        })
    }

    /// Ключи снимков, показанных на этой странице, — ровно то, на что действует
    /// коробочка в шапке.
    ///
    /// Страница, а не весь отбор: коробочка стои́т над видимым, и отмечать ею
    /// сотню строк, которых на экране нет, значило бы заодно спросить у
    /// каталога сотню продуктов — по одному на контур.
    pub fn marks(&self) -> impl Iterator<Item = &str> {
        self.shown()
            .filter(|row| row.is_snapshot())
            .map(Row::snapshot_key)
            .filter(|key| !key.is_empty())
    }

    /// Отмечено ли всё, что видно. Отмечать нечего — не отмечено: коробочка
    /// тогда предлагает отметить, а нажатие ничего не меняет.
    pub fn all_marked(&self, listing: &ListingState) -> bool {
        let mut marks = self.marks().peekable();
        marks.peek().is_some() && marks.all(|key| listing.selected.contains(key))
    }
}

/// Строки, прошедшие отбор, в порядке показа.
///
/// Общее начало у показа и у всякого действия над списком целиком: «отметить
/// всё» отмечает ровно то, что видно, а переход к строке ищет её страницу в
/// том же порядке. Вторая копия этого отбора разошлась бы с первой ровно там,
/// где это заметнее всего.
pub fn filtered<'a>(rows: &'a [Row], listing: &ListingState) -> Vec<&'a Row> {
    let query = listing.query.to_lowercase();
    let mut passing: Vec<&Row> = rows
        .iter()
        .filter(|row| matches_query(row, &query) && listing.filter.matches(&row.status))
        .collect();
    sort(&mut passing, listing);
    passing
}

/// На какой странице стоит строка с этим ключом. `None` — под отбор она не
/// подошла, и вести к ней некуда.
pub fn page_of(rows: &[Row], listing: &ListingState, key: &str) -> Option<usize> {
    let at = filtered(rows, listing).iter().position(|row| row.named(key))?;
    Some(at / PAGE_SIZE)
}

pub fn arrange<'a>(rows: &'a [Row], listing: &ListingState) -> Arranged<'a> {
    use crate::module::state::listing::{Choice, Filter};

    // Счётчики меню считаются до отбора по состоянию, но после отбора по
    // имени: «Все» иначе показывало бы столько же, сколько выбранное.
    let query = listing.query.to_lowercase();
    let counts = Filter::ALL
        .iter()
        .map(|filter| {
            rows.iter()
                .filter(|row| matches_query(row, &query) && filter.matches(&row.status))
                .count()
        })
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

/// Отбор по имени — по вхождению, без учёта регистра: пользователь помнит
/// кусок имени, а не его начало. `query` уже в нижнем регистре.
fn matches_query(row: &Row, query: &str) -> bool {
    query.is_empty() || row.title.to_lowercase().contains(query)
}

fn sort(rows: &mut [&Row], listing: &ListingState) {
    use crate::module::state::listing::Sorting;
    rows.sort_by(|left, right| {
        // При группировке порядок папок старше любого выбранного: иначе строки
        // одной папки расходятся, и заголовок над ними открывается по разу на
        // каждую.
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

/// Раскладывает записи по группам. Обе группировки — один и тот же обход: у
/// «по папкам» ступенька одна (вся папка целиком), у «дерева» их столько,
/// сколько сегментов в пути. Заголовок открывается там, где путь разошёлся с
/// предыдущим, — строки к этому моменту уже стоят по папкам (см. `sort`).
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

    // Пути считаются один раз на строку: их сравнивают и с предыдущей строкой,
    // и со всеми следующими — пересчитывать их на каждое сравнение значит
    // разбирать один и тот же путь десяток раз.
    let paths: Vec<Vec<&str>> = rows.iter().map(|row| segments(row.folder(), grouping)).collect();

    let mut lines = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let path = &paths[index];
        let common = match index.checked_sub(1) {
            Some(before) => path.iter().zip(&paths[before]).take_while(|(next, previous)| next == previous).count(),
            None => 0,
        };

        for (depth, segment) in path.iter().enumerate().skip(common) {
            // Счётчик — только у самой глубокой ступени: у промежуточных он
            // считал бы не показанное здесь, а всё поддерево.
            let meta = if depth + 1 == path.len() {
                let same = paths[index..].iter().take_while(|other| *other == path).count();
                format!("{} {}", same, format::plural(same, ["файл", "файла", "файлов"]))
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

/// Содержимое раскрытой строки — подстроками под ней. Ступенькой глубже, тем
/// же `Line::Entry`: подстрока это такая же строка, и рисовать её отдельным
/// родом значило бы завести вторую таблицу.
///
/// Вглубь, а не на один ярус: раскрытая папка внутри раскрытой — обычное дело,
/// и второго правила для неё нет.
///
/// Отбор и сортировка их не касаются: содержимое показывает состав строки, а
/// не участвует в списке наравне с ней — иначе «показано 20 из 36» считало бы
/// разное в зависимости от того, что раскрыто.
fn expand<'a>(row: &'a Row, listing: &ListingState, depth: usize, lines: &mut Vec<Line<'a>>) {
    if !listing.expanded.contains(row.key()) {
        return;
    }
    // Ответ ещё в пути — так и сказано. Пустое место под раскрытой папкой
    // читается как «здесь ничего нет», а это неправда ровно до конца листинга.
    if row.loading {
        lines.push(Line::Waiting { depth: depth + 1 });
    }
    for child in &row.children {
        lines.push(Line::Entry { row: child, depth: depth + 1 });
        expand(child, listing, depth + 1, lines);
    }
}

/// Ступени, которые открывает эта папка.
fn segments(folder: &str, grouping: Grouping) -> Vec<&str> {
    match grouping {
        // Заголовком названа сама папка, а путь к ней не показан: он одинаков
        // у всех строк вида и места в заголовке не стоит.
        Grouping::Folder => folder.rsplit('/').next().filter(|name| !name.is_empty()).into_iter().collect(),
        Grouping::Tree => folder.split('/').filter(|name| !name.is_empty()).collect(),
        Grouping::None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::components::{RowKind, RowStatus};
    use crate::module::state::listing::Sorting;

    fn folder(key: &str, children: Vec<Row>) -> Row {
        Row {
            children,
            ..Row::container_row(
                key.to_string(),
                key.to_string(),
                RowStatus::Remote,
                RowKind::Folder,
            )
        }
    }

    fn snapshot(key: &str) -> Row {
        Row::container_row(
            key.to_string(),
            key.to_string(),
            RowStatus::Remote,
            RowKind::Product { folder: true },
        )
    }

    fn depths(arranged: &Arranged<'_>) -> Vec<usize> {
        arranged
            .lines
            .iter()
            .map(|line| match line {
                Line::Entry { depth, .. }
                | Line::Group { depth, .. }
                | Line::Waiting { depth } => *depth,
            })
            .collect()
    }

    /// Раскрытая папка внутри раскрытой показывается вглубь, ступенькой на
    /// каждый ярус: второго правила для вложенности нет.
    #[test]
    fn expansion_goes_all_the_way_down() {
        let rows = vec![folder("a/", vec![folder("a/b/", vec![folder("a/b/c/", Vec::new())])])];
        let mut listing = ListingState::default();
        listing.expanded.insert("a/".to_string());
        listing.expanded.insert("a/b/".to_string());

        assert_eq!(depths(&arrange(&rows, &listing)), vec![0, 1, 2]);

        // Свёрнутая молчит о своём содержимом, даже когда оно уже приехало.
        listing.expanded.remove("a/b/");
        assert_eq!(depths(&arrange(&rows, &listing)), vec![0, 1]);
    }

    /// Страница строки считается по тому же отбору, что показан, — и слэш
    /// папки в ключе в счёт не идёт: в каталоге он есть, в выдаче поиска нет,
    /// а строка одна и та же.
    #[test]
    fn page_of_finds_the_row_across_pages_and_a_trailing_slash() {
        let rows: Vec<Row> =
            (0..45).map(|index| folder(&format!("p/{:02}/", index), Vec::new())).collect();
        let listing = ListingState { sorting: Sorting::Name, ..Default::default() };

        assert_eq!(page_of(&rows, &listing, "p/00"), Some(0));
        assert_eq!(page_of(&rows, &listing, "p/25/"), Some(1));
        assert_eq!(page_of(&rows, &listing, "p/44"), Some(2));
        assert_eq!(page_of(&rows, &listing, "чего тут нет"), None);
    }

    /// Отбор один на показ и на переход: строка, которую отбор убрал, не
    /// показана — и вести к ней некуда.
    #[test]
    fn page_of_obeys_the_same_filter_as_the_list() {
        let rows: Vec<Row> = vec![folder("p/one/", Vec::new()), folder("p/two/", Vec::new())];
        let listing = ListingState { query: "two".to_string(), ..Default::default() };

        assert_eq!(page_of(&rows, &listing, "p/two/"), Some(0));
        assert_eq!(page_of(&rows, &listing, "p/one/"), None);
    }

    /// Коробочка в шапке говорит о снимках, показанных под ней: у папки пути
    /// контура нет и отмечать её нечем, а скрытое отбором в счёт не идёт —
    /// иначе на шар легло бы то, чего на экране нет.
    #[test]
    fn the_header_box_counts_only_the_snapshots_it_shows() {
        let rows = vec![snapshot("p/a"), snapshot("p/b"), folder("p/c/", Vec::new())];
        let mut listing = ListingState::default();
        assert!(!arrange(&rows, &listing).all_marked(&listing), "пока не отмечено ничего");

        listing.selected.insert("p/a".to_string());
        assert!(!arrange(&rows, &listing).all_marked(&listing), "отмечен один из двух");

        listing.selected.insert("p/b".to_string());
        assert!(arrange(&rows, &listing).all_marked(&listing), "папка в счёт не идёт");

        // Отбор сузил список до одного снимка — и он отмечен.
        let mut narrowed = ListingState { query: "a".to_string(), ..Default::default() };
        narrowed.selected.insert("p/a".to_string());
        assert!(arrange(&rows, &narrowed).all_marked(&narrowed));
    }

    /// Отмечать нечего — коробочка не держится нажатой: иначе она обещала бы,
    /// что нажатие её отпустит, а отпускать нечего.
    #[test]
    fn the_header_box_of_an_unmarkable_list_is_empty() {
        let listing = ListingState::default();
        let folders = vec![folder("p/c/", Vec::new())];
        assert!(!arrange(&folders, &listing).all_marked(&listing));
        assert!(!arrange(&[], &listing).all_marked(&listing));
    }

    /// Раскрытая подстрока-снимок видна наравне с прочими — и коробочка в
    /// шапке обязана считать её вместе со всеми, иначе она никогда не
    /// заполнится там, где что-то раскрыли.
    #[test]
    fn expanded_snapshots_count_as_shown() {
        let rows = vec![folder("p/day/", vec![snapshot("p/day/one")])];
        let mut listing = ListingState::default();
        listing.expanded.insert("p/day/".to_string());

        assert_eq!(arrange(&rows, &listing).marks().collect::<Vec<_>>(), vec!["p/day/one"]);
        assert!(!arrange(&rows, &listing).all_marked(&listing));

        listing.selected.insert("p/day/one".to_string());
        assert!(arrange(&rows, &listing).all_marked(&listing));
    }

    /// Страница ограничивает и коробочку: отмечать разом то, чего на экране
    /// нет, значило бы спросить у каталога сотню продуктов за одно нажатие.
    #[test]
    fn the_header_box_stops_at_the_page() {
        let rows: Vec<Row> =
            (0..PAGE_SIZE + 5).map(|index| snapshot(&format!("p/{:02}", index))).collect();
        let listing = ListingState::default();
        assert_eq!(arrange(&rows, &listing).marks().count(), PAGE_SIZE);
    }
}
