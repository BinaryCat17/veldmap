//! Открытие скачанного файла ресурсом для заказчика.
//!
//! Библиотека открывает только то, что у неё есть. Продукт, которого в
//! каталоге нет, открывает провайдер (data-provider/on_open): подписать
//! запрос к хранилищу умеет только он, а библиотека про подписи не знает
//! ровно так же, как провайдер не знает про раскладку диска.

use crate::module::{ReadPurpose, State};
use crate::proto::data_library::OpenRequest;
use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::FsReadRequest;

pub fn on_open(state: &mut State, req: OpenRequest) {
    let owner = veldsdk::abi::event_publisher();
    if owner.is_empty() {
        emit_error(req.correlation_id, "on_open пришёл от хоста: ресурс передать некому");
        return;
    }
    let Some(entry) = state.entry_for(&req.name) else {
        emit_error(req.correlation_id, &format!("в каталоге нет записи '{}'", req.name));
        return;
    };
    if entry.is_partial {
        emit_error(req.correlation_id, &format!("'{}' скачан не полностью", req.name));
        return;
    }

    let path = entry.path.clone();
    // Внешний id вернём в ответе; собственный — это ключ ожидания, по нему
    // же ответ и опознаётся как «открытие файла», а не чтение сидкара.
    let correlation_id = state.pending_reads.begin(ReadPurpose::File(OpenFor {
        owner,
        reply_to: req.correlation_id,
    }));
    crate::calls::fs::on_read(&FsReadRequest { path, correlation_id });
}

/// Кому уйдёт открытый ресурс и на какой запрос он отвечает.
pub struct OpenFor {
    pub owner: String,
    pub reply_to: String,
}

/// fs открыл файл, который мы просили для заказчика (`target` — снятое с
/// учёта ожидание, см. module::on_read_result).
pub fn on_file_opened(target: OpenFor, opened: &ResourceOpened) {
    if !opened.error.is_empty() {
        emit_error(target.reply_to, &opened.error);
        return;
    }
    let Some(handle) = opened.handle.clone() else {
        emit_error(target.reply_to, "fs вернул пустой handle");
        return;
    };
    // Владение — заказчику: дальше он читает ресурс как хочет и сам решает,
    // когда закрыть.
    if !veldsdk::abi::arena_transfer(handle.id, &target.owner) {
        veldsdk::abi::arena_free(handle.id);
        emit_error(target.reply_to, &format!("не удалось передать ресурс сервису '{}'", target.owner));
        return;
    }

    crate::emit::on_open_result(&ResourceOpened {
        handle: Some(handle),
        error: String::new(),
        correlation_id: target.reply_to,
    });
}

fn emit_error(correlation_id: String, error: &str) {
    crate::emit::on_open_result(&ResourceOpened {
        handle: None,
        error: error.to_string(),
        correlation_id,
    });
}
