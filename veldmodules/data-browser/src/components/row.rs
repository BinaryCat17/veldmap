//! components/row.rs — строка списка: то, что показывают все три вида.
//!
//! Состояние записи здесь не выводится: его присылает data-library, которая
//! одна и знает, что лежит на диске, что качается и откуда взялось. Осталось
//! сопоставление «запись каталога → то, что рисуем».

use crate::module::state::library::{status_of, LibraryState};
use crate::proto::data_library::{LibraryEntry, LibraryStatus};

/// Состояние строки. Сумма-тип, а не набор булевых полей: сочетания вроде
/// «недокачан, но байт ноль и задачи нет» перестают быть выразимыми.
#[derive(Clone, PartialEq)]
pub enum RowStatus {
    /// Записи в библиотеке нет — есть только у провайдера.
    Remote,
    Downloading { done: u64, total: u64 },
    /// Начато, но не доведено. `done: 0` — закачка сорвалась до первых байт.
    Paused { done: u64, total: u64 },
    Complete,
    /// Папка, часть содержимого которой уже на диске. Сколько там файлов
    /// всего, каталог не говорит, поэтому сказано только скачанное.
    Partial { done: usize },
}

impl RowStatus {
    /// Сколько сделано из скольки, если это вообще идёт. `None` — полосе
    /// загрузки в этой строке места нет.
    pub fn progress(&self) -> Option<(u64, u64)> {
        match self {
            RowStatus::Downloading { done, total } | RowStatus::Paused { done, total } if *total > 0 => {
                Some((*done, *total))
            }
            _ => None,
        }
    }
}

/// Чем запись является для приложения. Сумма-тип, а не пара булевых полей:
/// «папка» и «снимок» — не два независимых свойства, а один род. Парой их не
/// выразить: снимок бывает и каталогом, и одним объектом, а «папка, которая не
/// снимок» и «снимок, который не папка» — это разные строки с разными
/// действиями, а не сочетания флагов.
#[derive(Clone, Copy, PartialEq)]
pub enum RowKind {
    File,
    /// Папка пути: внутри снимки или другие папки. Сама по себе не показывает
    /// ничего — в неё только заходят.
    Folder,
    /// Снимок — одна логическая единица. Лежит он развёрнутым каталогом
    /// (.SAFE, .SEN3) или одним объектом, показу безразлично: и то, и другое
    /// декодер собирает в один растр. `folder` говорит лишь о том, есть ли
    /// куда зайти внутрь.
    Product { folder: bool },
}

impl RowKind {
    /// Есть ли внутри содержимое, в которое заходят. У снимка-каталога оно
    /// есть, но заход — не главное его действие: смотрят снимок целиком.
    pub fn is_folder(self) -> bool {
        matches!(self, RowKind::Folder | RowKind::Product { folder: true })
    }

    /// Снимок ли это — то, что кладут на шар и смотрят одной картинкой.
    pub fn is_product(self) -> bool {
        matches!(self, RowKind::Product { .. })
    }
}

/// Голый ключ хранилища: без завершающего слэша, которым листинг помечает
/// папку.
///
/// Слэш — признак строки листинга, а не часть пути: один и тот же продукт
/// приходит из каталога со слэшем, а из выдачи поиска без него. Правило
/// написано здесь одно на всех — от него зависят и папка записи, и путь
/// листинга, и ключ снимка, и сравнение строки с ключом перехода, а пять его
/// копий однажды сочли бы один снимок за два.
pub fn bare(key: &str) -> &str {
    key.trim_end_matches('/')
}

/// Папка, в которой лежит ключ провайдера. Выводится из самого ключа — своего
/// поля под это нет ни у каталога, ни у библиотеки, — и написана здесь одна на
/// всех: её спрашивают и строка списка, и полоса под глобусом, а две копии
/// правила «где кончается путь» однажды ответят по-разному.
pub fn folder_of(identifier: &str) -> &str {
    let path = bare(identifier);
    match path.rfind('/') {
        Some(cut) => &path[..cut],
        None => "",
    }
}

/// Путь, которым листают содержимое этой записи: ключ папки — всегда со
/// слэшем. Без него листинг по префиксу показал бы саму папку вместо того, что
/// в ней лежит.
pub fn folder_path(identifier: &str) -> String {
    format!("{}/", bare(identifier))
}

/// Чем смотреть снимок, о котором известен один ключ.
///
/// Правило одно на всех, кто предлагает «смотреть» не из строки списка: полоса
/// под шаром и список слоёв. Строка решает это сама — род и запись библиотеки
/// записаны прямо в ней.
///
/// Порядок вопросов — от дешёвого к дорогому. Скачанное смотрят с диска: файл
/// под рукой, и ходить за ним по сети значит ждать того, что уже есть. Снимок,
/// лежащий каталогом, открывается через провайдера — растр внутри выбирает он,
/// потому что `GET` по пути каталога отвечает 404. Остальное открывается прямо.
pub fn preview_of(
    library: &LibraryState,
    identifier: &str,
    folder: bool,
) -> crate::module::ViewMsg {
    use crate::module::ViewMsg;
    if let Some(entry) = library.by_identifier(identifier)
        && status_of(entry) == LibraryStatus::LibComplete
    {
        return ViewMsg::Preview(entry.name.clone());
    }
    match folder {
        true => ViewMsg::PreviewProduct(identifier.to_string()),
        false => ViewMsg::PreviewRemote(identifier.to_string()),
    }
}

pub struct Row {
    /// Ключ провайдера; пустой — неизвестен, тогда докачка и просмотр
    /// удалённого не предлагаются.
    pub identifier: String,
    /// Имя записи в библиотеке — ключ для удаления и просмотра. Пустое, если
    /// записи ещё нет (файл не скачан).
    pub name: String,
    /// Отображаемое имя.
    pub title: String,
    pub kind: RowKind,
    /// Размер в байтах; 0 — неизвестен.
    pub size: u64,
    /// Время файла, unix-секунды; 0 — неизвестно.
    pub date: i64,
    /// Чем запись считает каталог: у снимка это тип продукта («S2MSI2A»).
    /// Пусто — каталог не сказал, и вид выводится из рода и имени
    /// (см. [`Row::format`]).
    pub product_type: String,
    /// Снимок, к которому относится запись; пусто — она сама по себе. Едет с
    /// ней до самой закачки: библиотека записывает файл в тот снимок, из
    /// которого его позвали, а вывести это из ключа она не может — раскладку
    /// бакета знает провайдер (см. `ListEntry.product`).
    pub product: String,
    /// Содержимое строки: файлы снимка или записи раскрытой папки.
    ///
    /// У сложенной строки они есть всегда — она из них и сложена, и по ним
    /// считается всё, что о ней известно (см. [`Row::stored`]); у папки
    /// каталога наполняются только раскрытые, потому что до раскрытия их и не
    /// спрашивали. Показывать содержимое или молчать о нём — дело не этого
    /// поля, а раскладки (см. `arrange::expand`).
    pub children: Vec<Row>,
    /// Строка сложена из записей библиотеки, а не является записью. Качать и
    /// открывать нужно её файлы: её ключ — путь снимка в хранилище, и послать
    /// его в закачку значит попросить скачать папку одним объектом.
    ///
    /// Отдельным полем, а не «есть дети»: детей заводит и раскрытая папка
    /// каталога, а она как раз обычная запись, и заход внутрь у неё никто не
    /// отнимает.
    pub folded: bool,
    /// Содержимое раскрытой папки ещё едет. Пустота под ней читается как
    /// «здесь ничего нет», и это неправда до конца листинга.
    pub loading: bool,
    pub status: RowStatus,
}

impl Row {
    /// Устойчивое имя строки в списке — им она сопоставляется между кадрами
    /// (см. `Element::key`) и им же адресуется её меню. Ключ провайдера, а без
    /// него имя записи: пустыми оба сразу не бывают, иначе строке неоткуда
    /// взяться.
    pub fn key(&self) -> &str {
        if self.identifier.is_empty() { &self.name } else { &self.identifier }
    }

    /// Ключ снимка, каким его знает провайдер (см. [`bare`]). Им снимок
    /// отмечают на шаре, кладут на него и открывают. Пуст у записи, которой
    /// провайдер не знает вовсе, — такую ни очертить, ни показать.
    pub fn snapshot_key(&self) -> &str {
        bare(&self.identifier)
    }

    /// Та ли это строка, о которой говорит ключ перехода. Сравнивается голыми
    /// ключами по той же причине, по какой они и голые: слэш есть в каталоге и
    /// нет в выдаче, а строка одна и та же.
    pub fn named(&self, key: &str) -> bool {
        !key.is_empty() && bare(self.key()) == bare(key)
    }

    /// Снимок ли это — то, у чего есть контур, растры и своя единица показа.
    /// Род отвечает на это почти всегда; исключение — скачанный файл, который
    /// сам себе снимок: записью библиотеки он остаётся файлом
    /// (см. `downloaded_rows`).
    pub fn is_snapshot(&self) -> bool {
        self.kind.is_product()
            || (!self.product.is_empty() && self.product == self.snapshot_key())
    }

    /// Есть ли что раскрыть под этой строкой: файлы снимка или содержимое
    /// папки. У папки каталога оно ещё не приехало — раскрытие его и спросит.
    pub fn expandable(&self) -> bool {
        self.folded || self.kind.is_folder()
    }

    /// Путь папки, в которой лежит запись: по нему строки группируются, он же
    /// показывается в меню строки.
    pub fn folder(&self) -> &str {
        folder_of(&self.identifier)
    }

    /// Сколько байт этой записи уже на диске: у недокачанной — скачанное, у
    /// готовой — весь размер.
    pub fn stored(&self) -> u64 {
        // У сложенной строки своих байтов нет: она сумма своих файлов, и
        // спрашивать её собственный статус значило бы считать снимок пустым
        // ровно тогда, когда часть его уже лежит на диске.
        if self.folded {
            return self.children.iter().map(Row::stored).sum();
        }
        match &self.status {
            RowStatus::Complete => self.size,
            RowStatus::Downloading { done, .. } | RowStatus::Paused { done, .. } => *done,
            RowStatus::Remote | RowStatus::Partial { .. } => 0,
        }
    }

    /// Колонка «формат»: то, чем запись назвал каталог, а без этого — род
    /// записи, а без него — расширение прописными. Тип от каталога старше
    /// всего остального: снимок подписывается своим типом («S2MSI2A»), и
    /// только безымянному достаётся слово строчными — это не расширение, и
    /// набрано оно поэтому иначе.
    pub fn format(&self) -> String {
        if !self.product_type.is_empty() {
            return self.product_type.clone();
        }
        match self.kind {
            RowKind::Product { .. } => return "снимок".to_string(),
            RowKind::Folder => return "папка".to_string(),
            RowKind::File => {}
        }
        match self.title.rfind('.') {
            Some(dot) => self.title[dot + 1..].to_uppercase(),
            None => String::new(),
        }
    }

    /// Строка того, что содержит другое: папки пути или снимка, разложенного
    /// каталогом. Размера и времени за ней не стоит — в S3 папка это общий
    /// префикс ключей, и ни того, ни другого за ним нет; у снимка их знает
    /// каталог, и приписывает их вызывающий (см. `components::rows::from_key`).
    pub fn container_row(identifier: String, title: String, status: RowStatus, kind: RowKind) -> Row {
        Row {
            identifier,
            name: String::new(),
            title,
            kind,
            size: 0,
            date: 0,
            product_type: String::new(),
            product: String::new(),
            children: Vec::new(),
            folded: false,
            loading: false,
            status,
        }
    }

    /// Строка из записи библиотеки — вид «Скачанное».
    pub fn from_entry(entry: &LibraryEntry) -> Row {
        let status = match status_of(entry) {
            LibraryStatus::LibDownloading => RowStatus::Downloading { done: entry.done, total: entry.total },
            LibraryStatus::LibPaused => RowStatus::Paused { done: entry.done, total: entry.total },
            LibraryStatus::LibComplete => RowStatus::Complete,
        };
        Row {
            identifier: entry.identifier.clone(),
            name: entry.name.clone(),
            // Имя записи — путь внутри снимка, и целиком оно в строке не нужно:
            // снимок уже назван строкой выше, а под ним читают файл. Ключом при
            // этом остаётся имя целиком (поле `name`) — им запись адресуют.
            title: entry.name.rsplit('/').next().unwrap_or(&entry.name).to_string(),
            kind: RowKind::File,
            size: if entry.total > 0 { entry.total } else { entry.done },
            date: entry.modified,
            product_type: String::new(),
            // Снимок едет с записью дальше: продолжение закачки уходит тем же
            // сообщением, и потерянный здесь снимок стёр бы принадлежность в
            // сидкаре — файл вывалился бы из своего снимка молча.
            product: entry.product.clone(),
            children: Vec::new(),
            folded: false,
            loading: false,
            status,
        }
    }

    /// Строка объекта каталога провайдера: состояние подтягивается из
    /// библиотеки, если такая запись там уже есть.
    ///
    /// Род приходит параметром, а не выводится здесь: снимком объект делает
    /// раскладка хранилища, а знает её только провайдер (см. `s3::product_root`).
    pub fn remote(
        library: &LibraryState,
        identifier: String,
        title: String,
        size: u64,
        date: i64,
        kind: RowKind,
    ) -> Row {
        match library.by_identifier(&identifier) {
            Some(entry) => Row {
                title,
                kind,
                // Размер и время у каталога точнее: библиотека знает только то,
                // что успело лечь на диск.
                size: if size > 0 { size } else { entry.total.max(entry.done) },
                date: if date > 0 { date } else { entry.modified },
                ..Row::from_entry(entry)
            },
            None => Row {
                identifier,
                name: String::new(),
                title,
                kind,
                size,
                date,
                product_type: String::new(),
                product: String::new(),
                children: Vec::new(),
                folded: false,
                loading: false,
                status: RowStatus::Remote,
            },
        }
    }
}

/// Все строки вида «Скачанное»: файлы одного снимка сведены в одну строку, а
/// то, что снимку не принадлежит, остаётся само по себе.
///
/// Свёртка здесь, а не в библиотеке: она ведёт учёт файлам — это её предмет, и
/// один файл там одна запись, — а «снимок» это то, чем его показывают. Знание,
/// какому снимку файл принадлежит, библиотека при этом хранит и отдаёт
/// (`LibraryEntry.product`): вывести его из имени она не могла бы.
pub fn downloaded_rows(library: &LibraryState) -> Vec<Row> {
    let mut products: Vec<(String, Vec<Row>)> = Vec::new();
    let mut alone: Vec<Row> = Vec::new();

    for entry in &library.entries {
        let row = Row::from_entry(entry);
        if entry.product.is_empty() {
            alone.push(row);
            continue;
        }
        // Порядок снимков — порядок первой встреченной записи: библиотека
        // отдаёт их уже в своём порядке, и пересортировать их всё равно
        // предстоит показу (см. `arrange`).
        match products.iter_mut().find(|(key, _)| key == &entry.product) {
            Some((_, files)) => files.push(row),
            None => products.push((entry.product.clone(), vec![row])),
        }
    }

    products
        .into_iter()
        .map(|(key, mut files)| match files.len() == 1 && files[0].identifier == key {
            // Снимок из одного файла — это и есть тот файл: обёртка над ним
            // носила бы тот же ключ строки, и список сопоставлял бы состояние
            // виджетов между ней и её же ребёнком.
            true => files.remove(0),
            // Состав снимка спрашиваем у библиотеки, а не складываем здесь:
            // правило «сколько файлов в снимке» одно на всех, и второй его
            // носитель разошёлся бы с первым (см. `LibraryState::snapshot`).
            false => {
                let (_, siblings) = library.snapshot(&key);
                snapshot(key, siblings, files)
            }
        })
        .chain(alone)
        .collect()
}

/// Строка снимка поверх его файлов: то, что о снимке можно сказать, сложено из
/// того, что известно о каждом файле, — кроме одного. Сколько файлов в снимке
/// всего (`siblings`), из записей не выводится: их столько, сколько качали.
fn snapshot(product: String, siblings: u32, files: Vec<Row>) -> Row {
    let done = files.iter().filter(|file| matches!(file.status, RowStatus::Complete)).count();
    let downloading = files.iter().any(|file| matches!(file.status, RowStatus::Downloading { .. }));
    let size = files.iter().map(Row::stored).sum();
    // Время снимка — время самого свежего его файла: снимок «появился на
    // диске» тогда, когда лёг последний из них.
    let date = files.iter().map(|file| file.date).max().unwrap_or(0);

    let status = match (downloading, done, files.len()) {
        // Пока хоть один файл едет, снимок едет весь: сумма байтов по частям
        // была бы правдой о файлах, а не о нём.
        (true, ..) => RowStatus::Downloading { done: size, total: 0 },
        // «На диске» — только когда известно, из скольки файлов снимок состоит,
        // и все они здесь и доведены. Без этого числа доведённым выглядел бы
        // всякий снимок, у которого доведено скачанное, — три файла из
        // двадцати шести читались бы как целый снимок.
        (false, done, total) if siblings > 0 && done == total && total as u32 == siblings => {
            RowStatus::Complete
        }
        // Не доведён ни один — снимок оборван, а не «частично скачан»: зелёная
        // строка с нулём на диске обещает то, чего нет, и проходит отбор «на
        // диске» вместе с целыми.
        (false, 0, _) => RowStatus::Paused { done: size, total: 0 },
        (false, done, _) => RowStatus::Partial { done },
    };

    Row {
        title: product.rsplit('/').next().unwrap_or(&product).to_string(),
        identifier: product.clone(),
        name: String::new(),
        kind: RowKind::Product { folder: true },
        size,
        date,
        product_type: String::new(),
        product,
        children: files,
        folded: true,
        loading: false,
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::data_library::{LibraryEntry, LibraryStatus};

    fn entry(name: &str, product: &str, status: LibraryStatus, done: u64) -> LibraryEntry {
        LibraryEntry {
            name: name.to_string(),
            identifier: format!("eodata/{}/{}", product, name),
            product: product.to_string(),
            done,
            total: done,
            status: status as i32,
            modified: 0,
            siblings: 0,
        }
    }

    /// Те же записи, но снимок к этому времени обойдён: в нём столько файлов,
    /// сколько сказано. Число носит каждая запись — оно и приходит к ним по
    /// одной, в сидкары (см. data-library::catalog::on_snapshot).
    fn walked(files: u32, mut entries: Vec<LibraryEntry>) -> Vec<LibraryEntry> {
        for entry in &mut entries {
            entry.siblings = files;
        }
        entries
    }

    /// Файлы одного снимка сходятся в одну строку, а то, что снимку не
    /// принадлежит, остаётся само по себе — это и есть весь смысл свёртки.
    #[test]
    fn files_of_one_snapshot_fold_into_one_row() {
        let mut entries = walked(2, vec![
            entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10),
            entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 20),
        ]);
        entries.push(entry("dem.tif", "", LibraryStatus::LibComplete, 7));
        let rows = downloaded_rows(&LibraryState { entries });

        assert_eq!(rows.len(), 2, "снимок и одиночный файл");
        let snapshot = &rows[0];
        assert_eq!(snapshot.title, "S2B_X.SAFE");
        assert!(snapshot.kind.is_product());
        assert_eq!(snapshot.children.len(), 2);
        // Размер снимка — сумма его файлов, а не размер какого-то одного.
        assert_eq!(snapshot.size, 30);
        assert!(matches!(snapshot.status, RowStatus::Complete));

        // Одиночный файл снимком не притворяется и детей не заводит.
        assert_eq!(rows[1].title, "dem.tif");
        assert!(!rows[1].kind.is_product());
        assert!(rows[1].children.is_empty());
    }

    /// Пока хоть один файл едет, едет весь снимок: «3 на диске» рядом с идущей
    /// закачкой говорило бы о файлах, а не о нём.
    #[test]
    fn snapshot_is_downloading_while_any_file_is() {
        let rows = downloaded_rows(&LibraryState {
            entries: vec![
                entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10),
                entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibDownloading, 5),
            ],
        });
        assert!(matches!(rows[0].status, RowStatus::Downloading { .. }));

        // Ни один не идёт, но и не все доведены — сказано, сколько доведено.
        let rows = downloaded_rows(&LibraryState {
            entries: vec![
                entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10),
                entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibPaused, 5),
            ],
        });
        assert!(matches!(rows[0].status, RowStatus::Partial { done: 1 }));
    }

    /// Целым снимок называется только по обходу: доведённые файлы говорят,
    /// сколько скачали, а не сколько в снимке есть.
    #[test]
    fn snapshot_is_whole_only_when_its_size_is_known() {
        let files = vec![
            entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10),
            entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 20),
        ];
        // Снимок не обходили: два доведённых файла — это два файла на диске.
        let rows = downloaded_rows(&LibraryState { entries: files.clone() });
        assert!(matches!(rows[0].status, RowStatus::Partial { done: 2 }));
        // Обошли и насчитали три — двух мало.
        let rows = downloaded_rows(&LibraryState { entries: walked(3, files.clone()) });
        assert!(matches!(rows[0].status, RowStatus::Partial { done: 2 }));
        // Насчитали два — снимок на диске целиком.
        let rows = downloaded_rows(&LibraryState { entries: walked(2, files) });
        assert!(matches!(rows[0].status, RowStatus::Complete));
    }

    /// Снимок, у которого не доведён ни один файл, оборван — а не «частично
    /// скачан»: зелёная строка с нулём на диске обещала бы то, чего нет.
    #[test]
    fn snapshot_of_interrupted_files_says_it_is_interrupted() {
        let rows = downloaded_rows(&LibraryState {
            entries: vec![
                entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibPaused, 3),
                entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibPaused, 4),
            ],
        });
        assert!(matches!(rows[0].status, RowStatus::Paused { .. }));
        // Байты при этом не теряются: у строки с детьми они сумма детей.
        assert_eq!(rows[0].stored(), 7);
    }

    /// Снимок из одного файла — это и есть тот файл: обёртка над ним носила бы
    /// тот же ключ строки, что и её единственный ребёнок.
    ///
    /// Записью библиотеки он при этом остаётся файлом, а снимком его делает
    /// совпадение ключей: род об этом сказать не может, а контур и шар у него
    /// есть наравне с прочими снимками.
    #[test]
    fn single_file_snapshot_is_not_wrapped() {
        let mut only = entry("S1A_X.zip", "eodata/S1A_X.zip", LibraryStatus::LibComplete, 9);
        only.identifier = "eodata/S1A_X.zip".to_string();
        let rows = downloaded_rows(&LibraryState { entries: vec![only] });
        assert_eq!(rows.len(), 1);
        assert!(rows[0].children.is_empty(), "обёртки нет");
        assert_eq!(rows[0].name, "S1A_X.zip", "запись библиотеки осталась записью");
        assert!(rows[0].is_snapshot(), "снимком его делает совпадение ключей");
        assert!(!rows[0].expandable(), "раскрывать в нём нечего");
    }

    /// Сложенная строка раскрывается, но сама записью не является: её файлы
    /// качают и открывают по одному, а её собственный ключ — путь снимка.
    #[test]
    fn a_folded_snapshot_expands_but_is_not_a_record() {
        let rows = downloaded_rows(&LibraryState {
            entries: vec![
                entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10),
                entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 20),
            ],
        });
        assert!(rows[0].folded);
        assert!(rows[0].expandable());
        assert!(rows[0].name.is_empty(), "своего имени в библиотеке у неё нет");
        // Файлы под ней — обычные записи, и раскрывать в них нечего.
        assert!(!rows[0].children[0].folded);
        assert!(!rows[0].children[0].expandable());
    }

    /// Слэш папки в ключе перехода в счёт не идёт: в каталоге он есть, а в
    /// выдаче поиска нет — строка при этом одна и та же.
    #[test]
    fn a_row_answers_to_its_key_with_or_without_the_folder_slash() {
        let row = Row::container_row(
            "eodata/S2B_X.SAFE/".to_string(),
            "S2B_X.SAFE".to_string(),
            RowStatus::Remote,
            RowKind::Product { folder: true },
        );
        assert!(row.named("eodata/S2B_X.SAFE"));
        assert!(row.named("eodata/S2B_X.SAFE/"));
        assert!(!row.named("eodata/S2B_Y.SAFE"));
        assert!(!row.named(""), "пустой ключ не называет никого");
    }

    /// Смотреть скачанное надо с диска: файл под рукой, и ходить за ним по
    /// сети значит ждать того, что уже есть.
    #[test]
    fn preview_prefers_the_downloaded_file() {
        use crate::module::ViewMsg;
        let key = "eodata/S1A_X.SAFE/quick-look.png";

        let done = LibraryState {
            entries: vec![entry("quick-look.png", "S1A_X.SAFE", LibraryStatus::LibComplete, 4)],
        };
        assert!(
            matches!(preview_of(&done, key, false), ViewMsg::Preview(name) if name == "quick-look.png")
        );

        // Недокачанного на диске ещё нет — смотреть его надо из хранилища.
        let started = LibraryState {
            entries: vec![entry("quick-look.png", "S1A_X.SAFE", LibraryStatus::LibPaused, 4)],
        };
        assert!(matches!(preview_of(&started, key, false), ViewMsg::PreviewRemote(_)));

        // Снимок, лежащий каталогом, открывает провайдер: GET по пути каталога
        // отвечает 404, и растр внутри выбирает он.
        assert!(matches!(
            preview_of(&started, "eodata/S1A_X.SAFE", true),
            ViewMsg::PreviewProduct(_)
        ));
    }
}
