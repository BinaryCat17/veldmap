//! Приобретение продукта: подпись у провайдера, закачка, пауза, удаление.
//!
//! У операции здесь одно имя на весь путь: наш `correlation_id` — он же id
//! запроса к провайдеру, он же id запроса к network, он же имя операции у
//! платформы. Владельцем её платформа делает того, кто публикует
//! `network/on_fs_download`, то есть нас, — поэтому убиваем мы её сами.
//!
//! Провайдер участвует ровно в одном шаге — подписывает адрес. Раскладка
//! хранения из библиотеки не выходит: путь подставляем мы, уже после подписи.

use crate::module::{Download, State};
use crate::module::storage;
use crate::module::catalog::{self, delete_data, delete_entry, write_sidecar};
use crate::proto::data_library::{DownloadRequest, ItemRequest};
use crate::proto::data_provider::{SignRequest, SignedUrl};
use veldsdk::proto::network::{FsDownloadProgress, FsDownloadRequest, FsDownloadResponse};

/// Скачать или докачать продукт. Куда он ляжет — решаем мы: раскладка
/// хранения наша, и заказчику её знать незачем.
pub fn on_download(state: &mut State, req: DownloadRequest) {
    if req.identifier.is_empty() { return; }

    // Снимок, если его не назвали, берётся из уже записанного: перекачка того
    // же файла приходит без него, а забыть, к какому снимку файл относится,
    // значило бы вынуть его оттуда навсегда — заново эту границу нам не
    // спросить, её знает провайдер. Ищем по ключу, а не по имени: имя из
    // снимка и выводится, и до него ещё не добрались.
    let product = match req.product.is_empty() {
        true => state
            .origins
            .values()
            .find(|origin| origin.identifier == req.identifier)
            .map(|origin| origin.product.clone())
            .unwrap_or_default(),
        false => req.product.clone(),
    };
    let name = storage::name_from_identifier(&req.identifier, &product);

    // Повторное нажатие, пока идёт предыдущая попытка (в том числе пока она
    // ждёт подписи). Отсекаем здесь: id операции наш с самого начала, поэтому
    // окна, в котором закачка идёт, а учёта её ещё нет, не существует.
    if state.downloads.values().any(|d| d.name == name) {
        veldsdk::log::info!(target: "handlers", "{} уже качается, повтор игнорируем", name);
        return;
    }

    // Причина прошлого срыва — про прошлую попытку, и новая её снимает: пока
    // она идёт, говорить о ней нечего, а сорвётся — скажет своё.
    state.troubles.remove(&name);

    // Сидкар — сразу, до подписи: даже если приложение упадёт на первом байте,
    // на диске уже будет известно, откуда файл, и запись о намерении не
    // пропадёт. Из прежнего сидкара переносим то, что о записи известно, но
    // добыто не этим запросом: ожидаемый размер с прошлой попытки и состав
    // снимка, если его успели обойти.
    let (total_bytes, siblings) = state.origins.get(&name)
        .map_or((None, 0), |origin| (origin.total_bytes, origin.siblings));
    write_sidecar(state, &name, storage::OriginSidecar {
        provider: storage::PROVIDER_NAME.to_string(),
        identifier: req.identifier.clone(),
        total_bytes,
        product: product.clone(),
        siblings,
    });

    // Перекачка доведённого файла сносит его до старта. Качальщик пишет в
    // `.part` и переименовывает его только в конце, поэтому иначе рядом с
    // готовым файлом лёг бы второй, недокачанный, под тем же именем записи —
    // а «одно имя, одна запись» держится ровно тем, что такой пары на диске
    // не бывает. Отсюда и предупреждение в меню: перекачка необратима.
    if state.entry_for(&name).is_some_and(|file| !file.is_partial) {
        delete_data(state, &name);
    }

    // Засеваем тем, что уже известно, а не нулями: у недокачанной записи
    // закачка продолжится с лежащих на диске байт, и «0 B» на возобновлении
    // было бы враньём. У доведённой `.part` нет — она только что снесена, и
    // перекачка идёт с нуля.
    let done = state.entry_for(&name).filter(|file| file.is_partial).map(|file| file.size).unwrap_or(0);
    let total = state.total_bytes(&name);

    // Операцию именуем мы: этим же id мы спросим подпись, попросим закачку и
    // отменим задачу у платформы — она наша от начала до конца.
    let correlation_id = veldsdk::generate_id();
    state.downloads.insert(correlation_id.clone(), Download {
        identifier: req.identifier.clone(),
        name,
        done,
        total,
        delete_when_done: false,
    });

    crate::calls::data_provider::on_sign(&SignRequest {
        identifier: req.identifier,
    }, &correlation_id);
    catalog::publish(state);
}

/// Адрес подписан — качаем. Путь на диске подставляется здесь: провайдер
/// раскладки хранения не знает и знать не должен.
pub fn on_signed(state: &mut State, signed: SignedUrl) {
    let correlation_id = veldsdk::correlation();
    let Some(dl) = state.downloads.get(&correlation_id) else { return };
    let name = dl.name.clone();

    if !signed.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "подпись для {} не удалась: {}", name, signed.error);
        state.troubles.insert(name, format!("подпись не удалась: {}", signed.error));
        // Задачи ещё нет — терминального события платформы не будет, снимаем
        // с учёта сами, иначе запись навсегда останется «качается».
        finish(state, &correlation_id);
        return;
    }

    crate::calls::network::on_fs_download(&FsDownloadRequest {
        url: signed.url,
        path: storage::file_path(&name),
        headers: signed.headers,
    }, &correlation_id);
}

/// Пауза: `.part` остаётся на диске ровно там, где его бросил обрыв, и
/// следующее «скачать» продолжит с оборванного байта. Убиваем сами: операция
/// наша. Прибирать за собой здесь нечего — терминальный
/// `on_fs_download_result` придёт всё равно, его за убитого качальщика
/// опубликует хост.
///
/// Отказаться от начатого — это не сюда, а в `on_delete`: остановка с
/// сохранением и остановка с выбрасыванием — разные вещи, и топика на них
/// два, а не один с двумя смыслами.
pub fn on_pause(state: &mut State, req: ItemRequest) {
    let Some((task_id, _)) = state.active_download(&req.name) else { return };
    let task_id = task_id.to_string();
    // Убивать бывает нечего: закачка ещё ждёт подписи, и задачи у платформы
    // нет. Терминального события тогда не будет, снимаем запись сами — иначе
    // пришедшая подпись запустила бы «поставленную на паузу» закачку.
    if !crate::cancel::network::on_fs_download(&task_id) {
        finish(state, &task_id);
    }
}

/// Удалить запись — полную, недокачанную или заявленную одним лишь сидкаром.
/// Идущую закачку это заодно и отменяет: отказ от начатого выражается тем же
/// топиком, потому что оставить после себя он обязан то же самое — ничего.
pub fn on_delete(state: &mut State, req: ItemRequest) {
    // Файл прямо сейчас качается — host держит `.part` открытым на запись,
    // удалять поверх активной записи нельзя. Убиваем закачку и помечаем запись
    // к удалению: `abort` не дожидается, пока качальщик отпустит файл, поэтому
    // сам delete делается по терминальному ответу — к тому моменту фьючерс
    // уже дропнут вместе с дескриптором.
    if let Some((task_id, _)) = state.active_download(&req.name) {
        let task_id = task_id.to_string();
        if let Some(dl) = state.downloads.get_mut(&task_id) {
            dl.delete_when_done = true;
        }
        // Убивать было нечего — закачка ещё ждёт подписи (см. on_pause).
        // Отложенное удаление срабатывает здесь же: finish читает флаг.
        if !crate::cancel::network::on_fs_download(&task_id) {
            finish(state, &task_id);
        }
        return;
    }
    delete_entry(state, &req.name);
}

/// Broadcast-топик — сверяем correlation_id, чтобы не принять чужой ответ.
pub fn on_delete_result(state: &mut State, response: veldsdk::proto::fs::FsDeleteResult) {
    let Some(path) = state.pending_delete.take(&veldsdk::correlation()) else { return };
    if !response.error.is_empty() {
        veldsdk::log::warn!(target: "handlers", "не удалось удалить {}: {}", path, response.error);
    }
    // Перечитываем в любом случае: при ошибке — чтобы показать то, что на
    // диске на самом деле, при успехе — чтобы запись ушла.
    catalog::rescan(state);
}

pub fn on_fs_download_progress(state: &mut State, event: FsDownloadProgress) {
    let Some(dl) = state.downloads.get_mut(&veldsdk::correlation()) else { return };
    dl.done = event.downloaded_bytes;
    dl.total = event.total_bytes;

    // Content-Length пишем в сидкар, когда узнали новое значение (не на каждом
    // событии): иначе после рестарта «из скольки» снова неизвестно, пока не
    // начнётся новая закачка — сидкар это единственное, что переживает
    // перезапуск.
    if event.total_bytes > 0 {
        let name = dl.name.clone();
        // Правим одно поле поверх записанного сидкара: всё остальное приехало
        // с запросом закачки, а здесь этих фактов взять неоткуда — переписать
        // их пустыми значило бы потерять и происхождение, и снимок.
        let known = state.origins.get(&name).cloned()
            .filter(|origin| origin.total_bytes != Some(event.total_bytes));
        if let Some(known) = known {
            write_sidecar(state, &name, storage::OriginSidecar {
                total_bytes: Some(event.total_bytes),
                ..known
            });
        }
    }
    catalog::publish(state);
}

/// Единственный конец закачки, каким бы он ни был: успех, ошибка сети или
/// убийство.
///
/// Непустая ошибка — это именно срыв: у убитой доменного итога нет, за неё
/// публикует пустой ответ сам хост. Тем и отличается сорвавшаяся закачка от
/// остановленной человеком — больше их не различает ничто.
pub fn on_fs_download_result(state: &mut State, response: FsDownloadResponse) {
    let correlation_id = veldsdk::correlation();
    if !response.error.is_empty() {
        let name = state.downloads.get(&correlation_id).map(|d| d.name.clone());
        if let Some(name) = name {
            veldsdk::log::warn!(target: "handlers", "закачка {} не удалась: {}", name, response.error);
            state.troubles.insert(name, response.error.clone());
        }
    }
    finish(state, &correlation_id);
}

/// Снимает закачку с учёта и приводит каталог в соответствие диску.
///
/// Идемпотентна намеренно: ранний отказ (подпись не удалась) случается до
/// того, как операция вообще заведена, и терминального ответа по нему не
/// будет — этот путь ведёт сюда напрямую.
fn finish(state: &mut State, id: &str) {
    let Some(dl) = state.downloads.remove(id) else { return };

    // Корзину нажимали во время закачки — тогда это не отмена ради отмены,
    // а отложенное удаление: `.part` остался ровно там, где abort его бросил,
    // и теперь его можно безопасно удалить.
    if dl.delete_when_done {
        delete_entry(state, &dl.name);
        return;
    }

    // Перечитываем каталог: размер `.part`/готового файла берётся с диска, а
    // не переносится руками из реестра в запись.
    catalog::rescan(state);
}
