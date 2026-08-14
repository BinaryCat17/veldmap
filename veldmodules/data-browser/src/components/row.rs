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

/// Папка, в которой лежит ключ провайдера. Выводится из самого ключа — своего
/// поля под это нет ни у каталога, ни у библиотеки, — и написана здесь одна на
/// всех: её спрашивают и строка списка, и полоса под глобусом, а две копии
/// правила «где кончается путь» однажды ответят по-разному.
pub fn folder_of(identifier: &str) -> &str {
    let path = identifier.trim_end_matches('/');
    match path.rfind('/') {
        Some(cut) => &path[..cut],
        None => "",
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
    /// Файлы, из которых состоит снимок. Непусто только у строки-снимка,
    /// собранной из нескольких записей: раскрытая строка показывает их
    /// подстроками, закрытая — молчит о них.
    pub children: Vec<Row>,
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

    /// Путь папки, в которой лежит запись: по нему строки группируются, он же
    /// показывается в меню строки.
    pub fn folder(&self) -> &str {
        folder_of(&self.identifier)
    }

    /// Сколько байт этой записи уже на диске: у недокачанной — скачанное, у
    /// готовой — весь размер.
    pub fn stored(&self) -> u64 {
        // У строки с детьми своих байтов нет: она сумма своих файлов, и
        // спрашивать её собственный статус значило бы считать снимок пустым
        // ровно тогда, когда часть его уже лежит на диске.
        if !self.children.is_empty() {
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
    /// каталогом. Размера и времени у такой записи нет — за общим префиксом
    /// ключей в S3 не стоит ни того, ни другого.
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
            false => snapshot(key, files),
        })
        .chain(alone)
        .collect()
}

/// Строка снимка поверх его файлов: то, что о снимке можно сказать, сложено из
/// того, что известно о каждом файле.
fn snapshot(product: String, files: Vec<Row>) -> Row {
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
        (false, done, total) if done == total => RowStatus::Complete,
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
        }
    }

    /// Файлы одного снимка сходятся в одну строку, а то, что снимку не
    /// принадлежит, остаётся само по себе — это и есть весь смысл свёртки.
    #[test]
    fn files_of_one_snapshot_fold_into_one_row() {
        let rows = downloaded_rows(&LibraryState {
            entries: vec![
                entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10),
                entry("dem.tif", "", LibraryStatus::LibComplete, 7),
                entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 20),
            ],
        });

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
    #[test]
    fn single_file_snapshot_is_not_wrapped() {
        let mut only = entry("S1A_X.zip", "eodata/S1A_X.zip", LibraryStatus::LibComplete, 9);
        only.identifier = "eodata/S1A_X.zip".to_string();
        let rows = downloaded_rows(&LibraryState { entries: vec![only] });
        assert_eq!(rows.len(), 1);
        assert!(rows[0].children.is_empty(), "обёртки нет");
        assert_eq!(rows[0].name, "S1A_X.zip", "запись библиотеки осталась записью");
    }
}
