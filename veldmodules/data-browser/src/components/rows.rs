//! components/rows.rs — строки списка из того, чем живёт вид.
//!
//! Отдельно от разметки затем, что спрашивает их не одна она: «отметить всё» и
//! переход к строке считают по тем же строкам, что показаны, — а вторая их
//! сборка разошлась бы с первой ровно там, где это заметнее всего, в отборе.
//!
//! Знания о видах здесь ровно столько, сколько нужно, чтобы назвать источник:
//! каталог отвечает записями листинга, поиск — продуктами, скачанное —
//! записями библиотеки. Всё остальное про строку живёт в `row`.

use crate::module::components::row::{
    bare, downloaded_rows, folder_path, OnGlobe, Row, RowKind, RowStatus,
};
use crate::module::state::browse::{BrowseItem, Children};
use crate::module::state::listing::ListingState;
use crate::module::state::{BrowseState, SearchState, State, ViewId, ViewKind};

/// Строки названного вида. Пусто — вид не списочный: у превью, глобуса и «На
/// просмотре» таблицы нет вовсе.
pub fn of(state: &State, view: ViewId) -> Vec<Row> {
    let Some(kind) = state.get(view) else { return Vec::new() };
    match kind {
        ViewKind::Browse(opened) => browse(state, opened),
        ViewKind::Search(opened) => search(state, opened),
        ViewKind::Downloaded(_) => {
            // Скачанное собрано из записей библиотеки, а не из ключей каталога,
            // — общей сборки строк (`from_key`) у него нет, и признак шара
            // проставляется здесь. Вглубь при этом идём тем же обходом, что и
            // общая сборка: одинаковая с виду подстрока не должна гореть в
            // каталоге и молчать здесь.
            let mut rows = downloaded_rows(&state.library);
            mark_globe(state, &mut rows);
            rows
        }
        // Таблицы у них нет вовсе; перечислены поимённо, чтобы новый вид со
        // списком не остался молча без строк.
        ViewKind::Empty | ViewKind::Preview(_) | ViewKind::Globe(_) | ViewKind::Shown => {
            Vec::new()
        }
    }
}

/// Проставить признак шара строкам «Скачанного» и всему, что под ними.
fn mark_globe(state: &State, rows: &mut [Row]) {
    for row in rows {
        row.globe = onto_globe(state, row.snapshot_key());
        mark_globe(state, &mut row.children);
    }
}

/// Чем снимок этой строки лежит на шаре (см. [`OnGlobe`]).
///
/// Спрашивается по ключу снимка, а не по ключу строки: строка каталога знает
/// папку со слэшем на конце, а наложение помнит продукт без него.
///
/// Единственное место, где строка встречается с набором наложений: и значок, и
/// полоса хода спрашивают об одном и том же, а два ответа на это однажды
/// разошлись бы.
fn onto_globe(state: &State, key: &str) -> OnGlobe {
    let Some(overlay) = state.overlays.iter().find(|overlay| overlay.identifier == key) else {
        return OnGlobe::Off;
    };
    match overlay.on_globe() {
        false => OnGlobe::Assembling,
        true => OnGlobe::Laid { hidden: overlay.hidden, share: overlay.progress.share() },
    }
}

/// Сетевой каталог: записи текущей папки и содержимое раскрытых.
pub fn browse(state: &State, view: &BrowseState) -> Vec<Row> {
    entries(state, &view.items, &view.children, &view.listing)
}

/// Выдача поиска: каждая строка — снимок, а раскрытая показывает свои файлы
/// тем же листингом, что и каталог.
pub fn search(state: &State, view: &SearchState) -> Vec<Row> {
    view.results
        .iter()
        .map(|product| {
            // Различает строки только то, чем продукт лежит в хранилище
            // (см. DataProduct.folder): GET по пути продукта-каталога — это
            // 404, и «открыть» его значит перейти внутрь, теми же строками,
            // что папки сетевого каталога; продукт-архив — обычный объект.
            let kind = RowKind::Product { folder: product.folder };
            let mut row = Row {
                product_type: product.product_type.clone(),
                ..from_key(
                    state,
                    product.identifier.clone(),
                    product.name.clone(),
                    product.size,
                    product.acquired,
                    kind,
                    // В выдаче строка и есть снимок — сама себе продукт.
                    product.identifier.clone(),
                )
            };
            fill(state, &mut row, &view.children, &view.listing);
            row
        })
        .collect()
}

/// Записи листинга → строки вместе с содержимым раскрытых папок.
fn entries(
    state: &State,
    items: &[BrowseItem],
    children: &Children,
    listing: &ListingState,
) -> Vec<Row> {
    items
        .iter()
        .map(|item| {
            // Снимком запись делает провайдер: раскладку бакета знает только
            // он (см. `ListEntry.product`). Папкой она при этом остаться может
            // — .SAFE и есть папка, — и заход внутрь у неё никто не отнимает.
            let itself = item.product == bare(&item.identifier);
            let kind = match (itself, item.is_folder) {
                (true, folder) => RowKind::Product { folder },
                (false, true) => RowKind::Folder,
                (false, false) => RowKind::File,
            };
            let mut row = from_key(
                state,
                item.identifier.clone(),
                item.name.clone(),
                item.size,
                item.modified,
                kind,
                item.product.clone(),
            );
            fill(state, &mut row, children, listing);
            row
        })
        .collect()
}

/// Строка из ключа хранилища — одна сборка на каталог и на выдачу поиска.
///
/// Разводит их один вопрос: есть ли внутри содержимое. У того, что содержит
/// другое, ни размера, ни времени за собой не стоит — в S3 папка это общий
/// префикс ключей; зато известно, сколько её содержимого уже на диске, и знает
/// это библиотека, а не каталог. У обычного объекта наоборот: размер и время
/// сказал каталог, а состояние — библиотека, если запись у неё есть.
fn from_key(
    state: &State,
    identifier: String,
    title: String,
    size: u64,
    date: i64,
    kind: RowKind,
    product: String,
) -> Row {
    // Ход укладки на шар — здесь, потому что здесь собираются строки и каталога,
    // и выдачи поиска: снимок в них один и тот же, и два ответа на этот вопрос
    // однажды разошлись бы.
    let globe = onto_globe(state, bare(&identifier));
    if kind.is_folder() {
        // Два разных вопроса о папке, и второй задаётся только снимку. Сколько
        // её содержимого на диске, видно по ключам записей — это и всё, что
        // можно сказать о папке пути: сколько файлов в ней должно быть, не
        // знает никто. У снимка это число есть (`LibraryEntry.siblings`), и
        // только с ним «весь на диске» — утверждение, а не догадка.
        //
        // Спрашивается оно снимком, а не ключом строки: ключ папки приезжает со
        // слэшем на конце, а записи библиотеки помнят снимок без него.
        let stored = state.library.count_under(&folder_path(&identifier));
        let (done, files) = state.library.snapshot(&product);
        let whole = files > 0 && done as u32 == files;
        let status = match (whole, stored) {
            (true, _) => RowStatus::Complete,
            (false, 0) => RowStatus::Remote,
            (false, done) => RowStatus::Partial { done },
        };
        let row = Row::container_row(identifier, title, status, kind);
        return Row { size, date, product, globe, ..row };
    }
    Row { product, globe, ..Row::remote(&state.library, identifier, title, size, date, kind) }
}

/// Содержимое раскрытой папки — подстроками под ней, и так вглубь: раскрытая
/// папка внутри раскрытой — обычное дело, и второго правила для неё нет.
///
/// Нераскрытая молчит о своём содержимом, даже когда оно уже приехало: строка
/// показывает то, что раскрыли, а не то, что успело загрузиться.
fn fill(state: &State, row: &mut Row, children: &Children, listing: &ListingState) {
    if !row.kind.is_folder() || !listing.expanded.contains(row.key()) {
        return;
    }
    let path = folder_path(&row.identifier);
    row.loading = children.waiting(&path);
    row.children = entries(state, children.get(&path), children, listing);
}
