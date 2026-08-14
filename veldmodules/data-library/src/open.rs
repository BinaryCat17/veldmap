//! Открытие скачанного файла ресурсом для заказчика.
//!
//! Библиотека открывает только то, что у неё есть. Продукт, которого в
//! каталоге нет, открывает провайдер (data-provider/on_open): подписать
//! запрос к хранилищу умеет только он, а библиотека про подписи не знает
//! ровно так же, как провайдер не знает про раскладку диска.

use crate::module::{ReadPurpose, State};
use crate::proto::data_library::{ItemRequest, OpenRequest};
use veldsdk::proto::core::ResourceOpened;
use veldsdk::proto::fs::FsReadRequest;
use veldsdk::resource;

pub fn on_open(state: &mut State, req: OpenRequest) {
    let reply_to = veldsdk::correlation();
    let owner = match resource::requester("data-library/on_open") {
        Ok(owner) => owner,
        Err(e) => return fail(reply_to, e),
    };
    let Some(entry) = state.entry_for(&req.name) else {
        return fail(reply_to, format!("в каталоге нет записи '{}'", req.name));
    };
    if entry.is_partial {
        return fail(reply_to, format!("'{}' скачан не полностью", req.name));
    }

    // Недокачанное отсечено выше, значит файл лежит под своим именем.
    let path = crate::module::storage::file_path(&req.name);
    // Внешний id вернём в ответе; собственный — это ключ ожидания, по нему
    // же ответ и опознаётся как «открытие файла», а не чтение сидкара.
    let correlation_id = state.pending_reads.begin(ReadPurpose::File(OpenFor {
        owner,
        reply_to,
    }));
    crate::calls::fs::on_read(&FsReadRequest { path }, &correlation_id);
}

/// Кому уйдёт открытый ресурс и на какой запрос он отвечает.
pub struct OpenFor {
    pub owner: String,
    pub reply_to: String,
}

/// fs открыл файл, который мы просили для заказчика (`target` — снятое с
/// учёта ожидание, см. module::on_read_result). Владение уходит заказчику:
/// дальше он читает ресурс как хочет и сам решает, когда закрыть.
pub fn on_file_opened(target: OpenFor, opened: &ResourceOpened) {
    crate::emit::on_open_result(&resource::relay(opened, &target.owner), &target.reply_to);
}

fn fail(correlation_id: String, error: String) {
    crate::emit::on_open_result(&resource::opened(Err(error)), &correlation_id);
}

/// Показать запись в файловом менеджере. Путь считаем мы: раскладка хранения
/// наша и наружу не выходит, — а показывает платформа, потому что рабочий стол
/// есть только у неё.
///
/// Недокачанную показываем под её `.part`: это то, что на диске лежит на самом
/// деле, и открывать вместо неё пустое место значило бы соврать.
pub fn on_reveal(state: &mut State, req: ItemRequest) {
    if req.name.is_empty() { return; }
    let is_partial = state.entry_for(&req.name).is_some_and(|file| file.is_partial);
    crate::calls::app::on_reveal_path(&veldsdk::proto::app::RevealPath {
        path: crate::module::storage::data_path(&req.name, is_partial),
    });
}
