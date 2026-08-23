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
    bare, downloaded_rows, folder_path, OnGlobe, OnOutline, Row, RowKind, RowStatus,
};
use crate::module::state::browse::{BrowseItem, Children};
use crate::module::state::listing::ListingState;
use crate::module::state::{BrowseState, Located, SearchState, State, ViewId, ViewKind};

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
        row.globe = onto_globe(state, row.product_key());
        row.outlined = outlined(state, row.product_key());
        mark_globe(state, &mut row.children);
    }
}

/// Чем снимок этой строки лежит на шаре (см. [`OnGlobe`]).
///
/// Спрашивается по ключу снимка, а не по ключу строки: строка каталога знает
/// папку со слэшем на конце, а наложение помнит продукт без него.
///
/// Единственное место, где ключ снимка встречается с набором наложений. Кроме
/// значка и полосы хода спрашивает его и штриховка занятой области (см.
/// `handlers::outline::send`): вопрос у всех троих один — «что с этим снимком
/// на шаре сейчас», — а три ответа на него разошлись бы молча.
pub fn onto_globe(state: &State, key: &str) -> OnGlobe {
    let Some(overlay) = state.overlays.iter().find(|overlay| overlay.identifier == key) else {
        // Наложения ещё нет, но показ уже просят: продукт по ключу
        // восстанавливает каталог, и ход к нему сетевой. Молчать эти секунды
        // нельзя — нажатие выглядело бы пропавшим, а второе нажатие по тому же
        // значку как раз и снимает просьбу.
        return match state.showing.contains(key) {
            true => OnGlobe::Asked,
            false => OnGlobe::Off,
        };
    };
    match overlay.on_globe() {
        false => OnGlobe::Assembling,
        true => OnGlobe::Laid { hidden: overlay.hidden, progress: overlay.progress },
    }
}

/// Очерчен ли снимок этой строки (см. [`OnOutline`]).
///
/// Просьба и нарисованное — разные вещи: геометрию знает каталог, ход к нему
/// сетевой, и между нажатием и контуром проходят секунды. Значок обязан
/// зажечься сразу, иначе нажатие выглядит пропавшим.
fn outlined(state: &State, key: &str) -> OnOutline {
    if key.is_empty() || !state.outlines.contains(key) {
        return OnOutline::Off;
    }
    if state.outlined.iter().any(|outlined| outlined.key == key) {
        return OnOutline::Drawn;
    }
    match state.located.get(key) {
        // Ответа ещё нет: запрос уходит той же пересборкой, что и эта строка.
        None | Some(Located::Asking) => OnOutline::Asking,
        // Спросить не вышло — это про сеть, а не про снимок, и переспросить
        // можно тем же значком.
        Some(Located::Failed) => OnOutline::Failed,
        // Каталог ответил, а рисовать нечего: он либо не знает этот ключ, либо
        // знает продукт без геометрии.
        Some(Located::Missing) => OnOutline::Blank,
        // Геометрия известна: пустая — рисовать нечего, непустая — уже
        // нарисована. Разойтись с `state.outlined` она может только внутри
        // одной пересборки, которая кладёт туда и сюда.
        Some(Located::Found(found)) => match found.footprint.is_empty() {
            true => OnOutline::Blank,
            false => OnOutline::Drawn,
        },
    }
}

/// Сетевой каталог: записи текущей папки и содержимое раскрытых.
pub fn browse(state: &State, view: &BrowseState) -> Vec<Row> {
    entries(state, &view.items, &view.children, &view.listing)
}

/// Выдача поиска: каждая строка — снимок, а раскрытая показывает своё
/// содержимое тем же листингом, что и каталог.
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
                unviewable: product.unviewable.clone(),
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
            match product.parts.len() {
                // Часть одна — снимок это она и есть, и раскрывается он прямо
                // в свои файлы.
                0 | 1 => fill(state, &mut row, &view.children, &view.listing),
                _ => parted(state, &mut row, product, &view.children, &view.listing),
            }
            row
        })
        .collect()
}

/// Снимок, о котором каталог отдал несколько продуктов: раскрывается он в свои
/// части, а уже они — каждая в свои файлы.
///
/// Ярусом больше, потому что часть — не файл снимка, а сама съёмка в другом
/// виде: другая обработка того же (сырьё приёмника, полосный TIFF, тайловый
/// COG) или другая измеренная величина того же пролёта (двуокись азота,
/// угарный газ, озон). Уложить их вперемешку с файлами значило бы сказать, что
/// снимок из них состои́т.
fn parted(
    state: &State,
    row: &mut Row,
    product: &crate::proto::data_provider::DataProduct,
    children: &Children,
    listing: &ListingState,
) {
    // Свой ключ, потому что показываемая часть стои́т под снимком собственной
    // строкой: с одним ключом на двоих раскрывались бы обе разом.
    row.group = Some(crate::module::components::row::scene_key(&product.identifier));
    if !listing.expanded.contains(row.key()) {
        return;
    }
    row.children = product
        .parts
        .iter()
        .map(|part| {
            let mut child = Row {
                // Подписана часть своим типом, а не именем файла: имена частей
                // одной съёмки расходятся только хвостом, и строки с такими
                // именами друг под другом не сообщают ничего.
                title: match part.product_type.is_empty() {
                    true => part.name.clone(),
                    false => part.product_type.clone(),
                },
                // Колонка вида у части свободна — типом её уже подписали, — и
                // занята она тем, что о ней и надо знать: показывают её или она
                // лежит про запас. В подпись это не влезает: имя строки
                // ужимается по ширине колонки, и пометка ужалась бы первой.
                product_type: match part.shown {
                    true => "показана".to_string(),
                    false => "часть".to_string(),
                },
                unviewable: part.unviewable.clone(),
                ..from_key(
                    state,
                    part.identifier.clone(),
                    part.name.clone(),
                    part.size,
                    product.acquired,
                    RowKind::Product { folder: part.folder },
                    part.identifier.clone(),
                )
            };
            fill(state, &mut child, children, listing);
            child
        })
        .collect();
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
            let mut row = Row {
                unviewable: item.unviewable.clone(),
                ..from_key(
                    state,
                    item.identifier.clone(),
                    item.name.clone(),
                    item.size,
                    item.modified,
                    kind,
                    item.product.clone(),
                )
            };
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
    // Оба состояния шара — здесь, потому что здесь собираются строки и каталога,
    // и выдачи поиска: снимок в них один и тот же, и два ответа на эти вопросы
    // однажды разошлись бы.
    //
    // Спрашиваются они снимком строки, а не самой строкой: на шаре лежит
    // снимок, и файл внутри него отвечает о нём, а не о себе (см.
    // [`Row::product_key`]). Своей полосы файлу это не даёт — её рисуют только
    // снимкам, — зато пункт меню, кладущий снимок на шар, знает, лежит ли он
    // там уже.
    let snapshot = match product.is_empty() {
        false => bare(&product),
        true => bare(&identifier),
    };
    let globe = onto_globe(state, snapshot);
    let outlined = outlined(state, snapshot);
    if kind.is_folder() {
        // Два разных вопроса о папке, и второй задаётся только снимку. Сколько
        // её содержимого на диске, видно по ключам записей — это и всё, что
        // можно сказать о папке пути: сколько файлов в ней должно быть, не
        // знает никто. У снимка это число есть (`LibraryEntry.siblings`), и
        // только с ним «весь на диске» — утверждение, а не догадка.
        //
        // Спрашивается оно снимком, а не ключом строки: ключ папки приезжает со
        // слэшем на конце, а записи библиотеки помнят снимок без него.
        let under = state.library.under(&folder_path(&identifier));
        let (done, files) = state.library.snapshot(&product);
        let whole = files > 0 && done as u32 == files;
        // Пока хоть один файл едет, едет весь снимок — то же правило, что у
        // сложенной строки «Скачанного» (см. `row::snapshot`). Без него
        // качающийся снимок стои́т с зелёным «3 на диске» и без полосы: два
        // разных ответа на один вопрос, и разошлись бы они молча.
        //
        // Снимок, а не всякая папка: у папки пути своей закачки нет, и качается
        // в ней не она, а лежащий внутри снимок. Сказанное о ней «скачивается»
        // отняло бы у неё счёт файлов на диске — единственное, что о папке
        // вообще можно сказать, — и ничего не дало бы взамен: размера у неё
        // тоже нет, и полосе не из чего взяться.
        let downloading = under.active && kind.is_product();
        let status = match (whole, downloading, under.files) {
            (true, ..) => RowStatus::Complete,
            // Всего байт — то, что о размере сказал каталог: сама библиотека
            // знает только уже скачанное, а без знаменателя полосе не из чего
            // взяться.
            (false, true, _) => RowStatus::Downloading { done: under.bytes, total: size },
            (false, false, 0) => RowStatus::Remote,
            // Стоять папке не по чему: своих закачек у неё нет.
            (false, false, done) => RowStatus::Partial { done, trouble: String::new() },
        };
        let row = Row::container_row(identifier, title, status, kind);
        return Row { size, date, product, globe, outlined, ..row };
    }
    Row {
        product,
        globe,
        outlined,
        ..Row::remote(&state.library, identifier, title, size, date, kind)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::state::globe::Outlined;
    use crate::proto::data_provider::{DataProduct, GeoPoint, Ring};

    const KEY: &str = "eodata/store/A.SAFE";

    fn state() -> State {
        State::new(crate::module::handlers::Config { initial_view: None }).expect("состояние")
    }

    fn answer(geometry: bool) -> Located {
        let footprint = match geometry {
            false => Vec::new(),
            true => vec![Ring { points: vec![GeoPoint { lat: 10.0, lon: 10.0 }] }],
        };
        Located::Found(DataProduct {
            identifier: KEY.to_string(),
            footprint,
            ..Default::default()
        })
    }

    fn drawn() -> Outlined {
        Outlined {
            key: KEY.to_string(),
            label: "A.SAFE".to_string(),
            folder: false,
            rings: Vec::new(),
        }
    }

    /// Пять лиц значка контура: просьба, ход к каталогу и три его исхода.
    ///
    /// Значок — единственное, по чему в списке видно, что с контуром
    /// происходит, и перепутанные лица врут молча: «спрашиваем» у того, за чем
    /// никто не пошёл, или «геометрии нет» у оборвавшейся сети.
    #[test]
    fn the_outline_icon_tells_the_request_from_the_drawing() {
        let mut state = state();
        assert_eq!(outlined(&state, KEY), OnOutline::Off, "очертить не просили");
        assert_eq!(outlined(&state, ""), OnOutline::Off, "адресовать нечем");

        state.outlines.insert(KEY.to_string());
        assert_eq!(outlined(&state, KEY), OnOutline::Asking, "запрос уходит этой же пересборкой");

        state.located.insert(KEY.to_string(), Located::Asking);
        assert_eq!(outlined(&state, KEY), OnOutline::Asking);

        state.located.insert(KEY.to_string(), Located::Failed);
        assert_eq!(outlined(&state, KEY), OnOutline::Failed, "сеть — не ответ про снимок");

        state.located.insert(KEY.to_string(), Located::Missing);
        assert_eq!(outlined(&state, KEY), OnOutline::Blank, "каталог такого не знает");

        state.located.insert(KEY.to_string(), answer(false));
        assert_eq!(outlined(&state, KEY), OnOutline::Blank, "знает, а геометрии нет");

        state.outlined.push(drawn());
        assert_eq!(outlined(&state, KEY), OnOutline::Drawn);
    }

    /// Состояние контура проставляется и строкам каталога, и строкам выдачи:
    /// собирает их другая сборка, чем «Скачанное», и молчащий там значок
    /// переключался бы против собственной подсказки.
    #[test]
    fn a_catalogue_row_knows_its_outline_too() {
        let mut state = state();
        state.outlines.insert(KEY.to_string());
        state.outlined.push(drawn());

        let row = from_key(
            &state,
            // Со слэшем — так ключ приезжает из листинга каталога.
            format!("{}/", KEY),
            "A.SAFE".to_string(),
            0,
            0,
            RowKind::Product { folder: true },
            KEY.to_string(),
        );

        assert_eq!(row.outlined, OnOutline::Drawn, "строка каталога не знает про контур");
    }

    /// Пока хоть один файл снимка едет, едет весь снимок — и строка каталога
    /// говорит это тем же словом, что и сложенная строка «Скачанного».
    ///
    /// Без этого правила качающийся .SAFE стои́т зелёным «2 на диске» и без
    /// полосы: закачки у него по файлам, а строка о файлах не знает.
    #[test]
    fn a_catalogue_snapshot_says_it_is_downloading() {
        use crate::proto::data_library::{LibraryEntry, LibraryStatus};
        let file = |name: &str, status: LibraryStatus, done: u64| LibraryEntry {
            name: name.to_string(),
            identifier: format!("{}/{}", KEY, name),
            product: KEY.to_string(),
            done,
            total: done,
            status: status as i32,
            modified: 0,
            siblings: 0,
            trouble: String::new(),
        };
        let row = |state: &State| {
            from_key(
                state,
                format!("{}/", KEY),
                "A.SAFE".to_string(),
                // Размер снимка сказал каталог — из него и берётся знаменатель
                // полосы: библиотека знает только уже скачанное.
                1000,
                0,
                RowKind::Product { folder: true },
                KEY.to_string(),
            )
        };

        let mut state = state();
        state.library.entries =
            vec![file("B1.TIF", LibraryStatus::LibComplete, 10), file("B2.TIF", LibraryStatus::LibDownloading, 5)];
        match row(&state).status {
            RowStatus::Downloading { done, total } => {
                assert_eq!((done, total), (15, 1000), "полосе нечем считаться");
            }
            _ => panic!("качающийся снимок назван не тем"),
        }

        // Закачка стала — снова счёт файлов на диске: стоять папке не по чему,
        // и «скачивается» было бы про то, чего уже нет.
        state.library.entries =
            vec![file("B1.TIF", LibraryStatus::LibComplete, 10), file("B2.TIF", LibraryStatus::LibPaused, 5)];
        assert!(matches!(row(&state).status, RowStatus::Partial { done: 2, .. }));

        // Ничего не заводили — снимок в хранилище.
        state.library.entries = Vec::new();
        assert!(matches!(row(&state).status, RowStatus::Remote));
    }
}
