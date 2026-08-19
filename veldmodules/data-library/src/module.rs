//! data-library: каталог скачанного.
//!
//! Владеет всем локальным хранением — каталогом, суффиксом `.part`,
//! сидкарами `.origin` — и отдаёт наружу только выведенное состояние
//! (см. types.proto). Ни один потребитель не знает, как это лежит на диске.
//!
//! Состояние выводится, а не накапливается. Три независимых источника фактов:
//! снимок диска (fs/on_list), сидкары (что откуда взялось) и идущие закачки.
//! Ни один не патчится «оптимистично»: снимок перечитывается на каждом
//! терминальном событии. Хранить строки материализованно значит править их из
//! четырёх мест, и каждый пропущенный патч врёт в UI до следующего листинга.
//!
//! module.rs — фасад: State, init и реэкспорты обработчиков. Логика —
//! в catalog.rs (снимок и сидкары), download.rs (закачки) и open.rs.

pub mod storage;
pub mod catalog;
pub mod download;
pub mod open;

use std::collections::HashMap;
use storage::{LocalFile, OriginSidecar};

#[derive(serde::Deserialize, Clone)]
pub struct Config {}

pub struct State {
    /// Что лежит на диске: имя записи → что под ним найдено. Единственный
    /// писатель — catalog::on_list_result.
    ///
    /// Ключ — имя записи, а не имя файла: `foo` и `foo.part` — это одна запись
    /// каталога в двух состояниях, и держать их двумя строками значит завести
    /// два ответа на вопрос «что с этой записью», выбираемых порядком обхода
    /// каталога.
    pub snapshot: HashMap<String, LocalFile>,
    /// Содержимое сидкаров по имени записи. Кэш диска, а не отдельная истина:
    /// листинг подрезает его под то, что реально лежит в каталоге, поэтому
    /// удалённый мимо приложения файл не воскреснет записью о намерении.
    pub origins: HashMap<String, OriginSidecar>,
    /// Идущие закачки — доменный реестр, а не таблица корреляции: у записи
    /// есть прогресс, два возможных терминальных события и своё место в
    /// выводимом состоянии. Ключ — id операции, он же task_id задачи у
    /// платформы: операцию именуем мы, потому что мы же её и владелец
    /// (см. download.rs). Запись живёт от нажатия «скачать» до терминального
    /// события — включая окно ожидания подписи, когда задачи ещё нет.
    pub downloads: HashMap<String, Download>,
    /// Почему сорвалась последняя попытка, по имени записи. Живёт здесь, а не
    /// в сидкаре: причина — про попытку, а не про файл, и переживать
    /// перезапуск ей незачем. Перечитывание каталога переживает: обход диска
    /// про отказы ничего не знает и молча стёр бы их все.
    pub troubles: HashMap<String, String>,

    /// Ожидание fs/on_list — гасит устаревший ответ. Запрос здесь всегда
    /// один: обходов диска ровно столько, сколько нужно, чтобы состояние было
    /// верным, а не сколько было поводов перечитать (см. `catalog::rescan`).
    pub pending_list: veldsdk::Correlator<()>,
    /// Повод перечитать, пришедший, пока обход шёл. Один флаг, а не счётчик:
    /// сколько бы их ни набежало, догоняет их всех один обход — он видит диск
    /// целиком.
    pub list_again: bool,
    /// Вопрос провайдеру «к каким снимкам относятся эти ключи». Актуален только
    /// последний: сидкары дочитываются по одному, и каждый прочитанный делает
    /// вопрос полнее — отвечать по устаревшему списку незачем.
    pub pending_roots: veldsdk::Latest,
    /// Отданные fs чтения, ещё не отвеченные. Одна таблица на топик, а не по
    /// таблице на назначение (см. [`veldsdk::Correlator`]): назначение
    /// различает `ReadPurpose`.
    pub pending_reads: veldsdk::Correlator<ReadPurpose>,
    /// Ожидание fs/on_write сидкара.
    pub pending_sidecar_writes: veldsdk::Correlator<SidecarWrite>,
    /// Сидкары, ждущие, пока допишется предыдущий той же записи. По имени, а
    /// не очередью: сидкар пишется целиком, и промежуточные состояния диску не
    /// нужны — ждёт всегда последнее (см. `catalog::write_sidecar`).
    pub queued_sidecars: HashMap<String, OriginSidecar>,
    /// Открытия, пришедшие раньше первого обхода каталога: отвечать им
    /// «такой записи нет» значило бы отказать за то, что мы не успели
    /// прочитать диск (см. `open::resume`).
    pub deferred_opens: Vec<crate::module::open::Deferred>,
    /// Ожидание fs/on_delete — контекст: путь удаляемого файла.
    pub pending_delete: veldsdk::Correlator<String>,
}

/// Зачем библиотека читает файл. Оба чтения идут одним топиком fs/read и
/// возвращаются одним `core.ResourceOpened` — различает их только это.
pub enum ReadPurpose {
    /// Сидкар записи; контекст — её имя.
    Sidecar(String),
    /// Файл, открываемый заказчику: кому уйдёт владение и на какой запрос
    /// отвечаем.
    File(open::OpenFor),
}

/// Одна идущая закачка — единственный источник байтового прогресса, пока она
/// жива. После неё прогресс берётся с диска (размер `.part` в снимке).
pub struct Download {
    pub identifier: String,
    pub name: String,
    pub done: u64,
    /// 0, если сервер не прислал Content-Length.
    pub total: u64,
    /// Корзину нажали во время закачки. Удалить `.part` поверх активной записи
    /// нельзя (хост держит файл открытым), поэтому сначала убиваем закачку, а
    /// delete срабатывает по её терминальному ответу. Флаг здесь, а не
    /// отдельной таблицей: ключ у неё был бы тот же самый и читалась бы она на
    /// том же топике — то есть на один топик приходилось бы две таблицы, ровно
    /// то, от чего защищает правило у `pending_reads`.
    pub delete_when_done: bool,
}
// Доли (`progress` из DownloadProgress) здесь нет намеренно: она полностью
// выводится из done/total, а хранить её рядом значит завести два числа,
// которые могут разойтись.

/// Сидкар, отданный в fs/on_write и ещё не подтверждённый.
pub struct SidecarWrite {
    /// CPU-регион с телом сидкара — освобождается по ответу хоста.
    pub region: u64,
    /// Имя записи: пока запись в полёте, сидкара на диске может ещё не быть,
    /// и подрезка `origins` по листингу не должна принять его за удалённый.
    pub name: String,
}

pub fn hook_init(_config: Config) -> anyhow::Result<State> {
    let state = State {
        snapshot: HashMap::new(),
        origins: HashMap::new(),
        queued_sidecars: HashMap::new(),
        deferred_opens: Vec::new(),
        downloads: HashMap::new(),
        troubles: HashMap::new(),
        pending_list: veldsdk::Correlator::new(),
        list_again: false,
        pending_roots: veldsdk::Latest::new(),
        pending_reads: veldsdk::Correlator::new(),
        pending_sidecar_writes: veldsdk::Correlator::new(),
        pending_delete: veldsdk::Correlator::new(),
    };
    Ok(state)
}

impl State {
    /// Запись на диске с данным именем (полная или недокачанная).
    pub fn entry_for(&self, name: &str) -> Option<&LocalFile> {
        self.snapshot.get(name)
    }

    /// Забывает причины срыва у записей, которых больше нет.
    ///
    /// Причина принадлежит записи, а не имени: пережившая саму запись, она
    /// однажды приписалась бы новой под тем же именем — сорвавшейся не в этом
    /// запуске.
    pub fn forget_gone_troubles(&mut self) {
        let (origins, snapshot) = (&self.origins, &self.snapshot);
        self.troubles.retain(|name, _| origins.contains_key(name) || snapshot.contains_key(name));
    }

    /// Лежат ли байты записи под `.part`. Ответ один на всех спрашивающих:
    /// удаление и показ в файловом менеджере спрашивают одно и то же, и два
    /// умолчания у одного вопроса однажды разъехались бы.
    ///
    /// Чего в снимке нет вовсе — это `.part`: единственное, что могло
    /// остаться от закачки, сорвавшейся между обходами диска.
    pub fn is_partial(&self, name: &str) -> bool {
        self.entry_for(name).map_or(true, |file| file.is_partial)
    }

    /// Идущая закачка записи вместе с её task_id (нужен для отмены).
    pub fn active_download(&self, name: &str) -> Option<(&str, &Download)> {
        self.downloads.iter()
            .find(|(_, d)| d.name == name)
            .map(|(id, d)| (id.as_str(), d))
    }

    /// Ключ провайдера для записи, если сидкар прочитан.
    pub fn identifier_of(&self, name: &str) -> Option<&str> {
        self.origins.get(name).map(|o| o.identifier.as_str())
    }

    /// Снимок, к которому относится запись. Знает об этом только сидкар: имя на
    /// диске о снимке не говорит ничего.
    pub fn product_of(&self, name: &str) -> Option<&str> {
        self.origins.get(name).map(|o| o.product.as_str())
    }

    /// Сколько файлов в снимке этой записи; 0 — снимок целиком не обходили,
    /// и о его полноте сказать нечего.
    pub fn siblings_of(&self, name: &str) -> u32 {
        self.origins.get(name).map(|o| o.siblings).unwrap_or(0)
    }

    /// Ожидаемый полный размер из сидкара; 0 — Content-Length ещё не видели.
    pub fn total_bytes(&self, name: &str) -> u64 {
        self.origins.get(name).and_then(|o| o.total_bytes).unwrap_or(0)
    }
}

// -- Input handlers --
pub use catalog::{on_list, on_list_result, on_product_roots_result, on_snapshot, on_write_result};

/// fs/on_read_result — топик один, назначений два: каталог дочитывает сидкары,
/// open отдаёт файл заказчику. Развилка здесь, а не в схеме, потому что
/// различает их не контракт шины, а назначение, записанное при запросе.
pub fn on_read_result(state: &mut State, opened: veldsdk::proto::core::ResourceOpened) {
    match state.pending_reads.take(&veldsdk::correlation()) {
        Some(ReadPurpose::Sidecar(name)) => catalog::on_sidecar_read(state, name, &opened),
        Some(ReadPurpose::File(target)) => open::on_file_opened(target, &opened),
        None => veldsdk::resource::discard("fs/on_read_result", opened),
    }
}
pub use download::{on_download, on_pause, on_delete, on_delete_result,
                   on_signed, on_fs_download_progress, on_fs_download_result};
pub use open::{on_open, on_reveal};
