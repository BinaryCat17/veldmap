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
}

/// Готовый к показу список и всё, что нужно знать о его страницах.
pub struct Arranged<'a> {
    pub lines: Vec<Line<'a>>,
    /// Сколько записей прошло отбор — по нему подписан диапазон страницы.
    pub total: usize,
    pub pages: usize,
    /// Страница, которая показана: та, что просили, но не дальше последней.
    pub page: usize,
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
}

/// Сколько записей в каждом состоянии — счётчики в меню отбора. Считаются до
/// отбора: иначе «Все» показывало бы столько же, сколько выбранное.
pub fn counts(rows: &[Row], listing: &ListingState) -> Vec<usize> {
    use crate::module::state::listing::{Choice, Filter};
    let matching: Vec<&Row> = rows.iter().filter(|row| matches_query(row, &listing.query)).collect();
    Filter::ALL
        .iter()
        .map(|filter| matching.iter().filter(|row| filter.matches(&row.status)).count())
        .collect()
}

pub fn arrange<'a>(rows: &'a [Row], listing: &ListingState) -> Arranged<'a> {
    let mut selected: Vec<&Row> = rows
        .iter()
        .filter(|row| listing.filter.matches(&row.status) && matches_query(row, &listing.query))
        .collect();

    sort(&mut selected, listing);

    let total = selected.len();
    let pages = total.div_ceil(PAGE_SIZE).max(1);
    let page = listing.page.min(pages - 1);
    let shown = selected
        .into_iter()
        .skip(page * PAGE_SIZE)
        .take(PAGE_SIZE)
        .collect();

    Arranged { lines: group(shown, listing.grouping), total, pages, page }
}

/// Отбор по имени — по вхождению, без учёта регистра: пользователь помнит
/// кусок имени, а не его начало.
fn matches_query(row: &Row, query: &str) -> bool {
    query.is_empty() || row.title.to_lowercase().contains(&query.to_lowercase())
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
fn group(rows: Vec<&Row>, grouping: Grouping) -> Vec<Line<'_>> {
    if grouping == Grouping::None {
        return rows.into_iter().map(|row| Line::Entry { row, depth: 0 }).collect();
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
    }
    lines
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
