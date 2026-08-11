//! Снимок каталога и сидкары: чтение диска и вывод состояния библиотеки.

use crate::module::{ReadPurpose, SidecarWrite, State};
use crate::module::storage::{self, LocalFile, OriginSidecar};
use crate::proto::data_library::{LibraryEntry, LibraryRequest, LibraryState, LibraryStatus};
use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::{FsDeleteRequest, FsListRequest, FsReadRequest, FsWriteRequest, FsWriteResult};

/// «Перечитай каталог». Ответ придёт не отсюда, а из on_list_result: диск
/// отвечает не мгновенно, и врать про содержимое каталога по памяти — ровно
/// то, чего этот сервис не делает.
pub fn on_list(state: &mut State, _req: LibraryRequest) {
    rescan(state);
}

/// Перечитать каталог; результатом станет рассылка состояния.
pub fn rescan(state: &mut State) {
    let correlation_id = state.pending_list.begin(());
    crate::calls::fs::on_list(&FsListRequest {
        path: storage::DATA_DIR.to_string(),
    }, &correlation_id);
}

/// Снимок диска пришёл. Здесь же подрезаются сидкары и дочитываются те,
/// которых ещё нет в памяти.
pub fn on_list_result(state: &mut State, response: veldsdk::proto::fs::FsListResult) {
    if state.pending_list.take(&veldsdk::correlation()).is_none() { return }

    if !response.error.is_empty() {
        publish_error(&response.error);
        return;
    }

    state.snapshot = response.entries.iter()
        .filter_map(|e| LocalFile::from_entry(&e.name, e.size, e.modified))
        .collect();

    // origins — кэш диска, а не независимая истина: подрезаем под то, что
    // реально лежит в каталоге, иначе файл, удалённый мимо приложения, остался
    // бы записью о намерении до конца сессии. Исключение — сидкары, чья запись
    // ещё в полёте: на диске их закономерно нет, и срезать их значило бы
    // потерять запись только что начатой закачки.
    let on_disk: Vec<String> = response.entries.iter()
        .filter_map(|e| e.name.strip_suffix(storage::ORIGIN_SUFFIX))
        .map(str::to_string)
        .collect();
    let in_flight: Vec<&str> = state.pending_sidecar_writes.values()
        .map(|w| w.name.as_str())
        .collect();
    state.origins.retain(|name, _| {
        on_disk.contains(name) || in_flight.contains(&name.as_str())
    });

    // Дочитываем то, чего ещё нет в памяти. Сидкар — единственное, что
    // переживает рестарт, так что для файлов с прошлого запуска это
    // единственный способ узнать ключ провайдера и ожидаемый размер.
    let missing: Vec<String> = on_disk.into_iter()
        .filter(|name| !state.origins.contains_key(name))
        .filter(|name| !state.pending_reads.values()
            .any(|purpose| matches!(purpose, ReadPurpose::Sidecar(n) if n == name)))
        .collect();

    // Состояние отдаём сразу, не дожидаясь сидкаров: файлы на диске — уже
    // правда, а происхождение дочитается и придёт следующей рассылкой.
    publish(state);

    for name in missing {
        let correlation_id = state.pending_reads.begin(ReadPurpose::Sidecar(name.clone()));
        crate::calls::fs::on_read(&FsReadRequest {
            path: storage::origin_path(&name),
        }, &correlation_id);
    }
}

/// Сидкар записи `name` прочитан (ожидание уже снято с учёта, см.
/// module::on_read_result). Отсутствие сидкара — не ошибка: файл мог быть
/// скачан мимо приложения, просто докачать его будет нечем.
pub fn on_sidecar_read(state: &mut State, name: String, opened: &ResourceOpened) {
    let Some(handle) = &opened.handle else { return };

    // RAII-гард: регион освобождается при любом выходе ниже.
    let resource = veldsdk::OwnedResource::new(handle.clone());
    let Ok(bytes) = veldsdk::abi::resource_read(resource.id(), 0, handle.size) else { return };
    let Ok(sidecar) = serde_json::from_slice::<OriginSidecar>(&bytes) else { return };
    if sidecar.provider != storage::PROVIDER_NAME { return }

    state.origins.insert(name, sidecar);
    publish(state);
}

/// Пишет сидкар. Он ложится на диск ДО старта закачки, поэтому переживает
/// сбой, случившийся до появления первых байт.
pub fn write_sidecar(state: &mut State, name: &str, identifier: &str, total_bytes: Option<u64>) {
    let sidecar = OriginSidecar {
        provider: storage::PROVIDER_NAME.to_string(),
        identifier: identifier.to_string(),
        total_bytes,
    };
    state.origins.insert(name.to_string(), sidecar.clone());

    let Ok(json) = serde_json::to_vec(&sidecar) else { return };
    let Some(region) = veldsdk::abi::resource_alloc_cpu(json.len() as u64) else { return };
    if let Err(e) = veldsdk::abi::resource_write(region, 0, &json) {
        veldsdk::log::warn!(target: "handlers", "сидкар {} не записан: {}", name, e);
        return;
    }

    // Гранта на "fs" не нужно (и он бы не сработал: fs — хостовый нативный
    // модуль, не wasm-плагин, dispatcher.instance_of его не резолвит). on_write
    // проверяет доступ по requestor_id — паблишеру события, то есть нам же,
    // а мы и так владелец региона.
    let correlation_id = state.pending_sidecar_writes.begin(SidecarWrite {
        region,
        name: name.to_string(),
    });
    crate::calls::fs::on_write(&FsWriteRequest {
        path: storage::origin_path(name),
        handle: Some(veldsdk::proto::core::ResourceHandle { id: region, size: json.len() as u64 }),
    }, &correlation_id);
}

/// fs прочитал наш буфер (успешно или нет) — регион больше не нужен.
pub fn on_write_result(state: &mut State, response: FsWriteResult) {
    let Some(write) = state.pending_sidecar_writes.take(&veldsdk::correlation()) else { return };
    drop(veldsdk::OwnedResource::from_raw_id(write.region));
    if !response.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "сидкар не сохранён: {}", response.error);
    }
}

/// Удаляет запись вместе с её сидкаром — иначе `.origin` остался бы сиротой
/// и файл воскрес бы записью о намерении.
pub fn delete_entry(state: &mut State, name: &str) {
    state.origins.remove(name);
    // Сидкар удаляем без учёта: ответ на него никого не интересует — судьбу
    // записи решает удаление самих данных ниже.
    crate::calls::fs::on_delete(&FsDeleteRequest {
        path: storage::origin_path(name),
    }, "");

    // Недокачанный лежит под `.part`, готовый — под своим именем.
    let path = match state.entry_for(name) {
        Some(entry) => entry.path.clone(),
        None => storage::part_path(name),
    };
    let correlation_id = state.pending_delete.begin(path.clone());
    crate::calls::fs::on_delete(&FsDeleteRequest { path }, &correlation_id);
}

// ── Вывод состояния ────────────────────────────────────────────

/// Рассылает состояние библиотеки — единственный способ рассказать о нём.
pub fn publish(state: &State) {
    crate::emit::on_state(&LibraryState { entries: entries(state), error: String::new() });
}

fn publish_error(error: &str) {
    crate::emit::on_state(&LibraryState { entries: Vec::new(), error: error.to_string() });
}

/// Выводит записи из трёх источников. Ни один не является надмножеством
/// остальных: закачка может идти до появления файла, а сидкар — остаться
/// без данных.
fn entries(state: &State) -> Vec<LibraryEntry> {
    let mut names: Vec<String> = state.snapshot.iter().map(|f| f.name.clone()).collect();
    for name in state.origins.keys() {
        if !names.contains(name) { names.push(name.clone()); }
    }
    for dl in state.downloads.values() {
        if !names.contains(&dl.name) { names.push(dl.name.clone()); }
    }
    names.sort();

    names.into_iter().map(|name| {
        let known_total = state.total_bytes(&name);
        let entry = state.entry_for(&name);

        let (status, done, total) = if let Some((_, dl)) = state.active_download(&name) {
            // Пока закачка жива, байты только отсюда: снимок диска обновляется
            // лишь на терминальных событиях и во время закачки заведомо отстал.
            let total = if dl.total > 0 { dl.total } else { known_total };
            (LibraryStatus::LibDownloading, dl.done, total)
        } else if let Some(e) = entry {
            if e.is_partial {
                (LibraryStatus::LibPaused, e.size, known_total)
            } else {
                (LibraryStatus::LibComplete, e.size, e.size)
            }
        } else {
            // Сидкар есть, данных нет — намерение пользователя, которое не
            // должно молча пропасть из списка.
            (LibraryStatus::LibPaused, 0, known_total)
        };

        LibraryEntry {
            identifier: state.identifier_of(&name).unwrap_or_default().to_string(),
            modified: entry.map(|e| e.modified).unwrap_or(0),
            name,
            done,
            total,
            status: status as i32,
        }
    }).collect()
}
