//! Действия над библиотекой: скачать/пауза, удалить — и приём её состояния.
//!
//! Здесь нет ни путей, ни сидкаров, ни знания о том, как на диске отличается
//! недокачанное: всё это делает data-library. Модуль только переводит нажатие
//! в запрос к ней, а состояние получает рассылкой.

use crate::proto::data_library::{
    DownloadRequest, ItemRequest, LibraryEntry, LibraryState as LibraryStateMsg, SnapshotFiles,
};
use crate::proto::data_provider::{ListPathRequest, ListPathResponse};

use crate::module::state::listing::Chosen;
use crate::module::state::{Batch, Listing, State, ViewId, ViewKind};
use crate::module::state::library::status_of;
use crate::proto::data_library::LibraryStatus;

/// Пользователь нажал «скачать». Повторное нажатие на идущую закачку —
/// пауза: скачанное сохраняется, следующее нажатие продолжит с него.
pub fn on_download_pressed(state: &mut State, identifier: String, product: String) {
    if identifier.is_empty() { return; }

    let downloading = state.library.by_identifier(&identifier)
        .filter(|e| status_of(e) == LibraryStatus::LibDownloading)
        .map(|e| e.name.clone());

    if let Some(name) = downloading {
        crate::calls::data_library::on_pause(&ItemRequest { name });
        return;
    }

    crate::calls::data_library::on_download(&DownloadRequest { identifier, product });
}

/// Пользователь нажал «удалить» — на любой записи библиотеки (полной,
/// недокачанной или заявленной одним лишь намерением). Идущую закачку это
/// заодно отменяет; отдельного «отменить» нет, потому что оставить после себя
/// он обязан то же самое — ничего (см. меню строки в components::table).
pub fn on_delete_pressed(_state: &mut State, name: String) {
    if name.is_empty() { return; }

    crate::calls::data_library::on_delete(&ItemRequest { name });
}

/// Показать запись в файловом менеджере. Путь считает библиотека: раскладка
/// хранения её, и знать её нам незачем.
pub fn on_reveal_pressed(_state: &mut State, name: String) {
    if name.is_empty() { return; }

    crate::calls::data_library::on_reveal(&ItemRequest { name });
}

/// Скачать снимок целиком. Библиотека качает по файлу — снимка она не знает
/// вовсе, — поэтому сперва спрашиваем провайдера, из чего снимок состоит.
///
/// Рекурсивным листингом, а не обходом по ярусам: файлы .SAFE разложены в
/// четыре-пять уровней, и обход стоил бы запроса на каждый.
pub fn on_download_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    let path = crate::module::components::folder_path(&product);
    let correlation_id = state.listings.begin(Listing::Snapshot { product, files: 0, queued: 0 });
    crate::calls::data_provider::on_list_path(
        &ListPathRequest { path, token: String::new(), recursive: true },
        &correlation_id,
    );
}

/// Снимок и то, что о нём насчитано страницами: обход идёт по одной, а оба
/// числа — итоговые.
pub struct Counted {
    pub product: String,
    /// Сколько файлов снимка уже перечислено.
    pub files: u32,
    /// Сколько из них поставлено в закачку.
    pub queued: usize,
}

/// Файлы снимка приехали — ставим в закачку то, чего на диске ещё нет, под тем
/// снимком, из которого их позвали.
///
/// Папок в рекурсивном листинге не бывает (см. `ListPathRequest.recursive`),
/// поэтому отсеивать по роду нечего: всё, что пришло, — файлы.
pub fn on_snapshot_files(
    state: &mut State,
    counted: Counted,
    correlation_id: String,
    response: ListPathResponse,
) {
    let Counted { product, mut files, mut queued } = counted;
    if !response.error.is_empty() {
        state.notice = Some(format!("Снимок «{}» не перечислился: {}", product, response.error));
        return;
    }

    files += response.entries.len() as u32;
    for entry in response.entries {
        // Уже доведённое не трогаем. Перекачка сносит готовый файл до старта
        // (см. data-library::download), поэтому «докачать снимок» на снимке,
        // где половина уже на диске, стирало бы ровно то, что в нём есть, — и
        // качало бы гигабайты заново. Перекачивают по одному файлу, из его
        // меню, где это и помечено как необратимое.
        let done = state
            .library
            .by_identifier(&entry.key)
            .is_some_and(|record| status_of(record) == LibraryStatus::LibComplete);
        if done {
            continue;
        }
        crate::calls::data_library::on_download(&DownloadRequest {
            identifier: entry.key,
            product: product.clone(),
        });
        queued += 1;
    }

    // Страница не последняя — дочитываем той же корреляцией. Потолок тот же,
    // что у обхода папки: снимок конечен, но раскладка, в которой он не
    // кончается, не должна превращаться в бесконечную закачку. Молча
    // обрывать её нельзя — недокачанный снимок с виду ничем не отличается от
    // целого.
    let short = crate::module::components::last_segment(&product).to_string();
    if response.next_token.is_empty() {
        veldsdk::log::info!(target: "handlers", "снимок '{}': {} файлов, в закачке {}", product, files, queued);
        // Обход дошёл до конца — значит, состав снимка известен целиком, и это
        // единственный момент, когда его можно назвать. Библиотека без этого
        // числа считает полным всякий снимок, у которого доведено всё, что
        // качали, — три файла из двадцати шести читались бы как «на диске».
        crate::calls::data_library::on_snapshot(&SnapshotFiles { product, files });
        state.notice = Some(match (files, queued) {
            // Обход кончился, не встретив ни файла. «Уже на диске» тут — ложь:
            // на диске ничего нет, и качать хранилищу тоже нечего.
            (0, _) => format!("В снимке «{}» нет файлов", short),
            (_, 0) => format!("Снимок «{}» уже на диске", short),
            (_, n) => format!("Снимок «{}»: в закачке {} файлов", short, n),
        });
        return;
    }
    if queued >= super::browse::MAX_ITEMS {
        // Состав снимка здесь не называем: обход оборван, и перечисленное —
        // не весь снимок. Сказать библиотеке насчитанное значило бы объявить
        // полным ровно тот снимок, который мы не дочитали.
        state.notice =
            Some(format!("Снимок «{}»: поставлено {} файлов, остальные пропущены", short, queued));
        return;
    }

    let path = crate::module::components::folder_path(&product);
    state.listings.insert(correlation_id.clone(), Listing::Snapshot { product, files, queued });
    crate::calls::data_provider::on_list_path(
        &ListPathRequest { path, token: response.next_token, recursive: true },
        &correlation_id,
    );
}

/// Приостановить закачку снимка целиком.
///
/// Разворачивается он так же, как при удалении: библиотека снимков не знает, а
/// файлов у одного бывает под три десятка, и жать паузу на каждом — работа,
/// которую приложение обязано сделать само. Приостанавливается только идущее:
/// пауза на доведённом файле означала бы «начать и остановить».
pub fn on_pause_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    let going = |entry: &LibraryEntry| status_of(entry) == LibraryStatus::LibDownloading;
    for name in files_of(state, &product, going) {
        crate::calls::data_library::on_pause(&ItemRequest { name });
    }
}

/// Выбросить снимок целиком. Библиотека про снимки не знает — она ведёт учёт
/// файлам, — поэтому разворачиваем его здесь, в том же месте, где строку
/// снимка и собрали (`downloaded_rows`), и просим удалить каждый её файл.
///
/// Отдельного «удали всё по снимку» в контракте нет намеренно: он потребовал
/// бы от библиотеки знать границу снимка, а знает её провайдер, и второй
/// носитель этого факта разошёлся бы с первым.
pub fn on_delete_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    for name in files_of(state, &product, |_| true) {
        crate::calls::data_library::on_delete(&ItemRequest { name });
    }
}

/// Скачать всё выбранное в списке.
///
/// Не то же, что нажать «скачать» у каждой строки: там это переключатель и
/// второе нажатие ставит закачку на паузу (см. [`on_download_pressed`]), а
/// пакетное действие названо одним словом и делать обязано одно.
pub fn on_download_selected(state: &mut State, view: ViewId) {
    for what in fetches(state, view) {
        match what {
            Fetch::Snapshot(product) => on_download_snapshot(state, product),
            Fetch::File { identifier, product } => {
                crate::calls::data_library::on_download(&DownloadRequest { identifier, product });
            }
        }
    }
}

/// Что уедет закачке за одно пакетное нажатие.
///
/// Снимок и файл разведены родом, а не парой полей: снимка библиотека не знает
/// вовсе — сперва у провайдера спрашивают, из чего он состоит (см.
/// [`on_download_snapshot`]), — а файл уходит закачке прямо.
#[derive(PartialEq, Eq, Debug)]
pub enum Fetch {
    Snapshot(String),
    File { identifier: String, product: String },
}

/// Имеет ли смысл качать эту строку — и чем.
///
/// Файл разбирается точно: доведённый и уже идущий пропускаются, оборванный
/// докачивается. Снимок — только целиком доведённый, потому что чего в нём
/// недостаёт, знает провайдер, а не мы: пока снимок не обойдён, у библиотеки
/// нет и списка его файлов. Отсюда и `files == 0` — необойдённый качать есть
/// зачем, и это самый частый случай: снимок из сетевого каталога.
fn fetch_of(state: &State, key: &str, what: &Chosen) -> Option<Fetch> {
    match what {
        Chosen::Snapshot => {
            let (done, files) = state.library.snapshot(key);
            (files == 0 || done < files as usize).then(|| Fetch::Snapshot(key.to_string()))
        }
        Chosen::File { identifier, product, .. } => {
            if identifier.is_empty() {
                return None;
            }
            let busy = state.library.by_identifier(identifier).is_some_and(|entry| {
                matches!(
                    status_of(entry),
                    LibraryStatus::LibDownloading | LibraryStatus::LibComplete
                )
            });
            (!busy).then(|| Fetch::File {
                identifier: identifier.clone(),
                product: product.clone(),
            })
        }
    }
}

/// Что из выбранного имеет смысл качать. Дальше остаются одни запросы.
fn fetches(state: &State, view: ViewId) -> Vec<Fetch> {
    selection(state, view)
        .iter()
        .filter_map(|(key, what)| fetch_of(state, key, what))
        .collect()
}

/// Имена записей библиотеки, которыми обернётся удаление этой строки.
///
/// Снимок разворачивается в свои файлы (см. [`files_of`]) — библиотека ведёт
/// учёт файлам и о снимках не знает. Файл называет себя сам, но имя
/// спрашивается у библиотеки, а не берётся из выбора: файл могли скачать уже
/// после того, как его выбрали, — тогда запись появилась, а в выборе имени
/// нет. Запомненное в выборе остаётся на случай, когда записи под этим ключом
/// у библиотеки нет вовсе.
fn deletions_of(state: &State, key: &str, what: &Chosen) -> Vec<String> {
    match what {
        Chosen::Snapshot => files_of(state, key, |_| true),
        Chosen::File { identifier, name, .. } => {
            let named = match state.library.by_identifier(identifier) {
                Some(entry) => entry.name.clone(),
                None => name.clone(),
            };
            // Пустое имя до библиотеки не доезжает: удалять нечего, а запрос
            // без имени она разберёт отказом.
            match named.is_empty() {
                true => Vec::new(),
                false => vec![named],
            }
        }
    }
}

/// Выбросить всё выбранное — и с диска, и из очереди закачек.
///
/// Выбор снимается с того, что действительно ушло удалению, и только с
/// файлов: строки за удалённым файлом в «Скачанном» больше нет, и оставшаяся
/// отметка считалась бы в заголовке до конца сеанса. Снимок остаётся выбранным
/// — удалены его файлы с диска, а сам он живёт в каталоге, и контур его на
/// шаре по-прежнему верен.
pub fn on_delete_selected(state: &mut State, view: ViewId) {
    let (names, gone) = deletions(state, view);
    for name in names {
        crate::calls::data_library::on_delete(&ItemRequest { name });
    }

    let Some(listing) = state.listing_mut(view) else { return };
    for key in gone {
        listing.selected.remove(&key);
    }
}

/// Что уйдёт удалению за одно пакетное нажатие: имена записей библиотеки и
/// ключи выбора, которые после этого перестанут на что-либо указывать.
///
/// Двумя списками из одного перебора, а не двумя переборами: разойдись они,
/// выбор снимался бы не с того, что удалили.
fn deletions(state: &State, view: ViewId) -> (Vec<String>, Vec<String>) {
    let mut names: Vec<String> = Vec::new();
    let mut gone: Vec<String> = Vec::new();
    for (key, what) in selection(state, view) {
        let asked = deletions_of(state, &key, &what);
        if what != Chosen::Snapshot && !asked.is_empty() {
            gone.push(key);
        }
        names.extend(asked);
    }
    // Выбрать можно и снимок, и отдельный его файл, а удалить запись дважды
    // значит вторым запросом промахнуться по уже несуществующему.
    names.sort();
    names.dedup();
    (names, gone)
}

/// Есть ли что делать пакетным кнопкам. Тем же разбором, каким потом пойдёт и
/// само действие ([`deletions_of`], [`fetch_of`]): по этим ответам решают,
/// показывать ли кнопку, и второй ответ на вопрос «что будет сделано» однажды
/// разошёлся бы с первым — кнопка обещала бы действие и не совершала его.
///
/// Ответом «да/нет», а не числом: числа нигде не показываются, а спрашивают об
/// этом на каждый кадр разметки — перебор обрывается на первом же выбранном,
/// которому есть что сделать.
pub fn batch(state: &State, view: ViewId) -> Batch {
    let picked = selection(state, view);
    Batch {
        deletable: picked.iter().any(|(key, what)| !deletions_of(state, key, what).is_empty()),
        fetchable: picked.iter().any(|(key, what)| fetch_of(state, key, what).is_some()),
    }
}

/// Выбранное в списке — копией: следом идут запросы к библиотеке, а они трогают
/// то же состояние.
///
/// В порядке ключей, а не множества: порядок множества случаен, и один и тот же
/// выбор давал бы то запросы в одном порядке, то в другом, — а по этому же
/// перебору считаются и числа заголовка.
fn selection(state: &State, view: ViewId) -> Vec<(String, Chosen)> {
    let Some(listing) = state.get(view).and_then(ViewKind::listing) else {
        return Vec::new();
    };
    let mut picked: Vec<(String, Chosen)> =
        listing.selected.iter().map(|(key, what)| (key.clone(), what.clone())).collect();
    picked.sort_by(|left, right| left.0.cmp(&right.0));
    picked
}

/// Имена записей снимка — то, чем библиотека адресует его файлы. Правило «что
/// считать файлом этого снимка» написано здесь одно на всех: разворачивают
/// снимок и пауза, и удаление, а два обхода её записей однажды разошлись бы —
/// и пауза оставила бы качаться то, что удаление стёрло.
///
/// Именами, а не ссылками на записи: следом за обходом идут запросы к
/// библиотеке, и держать её состояние занятым до конца перебора незачем.
fn files_of(state: &State, product: &str, wanted: impl Fn(&LibraryEntry) -> bool) -> Vec<String> {
    state
        .library
        .entries
        .iter()
        .filter(|entry| entry.product == product && wanted(*entry))
        .map(|entry| entry.name.clone())
        .collect()
}

/// Библиотека прислала состояние — своё или в ответ на наш запрос. Второго
/// источника правды о скачанном у нас нет, поэтому просто заменяем.
///
/// Отказ показывается до следующего удачного состояния и им же снимается:
/// сообщение, которое некому погасить, висит на экране до перезапуска и врёт
/// тем дольше, чем дольше работает приложение.
pub fn on_state(state: &mut State, msg: LibraryStateMsg) {
    if !msg.error.is_empty() {
        state.error = Some(format!("Library: {}", msg.error));
        return;
    }
    state.error = None;
    state.library.entries = msg.entries;
    measure_speed(state);
}

/// Скорость закачки — из двух соседних состояний: сколько байт прибавилось за
/// сколько секунд. Своего поля под неё у библиотеки нет, и быть не должно —
/// она рассылает то, что есть сейчас, а не то, как быстро оно росло.
///
/// Замер сбрасывается, когда качать нечего: скорость закончившейся закачки —
/// это уже не скорость, а последнее увиденное число.
fn measure_speed(state: &mut State) {
    let (count, done, _) = state.library.downloading();
    if count == 0 {
        state.speed = 0.0;
        state.measured = None;
        return;
    }

    let now = crate::module::components::format::now();

    if let Some((measured_at, measured_done)) = state.measured {
        let seconds = now - measured_at;
        // Того же мгновения мало: разделив на ноль секунд, получим не скорость,
        // а бесконечность. Прошлый замер при этом сохраняется — следующее
        // состояние сравнится уже с ним.
        if seconds > 0 {
            state.speed = done.saturating_sub(measured_done) as f32 / seconds as f32;
            state.measured = Some((now, done));
        }
        return;
    }
    state.measured = Some((now, done));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::state::{BrowseState, ViewKind};

    fn state() -> State {
        State::new(crate::module::handlers::Config { initial_view: None }).expect("состояние")
    }

    fn entry(name: &str, product: &str, status: LibraryStatus) -> LibraryEntry {
        LibraryEntry {
            name: name.to_string(),
            identifier: format!("eodata/{}/{}", product, name),
            product: product.to_string(),
            done: 10,
            total: 10,
            status: status as i32,
            modified: 0,
            siblings: 0,
            trouble: String::new(),
        }
    }

    /// Список с выбором: вид, его строки и то, что в нём отмечено.
    fn chose(picked: Vec<(&str, Chosen)>, entries: Vec<LibraryEntry>) -> (State, ViewId) {
        let mut state = state();
        state.library.entries = entries;
        let pane = state.focused();
        let view = state.open_in(pane, ViewKind::Browse(BrowseState::default()));
        let listing = state.listing_mut(view).expect("список");
        for (key, what) in picked {
            listing.selected.insert(key.to_string(), what);
        }
        (state, view)
    }

    fn file(identifier: &str, name: &str) -> Chosen {
        Chosen::File {
            identifier: identifier.to_string(),
            product: String::new(),
            name: name.to_string(),
        }
    }

    /// Выбранный снимок разворачивается в свои файлы — и только в свои; его же
    /// файл, выбранный отдельно, второго запроса не добавляет.
    ///
    /// Библиотека ведёт учёт файлам и о снимках не знает, поэтому разворот
    /// здесь единственный способ удалить снимок. Второй запрос по тому же
    /// имени промахнулся бы по уже несуществующему, а чужой файл рядом
    /// проверяет, что отбор идёт равенством `product`, а не префиксом.
    #[test]
    fn a_snapshot_unfolds_into_its_own_files_and_only_once() {
        let (state, view) = chose(
            vec![
                ("S2B_X.SAFE", Chosen::Snapshot),
                ("eodata/S2B_X.SAFE/B1.TIF", file("eodata/S2B_X.SAFE/B1.TIF", "B1.TIF")),
            ],
            vec![
                entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete),
                entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete),
                entry("C1.TIF", "S2B_Y.SAFE", LibraryStatus::LibComplete),
            ],
        );

        assert_eq!(
            deletions(&state, view).0,
            vec!["B1.TIF".to_string(), "B2.TIF".to_string()],
            "оба файла снимка по разу, чужой не тронут"
        );
    }

    /// Файл, скачанный уже после того, как его выбрали, всё равно удаляется:
    /// имя записи спрашивается у библиотеки, а запомненное в выборе остаётся на
    /// случай, когда записи под этим ключом у неё нет вовсе.
    #[test]
    fn a_file_downloaded_after_it_was_chosen_is_still_deleted() {
        let (state, view) = chose(
            vec![("eodata/lone/dem.tif", file("eodata/lone/dem.tif", ""))],
            vec![entry("dem.tif", "lone", LibraryStatus::LibComplete)],
        );

        assert_eq!(deletions(&state, view).0, vec!["dem.tif".to_string()]);
    }

    /// Выбранного нет на диске вовсе — удалять нечего, и пустое имя до
    /// библиотеки не доезжает: запрос без имени она разобрала бы отказом.
    #[test]
    fn nothing_on_disk_means_nothing_to_delete() {
        let (state, view) = chose(
            vec![("eodata/lone/dem.tif", file("eodata/lone/dem.tif", ""))],
            Vec::new(),
        );

        assert!(deletions(&state, view).0.is_empty());
    }

    /// Доведённое пакетное «скачать» обходит: у строки то же нажатие — это
    /// переключатель и остановило бы идущее, а пакетное действие названо одним
    /// словом и делает одно.
    #[test]
    fn what_is_already_here_is_not_fetched_again() {
        let (state, view) = chose(
            vec![
                ("eodata/lone/done.tif", file("eodata/lone/done.tif", "done.tif")),
                ("eodata/lone/going.tif", file("eodata/lone/going.tif", "going.tif")),
                ("eodata/lone/new.tif", file("eodata/lone/new.tif", "")),
            ],
            vec![
                entry("done.tif", "lone", LibraryStatus::LibComplete),
                entry("going.tif", "lone", LibraryStatus::LibDownloading),
            ],
        );

        assert_eq!(
            fetches(&state, view),
            vec![Fetch::File {
                identifier: "eodata/lone/new.tif".to_string(),
                product: String::new(),
            }]
        );
    }

    /// Снимок, обойдённый и доведённый целиком, качать незачем; обойдённый
    /// наполовину — есть зачем, а чего в нём недостаёт, скажет провайдер.
    #[test]
    fn a_whole_snapshot_is_not_fetched_again() {
        let whole = |files: u32, done: LibraryStatus| {
            let mut one = entry("B1.TIF", "S2B_X.SAFE", done);
            one.siblings = files;
            let mut two = entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete);
            two.siblings = files;
            vec![one, two]
        };

        let (state, view) =
            chose(vec![("S2B_X.SAFE", Chosen::Snapshot)], whole(2, LibraryStatus::LibComplete));
        assert!(fetches(&state, view).is_empty(), "снимок уже целиком на диске");

        let (state, view) =
            chose(vec![("S2B_X.SAFE", Chosen::Snapshot)], whole(3, LibraryStatus::LibComplete));
        assert_eq!(
            fetches(&state, view),
            vec![Fetch::Snapshot("S2B_X.SAFE".to_string())],
            "в снимке три файла, а на диске два"
        );
    }

    /// Запись без ключа провайдера удаляется по запомненному имени и в закачку
    /// не ставится: качать её неоткуда, а на диске она есть.
    #[test]
    fn a_file_without_a_provider_key_is_deleted_but_not_fetched() {
        let (state, view) = chose(
            vec![("dem.tif", Chosen::File {
                identifier: String::new(),
                product: String::new(),
                name: "dem.tif".to_string(),
            })],
            Vec::new(),
        );

        assert_eq!(deletions(&state, view).0, vec!["dem.tif".to_string()]);
        assert!(fetches(&state, view).is_empty(), "закачка без ключа никуда не поедет");
    }

    /// Оборванная закачка пакетом продолжается: остановленное — это то, что
    /// человек и хотел бы дотянуть, а `.part` докачивается с места обрыва.
    #[test]
    fn an_interrupted_file_is_picked_up_again() {
        let (state, view) = chose(
            vec![("eodata/lone/half.tif", file("eodata/lone/half.tif", "half.tif"))],
            vec![entry("half.tif", "lone", LibraryStatus::LibPaused)],
        );

        assert_eq!(
            fetches(&state, view),
            vec![Fetch::File {
                identifier: "eodata/lone/half.tif".to_string(),
                product: String::new(),
            }]
        );
    }

    /// Снимок, который ни разу не обходили, качать есть зачем — и это самый
    /// частый случай: снимок из сетевого каталога, где на диске нет ничего.
    /// Из чего он состоит, скажет провайдер.
    #[test]
    fn a_snapshot_never_walked_is_worth_fetching() {
        let (state, view) = chose(vec![("S2B_X.SAFE", Chosen::Snapshot)], Vec::new());

        assert_eq!(fetches(&state, view), vec![Fetch::Snapshot("S2B_X.SAFE".to_string())]);
    }

    /// Кнопка стои́т ровно тогда, когда ей есть что сделать, и отвечает на это
    /// тем же разбором, каким пойдёт действие.
    #[test]
    fn the_buttons_answer_for_what_the_action_will_do() {
        // Скачанный файл: удалять есть что, качать нечего.
        let (state, view) = chose(
            vec![("eodata/lone/done.tif", file("eodata/lone/done.tif", "done.tif"))],
            vec![entry("done.tif", "lone", LibraryStatus::LibComplete)],
        );
        let can = batch(&state, view);
        assert!(can.deletable && !can.fetchable);

        // Снимок из каталога, которого на диске нет: наоборот.
        let (state, view) = chose(vec![("S2B_X.SAFE", Chosen::Snapshot)], Vec::new());
        let can = batch(&state, view);
        assert!(!can.deletable && can.fetchable);
    }

    /// Удалённый файл уходит и из выбора: строки за ним больше нет, и оставшаяся
    /// отметка считалась бы в заголовке до конца сеанса. Снимок остаётся: с
    /// диска ушли его файлы, а сам он живёт в каталоге, и контур его верен.
    #[test]
    fn deleting_drops_the_file_from_the_choice_and_keeps_the_snapshot() {
        let (mut state, view) = chose(
            vec![
                ("S2B_X.SAFE", Chosen::Snapshot),
                ("eodata/lone/dem.tif", file("eodata/lone/dem.tif", "dem.tif")),
            ],
            vec![entry("dem.tif", "lone", LibraryStatus::LibComplete)],
        );

        on_delete_selected(&mut state, view);

        let listing = state.get(view).and_then(ViewKind::listing).expect("список");
        assert!(listing.selected.contains_key("S2B_X.SAFE"), "снимок ушёл из выбора");
        assert!(!listing.selected.contains_key("eodata/lone/dem.tif"), "файл остался в выборе");
    }
}
