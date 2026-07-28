//! Приобретение продукта: подпись у провайдера, закачка, пауза, удаление.
//!
//! У операции здесь одно имя на весь путь. Платформа делает владельцем задачи
//! того, кто публикует `network/on_fs_download` — то есть нас, — поэтому наш
//! `correlation_id` он же correlation_id запроса к провайдеру, он же
//! correlation_id запроса к network, он же `task_id` платформы. Отменяем мы
//! задачу напрямую (`tasks/on_cancel`), без посредника, и прогресс читаем
//! прямо из платформенных событий.
//!
//! Провайдер участвует ровно в одном шаге — подписывает адрес. Раскладка
//! хранения из библиотеки не выходит: путь подставляем мы, уже после подписи.

use crate::module::{Download, State};
use crate::module::storage;
use crate::module::catalog::{self, delete_entry, write_sidecar};
use crate::proto::data_library::{DownloadRequest, ItemRequest};
use crate::proto::data_provider::{SignRequest, SignedUrl};
use veldsdk::proto::network::{FsDownloadProgress, FsDownloadRequest, FsDownloadResponse};
use veldsdk::proto::tasks::{TaskCancelRequest, TaskFinished};

/// Скачать или докачать продукт. Куда он ляжет — решаем мы: раскладка
/// хранения наша, и заказчику её знать незачем.
pub fn on_download(state: &mut State, req: DownloadRequest) {
    if req.identifier.is_empty() { return; }
    let name = storage::name_from_identifier(&req.identifier);

    // Повторное нажатие, пока идёт предыдущая попытка (в том числе пока она
    // ждёт подписи). Раньше дубли отсекал провайдер — у него одного был id
    // операции; теперь id наш с самого начала, и окна без учёта нет.
    if state.downloads.values().any(|d| d.name == name) {
        veldsdk::log::info!(target: "handlers", "{} уже качается, повтор игнорируем", name);
        return;
    }

    // Сидкар — сразу, до подписи: даже если приложение упадёт на первом байте,
    // на диске уже будет известно, откуда файл, и запись о намерении не
    // пропадёт. total_bytes пишем, если он уже известен из прошлой попытки.
    let known_total = state.origins.get(&name).and_then(|o| o.total_bytes);
    write_sidecar(state, &name, &req.identifier, known_total);

    // Засеваем тем, что уже известно, а не нулями: у недокачанной записи
    // закачка продолжится с лежащих на диске байт, и «0 B» на возобновлении
    // было бы враньём. Берём размер ТОЛЬКО у недокачанной: у полной записи
    // `.part` нет, перекачка идёт с нуля.
    let done = state.entry_for(&name).filter(|e| e.is_partial).map(|e| e.size).unwrap_or(0);
    let total = state.total_bytes(&name);

    let correlation_id = state.downloads.begin(Download {
        identifier: req.identifier.clone(),
        name,
        done,
        total,
    });

    crate::calls::data_provider::on_sign(&SignRequest {
        identifier: req.identifier,
        correlation_id,
    });
    catalog::publish(state);
}

/// Адрес подписан — качаем. Путь на диске подставляется здесь: провайдер
/// раскладки хранения не знает и знать не должен.
pub fn on_signed(state: &mut State, signed: SignedUrl) {
    let Some(dl) = state.downloads.get(&signed.correlation_id) else { return };
    let name = dl.name.clone();

    if !signed.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "подпись для {} не удалась: {}", name, signed.error);
        // Задачи ещё нет — терминального события платформы не будет, снимаем
        // с учёта сами, иначе запись навсегда останется «качается».
        finish(state, &signed.correlation_id);
        return;
    }

    crate::calls::network::on_fs_download(&FsDownloadRequest {
        url: signed.url,
        path: storage::file_path(&name),
        headers: signed.headers,
        correlation_id: signed.correlation_id,
    });
}

/// Отмена — она же пауза: `.part` остаётся на диске, следующее «скачать»
/// продолжит с оборванного байта. Отменяем сами: задача наша.
pub fn on_cancel(state: &mut State, req: ItemRequest) {
    let Some((task_id, _)) = state.active_download(&req.name) else { return };
    crate::calls::tasks::on_cancel(&TaskCancelRequest { task_id: task_id.to_string() });
}

/// Удалить запись — полную, недокачанную или заявленную одним лишь сидкаром.
pub fn on_delete(state: &mut State, req: ItemRequest) {
    // Файл прямо сейчас качается — host держит `.part` открытым на запись,
    // удалять поверх активной записи нельзя. Сначала отменяем; сам delete
    // сработает по терминальному событию.
    if let Some((task_id, _)) = state.active_download(&req.name) {
        let task_id = task_id.to_string();
        state.pending_delete_on_cancel.insert(task_id.clone(), req.name);
        crate::calls::tasks::on_cancel(&TaskCancelRequest { task_id });
        return;
    }
    delete_entry(state, &req.name);
}

/// Broadcast-топик — сверяем correlation_id, чтобы не принять чужой ответ.
pub fn on_delete_result(state: &mut State, response: veldsdk::proto::fs::FsDeleteResult) {
    let Some(path) = state.pending_delete.take(&response.correlation_id) else { return };
    if !response.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "не удалось удалить {}: {}", path, response.error);
    }
    // Перечитываем в любом случае: при ошибке — чтобы показать то, что на
    // диске на самом деле, при успехе — чтобы запись ушла.
    catalog::rescan(state);
}

pub fn on_fs_download_progress(state: &mut State, event: FsDownloadProgress) {
    // event.progress игнорируем — доля выводится из байт у того, кто рисует.
    let Some(dl) = state.downloads.get_mut(&event.correlation_id) else { return };
    dl.done = event.downloaded_bytes;
    dl.total = event.total_bytes;

    // Content-Length пишем в сидкар, когда узнали новое значение (не на каждом
    // событии): иначе после рестарта «из скольки» снова неизвестно, пока не
    // начнётся новая закачка — сидкар это единственное, что переживает
    // перезапуск.
    if event.total_bytes > 0 {
        let (identifier, name) = (dl.identifier.clone(), dl.name.clone());
        if state.total_bytes(&name) != event.total_bytes {
            write_sidecar(state, &name, &identifier, Some(event.total_bytes));
        }
    }
    catalog::publish(state);
}

/// Доменный итог: закачка дошла до конца сама — успехом или ошибкой.
pub fn on_fs_download_result(state: &mut State, response: FsDownloadResponse) {
    if !response.error.is_empty() {
        let name = state.downloads.get(&response.correlation_id).map(|d| d.name.clone());
        if let Some(name) = name {
            veldsdk::log::warn!(target: "handlers", "закачка {} не удалась: {}", name, response.error);
        }
    }
    finish(state, &response.correlation_id);
}

/// Терминальное событие платформы. Доменный результат приходит первым и
/// снимает закачку с учёта, поэтому сюда доходят только отмены: при обрыве
/// задачи `fs_download_result` не публикуется вовсе.
pub fn on_task_finished(state: &mut State, event: TaskFinished) {
    if !event.cancelled { return; }
    finish(state, &event.task_id);
}

/// Снимает закачку с учёта и приводит каталог в соответствие диску.
///
/// Идемпотентна намеренно: терминальных источников два — доменный результат и
/// отмена, — и какой из них придёт, зависит от того, как закачка кончилась.
/// Ранние отказы (подпись не удалась, небезопасный путь) не порождают ни
/// одного из них, поэтому сюда же ведёт и путь ошибки подписи.
fn finish(state: &mut State, id: &str) {
    if state.downloads.take(id).is_none() { return; }

    // Корзину нажимали во время закачки — тогда это не отмена ради отмены,
    // а отложенное удаление: `.part` остался ровно там, где abort его бросил,
    // и теперь его можно безопасно удалить.
    if let Some(name) = state.pending_delete_on_cancel.take(id) {
        delete_entry(state, &name);
        return;
    }

    // Перечитываем каталог: размер `.part`/готового файла берётся с диска, а
    // не переносится руками из реестра в запись.
    catalog::rescan(state);
}
