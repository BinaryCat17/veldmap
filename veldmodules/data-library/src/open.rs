//! Открытие скачанного файла ресурсом для заказчика.
//!
//! Библиотека открывает только то, что у неё есть. Продукт, которого в
//! каталоге нет, открывает провайдер (data-provider/on_open): подписать
//! запрос к хранилищу умеет только он, а библиотека про подписи не знает
//! ровно так же, как провайдер не знает про раскладку диска.

use crate::module::State;
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
    // Внешний id вернём в ответе; собственный нужен, чтобы отличить своё
    // чтение от чтения сидкаров (топик fs/on_read_result общий).
    let correlation_id = state.pending_opens.begin(OpenFor {
        owner,
        reply_to: req.correlation_id,
    });
    crate::calls::fs::on_read(&FsReadRequest { path, correlation_id });
}

/// Кому уйдёт открытый ресурс и на какой запрос он отвечает.
pub struct OpenFor {
    pub owner: String,
    pub reply_to: String,
}

/// fs открыл файл. `false` — ответ не наш (значит, это сидкар).
pub fn on_file_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    let Some(target) = state.pending_opens.take(&opened.correlation_id) else { return false };

    if !opened.error.is_empty() {
        emit_error(target.reply_to, &opened.error);
        return true;
    }
    let Some(handle) = opened.handle.clone() else {
        emit_error(target.reply_to, "fs вернул пустой handle");
        return true;
    };
    // Владение — заказчику: дальше он читает ресурс как хочет и сам решает,
    // когда закрыть.
    if !veldsdk::abi::arena_transfer(handle.id, &target.owner) {
        veldsdk::abi::arena_free(handle.id);
        emit_error(target.reply_to, &format!("не удалось передать ресурс сервису '{}'", target.owner));
        return true;
    }

    crate::emit::on_open_result(&ResourceOpened {
        handle: Some(handle),
        error: String::new(),
        correlation_id: target.reply_to,
    });
    true
}

fn emit_error(correlation_id: String, error: &str) {
    crate::emit::on_open_result(&ResourceOpened {
        handle: None,
        error: error.to_string(),
        correlation_id,
    });
}
