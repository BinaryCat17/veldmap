//! Пополнение кэша и запись файлов.
//!
//! Записи fire-and-forget: тело уезжает CPU-ресурсом в fs, владение живёт в
//! таблице ожидания до подтверждения, а неудача — строка в логе и будущий
//! промах, не поломка. Атомарность на стороне fs (tmp + rename): на диске
//! не бывает полутайла.

use veldsdk::proto::core::ResourceHandle;
use veldsdk::proto::fs::{FsWriteRequest, FsWriteResult};

use crate::module::{evict, layout, State};
use crate::proto::tile_cache::StoreTile;

/// CPU-регион, уехавший в fs/on_write и ещё не подтверждённый.
pub struct PendingWrite {
    /// Освобождается по ответу.
    pub region: u64,
    /// Куда писали — для лога о неудаче.
    pub path: String,
}

pub fn on_store(state: &mut State, tile: StoreTile) {
    if !layout::valid_key(&tile.fingerprint) {
        veldsdk::log::warn!(target: "handlers", "store с негодным ключом: '{}'", tile.fingerprint);
        return;
    }
    // Проверяется то, что ляжет на диск, — байты. Размеры спрашивать здесь
    // не у кого и незачем: несёт их сам кодек, и разбирает их чтение
    // (`query::decode`), где они и отвечают за свою правдоподобность.
    if tile.qoi.is_empty() || tile.qoi.len() > layout::MAX_TILE_BYTES {
        veldsdk::log::warn!(target: "handlers",
            "store {}:{}:{}:{} отвергнут: {} байт",
            tile.fingerprint, tile.level, tile.x, tile.y, tile.qoi.len());
        return;
    }

    touch(state, &tile.fingerprint);
    write_file(state, layout::tile_path(&tile.fingerprint, tile.level, tile.x, tile.y), &tile.qoi);

    state.stores_since_sweep += 1;
    evict::maybe_sweep(state);
}

/// Отмечает источник живым: перезаписывает маркер каталога, двигая его mtime.
///
/// Не на каждый тайл: свежесть меряется днями, а маркер — это запись на диск, и
/// платить ею за каждую ячейку значило бы записывать вдвое больше, чем
/// показывать. Но и не раз за сессию: список отмеченных чистится каждым обходом
/// вытеснения (см. `evict::maybe_sweep`), поэтому маркер активного источника
/// заведомо не старше одного окна обхода. Иначе в долгой сессии он остался бы
/// от её начала, и жертвой стал бы ровно тот источник, на который сейчас
/// смотрят.
pub fn touch(state: &mut State, key: &str) {
    if state.touched.insert(key.to_string()) {
        write_file(state, layout::meta_path(key), layout::META_BODY);
    }
}

/// Кладёт байты в файл через fs: CPU-регион → on_write → освобождение по
/// ответу (см. PendingWrite).
pub fn write_file(state: &mut State, path: String, data: &[u8]) {
    let Some(region_id) = veldsdk::abi::resource_alloc_cpu(data.len() as u64) else {
        veldsdk::log::warn!(target: "handlers", "{} не записан: нет памяти под регион", path);
        return;
    };
    // Во владельца сразу после выделения: сорвись запись, регион освободит
    // Drop, а голый id остался бы висеть на хосте до конца процесса.
    let region = veldsdk::OwnedResource::from_raw_id(region_id);
    if let Err(e) = veldsdk::abi::resource_write(region.id(), 0, data) {
        veldsdk::log::warn!(target: "handlers", "{} не записан: {}", path, e);
        return;
    }

    // Гранта на "fs" не нужно: on_write проверяет доступ по requestor_id —
    // паблишеру события, то есть нам же, а мы и так владелец региона.
    let size = data.len() as u64;
    let correlation =
        state.pending_writes.begin(PendingWrite { region: region.id(), path: path.clone() });
    crate::calls::fs::on_write(&FsWriteRequest {
        path,
        // Владение уезжает в таблицу ожидания: освободит его on_write_result.
        handle: Some(ResourceHandle { id: region.into_handle().id, size }),
    }, &correlation);
}

pub fn on_write_result(state: &mut State, result: FsWriteResult) {
    let Some(write) = state.pending_writes.take(&veldsdk::correlation()) else { return };
    drop(veldsdk::OwnedResource::from_raw_id(write.region));
    if !result.error.is_empty() {
        // Потерянная запись — будущий промах: producer перезапишет.
        veldsdk::log::warn!(target: "handlers", "{} не записан: {}", write.path, result.error);
    }
}
