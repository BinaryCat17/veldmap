//! Закачки: старт, отмена, удаление и события провайдера.

use crate::module::{Download, State};
use crate::module::storage;
use crate::module::catalog::{self, delete_entry, write_sidecar};
use crate::proto::data_library::{DownloadRequest, ItemRequest};
use crate::proto::data_provider::{CancelDownloadRequest, DownloadProgress, DownloadStarted, Downloaded};

/// Скачать или докачать продукт. Куда он ляжет — решаем мы: раскладка
/// хранения наша, и заказчику её знать незачем.
pub fn on_download(state: &mut State, req: DownloadRequest) {
    if req.identifier.is_empty() { return; }
    let name = storage::name_from_identifier(&req.identifier);

    // Сидкар — сразу, до ответа провайдера: даже если приложение упадёт на
    // первом байте, на диске уже будет известно, откуда файл, и запись о
    // намерении не пропадёт. total_bytes пишем, если он уже известен из
    // прошлой попытки: не нужно ждать первого progress, чтобы узнать
    // известное.
    let known_total = state.origins.get(&name).and_then(|o| o.total_bytes);
    write_sidecar(state, &name, &req.identifier, known_total);

    crate::calls::data_provider::on_download(&crate::proto::data_provider::DownloadRequest {
        identifier: req.identifier,
        destination: storage::file_path(&name),
    });
    catalog::publish(state);
}

/// Отмена — она же пауза: `.part` остаётся на диске, следующее «скачать»
/// продолжит с оборванного байта.
pub fn on_cancel(state: &mut State, req: ItemRequest) {
    let Some((task_id, _)) = state.active_download(&req.name) else { return };
    crate::calls::data_provider::on_cancel_download(&CancelDownloadRequest {
        task_id: task_id.to_string(),
    });
}

/// Удалить запись — полную, недокачанную или заявленную одним лишь сидкаром.
pub fn on_delete(state: &mut State, req: ItemRequest) {
    // Файл прямо сейчас качается — host держит `.part` открытым на запись,
    // удалять поверх активной записи нельзя. Сначала отменяем; сам delete
    // сработает в on_downloaded, когда придёт подтверждение отмены.
    if let Some((task_id, _)) = state.active_download(&req.name) {
        let task_id = task_id.to_string();
        state.pending_delete_on_cancel.insert(task_id.clone(), req.name);
        crate::calls::data_provider::on_cancel_download(&CancelDownloadRequest { task_id });
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

/// Провайдер сообщил, что закачка началась и зарегистрирована платформой.
pub fn on_download_started(state: &mut State, event: DownloadStarted) {
    let name = storage::name_from_identifier(&event.identifier);

    // Засеваем тем, что уже известно, а не нулями: до первого progress-события
    // запись описывается отсюда, и «0 B» на возобновлении недокачанного файла
    // было бы враньём — байты на диске уже есть, host продолжит именно с них.
    // Берём размер ТОЛЬКО у недокачанной записи: у полной `.part` нет,
    // re-download пойдёт с нуля.
    let done = state.entry_for(&name)
        .filter(|e| e.is_partial)
        .map(|e| e.size)
        .unwrap_or(0);

    state.downloads.insert(event.task_id, Download {
        identifier: event.identifier,
        name: name.clone(),
        done,
        total: state.total_bytes(&name),
    });
    catalog::publish(state);
}

pub fn on_download_progress(state: &mut State, event: DownloadProgress) {
    // event.progress игнорируем — доля выводится из байт у того, кто рисует.
    let Some(dl) = state.downloads.get_mut(&event.task_id) else { return };
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

pub fn on_downloaded(state: &mut State, event: Downloaded) {
    // Имя этой закачки — только в реестре, отдельной таблицы не держим.
    // Снимаем с учёта сразу: реестр описывает идущие закачки, а эта кончилась.
    let Some(dl) = state.downloads.take(&event.task_id) else { return };

    // Корзину нажимали во время закачки — тогда это не отмена ради отмены,
    // а отложенное удаление: `.part` остался ровно там, где abort его бросил,
    // и теперь его можно безопасно удалить.
    if let Some(name) = state.pending_delete_on_cancel.take(&event.task_id) {
        delete_entry(state, &name);
        return;
    }

    if !event.success && !event.cancelled {
        veldsdk::log::warn!(target: "handlers", "закачка {} не удалась: {}", dl.name, event.error);
    }

    // Перечитываем каталог: размер `.part`/готового файла берётся с диска, а
    // не переносится руками из реестра в запись.
    catalog::rescan(state);
}
