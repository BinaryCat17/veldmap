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

/// Сколько закачек идёт разом. Остальные ждут места и трогаются, как только
/// оно освободится.
///
/// Потолок нужен не нам, а той стороне: пакетное «скачать» на странице из
/// двадцати снимков — это сотни файлов за одно нажатие, и хранилище отвечает
/// на такое отказами, а не данными. Десять — с запасом ниже того, на чём
/// хранилище начинает отказывать; точнее эта величина не мерена, и подпирать
/// её числом было бы выдумкой.
///
/// Ждущая запись видна списком наравне с идущими — с тем, что у неё уже лежит
/// на диске, или с нулём у перекачки: с нажатия
/// «скачать» она уже намерение пользователя, и прятать её до старта значило бы
/// терять нажатие из виду. Тем же самым выглядит и окно ожидания подписи —
/// оно и есть та же пауза, только короче (см. [`Download::waiting`]).
const AT_ONCE: usize = 10;

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

    // Засеваем тем, что уже известно, а не нулями: у недокачанной записи
    // закачка продолжится с лежащих на диске байт, и «0 B» на возобновлении
    // было бы враньём. У доведённой `.part` нет вовсе — перекачка идёт с нуля,
    // и отбор по `is_partial` это и говорит.
    let done = state.entry_for(&name).filter(|file| file.is_partial).map(|file| file.size).unwrap_or(0);
    let total = state.total_bytes(&name);

    // Операцию именуем мы: этим же id мы спросим подпись, попросим закачку и
    // отменим задачу у платформы — она наша от начала до конца.
    let correlation_id = veldsdk::generate_id();
    let queued = state.queue_tick;
    state.queue_tick += 1;
    // Место занято — запись встаёт в очередь, а не в сеть. Тронется она сама,
    // когда какая-нибудь из идущих кончится (см. [`start_waiting`]).
    let waiting = busy(state) >= AT_ONCE;
    state.downloads.insert(correlation_id.clone(), Download {
        identifier: req.identifier.clone(),
        name,
        done,
        total,
        delete_when_done: false,
        queued,
        waiting,
        loading: false,
    });

    if !waiting {
        start(state, &correlation_id);
    }
    catalog::publish(state);
}

/// Пустить запись в ход: снять ожидание и спросить подпись.
///
/// Единственное место, где закачка трогается с места, — и потому «ждёт» и «не
/// подписана» здесь одно и то же, а не два согласованных факта. Второй пуск в
/// обход этой функции развёл бы их молча: запись считалась бы ждущей, уже
/// качаясь, и очередь пустила бы её второй раз под тем же id.
///
/// Перекачка доведённого файла сносит его здесь, а не на нажатии. Качальщик
/// пишет в `.part` и переименовывает его только в конце, поэтому иначе рядом с
/// готовым файлом лёг бы второй, недокачанный, под тем же именем записи — а
/// «одно имя, одна запись» держится ровно тем, что такой пары на диске не
/// бывает. Но между нажатием и стартом стои́т очередь, и снесённый заранее файл
/// оставил бы человека без данных и без закачки на всё время ожидания.
///
/// Место держится до терминального события, и у не ответившей подписи его не
/// бывает: убить ход к провайдеру нечем, и такая запись стои́т в очереди, пока
/// её не снимут паузой. Это живое ограничение — своего срока у подписи нет.
fn start(state: &mut State, id: &str) {
    let Some(dl) = state.downloads.get_mut(id) else { return };
    dl.waiting = false;
    let (identifier, name) = (dl.identifier.clone(), dl.name.clone());

    if state.entry_for(&name).is_some_and(|file| !file.is_partial) {
        delete_data(state, &name);
    }
    crate::calls::data_provider::on_sign(&SignRequest { identifier }, id);
}

/// Сколько закачек уже пущено: ждущие места в счёт не идут, всё остальное идёт
/// — включая ждущие подписи, у них ход уже начат.
fn busy(state: &State) -> usize {
    state.downloads.values().filter(|dl| !dl.waiting).count()
}

/// Пустить следующую по очереди, если освободилось место.
///
/// По одной на освободившееся место, а не «сколько влезет»: зовут это с каждого
/// конца закачки, и мест ровно столько, сколько концов.
fn start_waiting(state: &mut State) {
    if busy(state) >= AT_ONCE {
        return;
    }
    let next = state
        .downloads
        .iter()
        .filter(|(_, dl)| dl.waiting)
        .min_by_key(|(_, dl)| dl.queued)
        .map(|(id, _)| id.clone());
    let Some(correlation_id) = next else { return };
    start(state, &correlation_id);
}

/// Адрес подписан — качаем. Путь на диске подставляется здесь: провайдер
/// раскладки хранения не знает и знать не должен.
pub fn on_signed(state: &mut State, signed: SignedUrl) {
    let correlation_id = veldsdk::correlation();
    let Some(dl) = state.downloads.get(&correlation_id) else { return };
    let name = dl.name.clone();

    // Пустой адрес при пустой причине — не удача: так выглядит ответ, который
    // договорил за исполнителя хост, не дождавшись его самого (см. README про
    // терминальный ответ). Приняв его за подпись, мы послали бы закачку в
    // пустоту и сказали бы человеку про сеть там, где дело в упавшем модуле.
    let refusal = match (signed.error.is_empty(), signed.url.is_empty()) {
        (false, _) => Some(signed.error.clone()),
        (true, true) => Some("провайдер не ответил".to_string()),
        (true, false) => None,
    };
    if let Some(why) = refusal {
        veldsdk::log::warn!(target: "handlers", "подпись для {} не удалась: {}", name, why);
        state.troubles.insert(name, format!("подпись не удалась: {}", why));
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
    // С этой минуты конец закачки объявляет платформа, а не мы.
    if let Some(dl) = state.downloads.get_mut(&correlation_id) {
        dl.loading = true;
    }
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
    let Some((task_id, dl)) = state.active_download(&req.name) else { return };
    let (task_id, loading) = (task_id.to_string(), dl.loading);
    crate::cancel::network::on_fs_download(&task_id);
    // Снимаем запись сами только до того, как запрос ушёл в сеть: задачи у
    // платформы ещё нет, терминального события не будет, а пришедшая подпись
    // запустила бы «поставленную на паузу» закачку.
    //
    // После — не снимаем никогда, даже когда убивать оказалось нечего: значит
    // закачка кончилась сама и её ответ уже в пути. Сняв запись здесь, мы
    // отняли бы у него имя записи, и причина срыва пропала бы вместе с ним.
    if !loading {
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
        let mut loading = false;
        if let Some(dl) = state.downloads.get_mut(&task_id) {
            dl.delete_when_done = true;
            loading = dl.loading;
        }
        crate::cancel::network::on_fs_download(&task_id);
        // До сети конец объявляем мы сами (см. on_pause); отложенное удаление
        // срабатывает здесь же — finish читает флаг.
        if !loading {
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

    // Место освободилось — пускаем следующую. До разбора того, чем кончилась
    // эта: очередь не должна ждать ни перечитывания каталога, ни удаления.
    start_waiting(state);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::Config;

    fn state() -> State {
        crate::module::hook_init(Config {}).expect("состояние собирается без диска")
    }

    fn ask(state: &mut State, at: usize) {
        on_download(state, DownloadRequest {
            identifier: format!("eodata/store/S{:02}.tif", at),
            product: String::new(),
        });
    }

    fn waiting(state: &State) -> usize {
        state.downloads.values().filter(|dl| dl.waiting).count()
    }

    /// Разом пущено не больше потолка, а остальное ждёт места.
    ///
    /// Считаем по записям, а не по ходам к провайдеру: наружу ждущая запись
    /// выглядит так же, как ждущая подписи, и отличает их только этот флаг.
    #[test]
    fn no_more_than_the_cap_goes_out_at_once() {
        let mut state = state();
        for at in 0..AT_ONCE + 3 {
            ask(&mut state, at);
        }

        assert_eq!(state.downloads.len(), AT_ONCE + 3, "все нажатия учтены");
        assert_eq!(busy(&state), AT_ONCE, "пущено ровно по потолку");
        assert_eq!(waiting(&state), 3, "остальные ждут места");
    }

    /// Ключ ждущей записи, вставшей в очередь раньше прочих.
    fn first_waiting(state: &State) -> String {
        state
            .downloads
            .iter()
            .filter(|(_, dl)| dl.waiting)
            .min_by_key(|(_, dl)| dl.queued)
            .map(|(id, _)| id.clone())
            .expect("ждущая нашлась")
    }

    /// Ключ любой идущей записи.
    fn any_going(state: &State) -> String {
        state
            .downloads
            .iter()
            .find(|(_, dl)| !dl.waiting)
            .map(|(id, _)| id.clone())
            .expect("идущая нашлась")
    }

    /// Освободившиеся места занимают те, которые нажали раньше, — и в том же
    /// порядке.
    ///
    /// Проверяется пятью подряд, а не одной: реестр закачек — хеш-таблица, и с
    /// одной ждущей потерянный порядок проходил бы такой тест в половине
    /// прогонов, то есть выглядел бы флаки-тестом, а не находкой.
    #[test]
    fn the_freed_places_go_in_the_order_they_were_asked() {
        let mut state = state();
        for at in 0..AT_ONCE + 5 {
            ask(&mut state, at);
        }

        let mut order: Vec<u64> = Vec::new();
        for _ in 0..5 {
            let next = first_waiting(&state);
            let queued = state.downloads[&next].queued;
            let going = any_going(&state);
            finish(&mut state, &going);
            assert!(!state.downloads[&next].waiting, "тронулась не та, которую ждали");
            order.push(queued);
            assert_eq!(busy(&state), AT_ONCE, "место занято ровно одной");
        }

        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(order, sorted, "очередь пошла не по порядку нажатий");
        assert_eq!(waiting(&state), 0, "ждать больше некому");
    }

    /// Удалённая во время закачки запись место освобождает наравне с прочими.
    ///
    /// Её конец уходит по своей ветке — с `return`, — и очередь легко оставить
    /// за ней: десять нажатых корзин по десяти идущим остановили бы её
    /// навсегда, до перезапуска приложения.
    #[test]
    fn a_deleted_download_frees_its_place_too() {
        let mut state = state();
        for at in 0..AT_ONCE + 1 {
            ask(&mut state, at);
        }
        let waited = first_waiting(&state);

        let going = any_going(&state);
        state.downloads.get_mut(&going).expect("запись жива").delete_when_done = true;
        finish(&mut state, &going);

        assert!(!state.downloads[&waited].waiting, "очередь встала за удалённой");
        assert_eq!(busy(&state), AT_ONCE);
    }

    /// Пока потолок не выбран, ждать нечего: первое же нажатие уходит сразу.
    #[test]
    fn the_first_press_does_not_wait() {
        let mut state = state();
        ask(&mut state, 0);

        assert_eq!(waiting(&state), 0);
        assert_eq!(busy(&state), 1);
    }
}
