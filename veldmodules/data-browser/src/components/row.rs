//! components/row.rs — строка списка: то, что показывают все три вида.
//!
//! Состояние записи здесь не выводится: его присылает data-library, которая
//! одна и знает, что лежит на диске, что качается и откуда взялось. Осталось
//! сопоставление «запись каталога → то, что рисуем».

use crate::module::state::library::{status_of, LibraryState};
use crate::module::state::overlay::{Pace, Progress};
use crate::proto::data_library::{LibraryEntry, LibraryStatus};

/// Состояние строки. Сумма-тип, а не набор булевых полей: сочетания вроде
/// «недокачан, но байт ноль и задачи нет» перестают быть выразимыми.
#[derive(Clone, PartialEq)]
pub enum RowStatus {
    /// Записи в библиотеке нет — есть только у провайдера.
    Remote,
    Downloading { done: u64, total: u64 },
    /// Начато, но не доведено. `done: 0` — закачка сорвалась до первых байт.
    ///
    /// `trouble` — почему сорвалась последняя попытка. Пусто — причины нет:
    /// закачку остановил человек, либо попытка была до перезапуска (причина
    /// живёт в памяти библиотеки), либо это строка снимка, чьи файлы стоя́т по
    /// разным причинам. Стои́т запись во всех случаях одинаково и продолжается
    /// одинаково, поэтому это не отдельное состояние, а причина при нём:
    /// разница только в том, знает ли человек, почему она стои́т.
    Paused { done: u64, total: u64, trouble: String },
    Complete,
    /// Папка или снимок, часть содержимого которых уже на диске. Сколько там
    /// файлов всего, каталог не говорит, поэтому сказано только скачанное.
    ///
    /// `trouble` — почему стои́т остальное; пусто — не стои́т или причина у
    /// частей разная. Нужно оно здесь по той же причине, что и у оборванной
    /// строки, и даже больше: «3 на диске» — надпись спокойная, зелёная, и
    /// отказ за ней не виден вовсе.
    Partial { done: usize, trouble: String },
}

impl RowStatus {
    /// Сколько сделано из скольки, если это вообще идёт. `None` — полосе
    /// загрузки в этой строке места нет.
    pub fn progress(&self) -> Option<(u64, u64)> {
        match self {
            RowStatus::Downloading { done, total } | RowStatus::Paused { done, total, .. } if *total > 0 => {
                Some((*done, *total))
            }
            _ => None,
        }
    }
}

/// Очерчен ли снимок строки на шаре.
///
/// Своё состояние, а не производное от выбора: контур, показ растром и выбор
/// строки — три независимых вещи, и сведённые в одну они отвечали бы друг за
/// друга. Спрашивают его двое: значок контура в строке (горит ли) и стоящая
/// рядом наводка — навести можно только на то, что на шаре есть.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OnOutline {
    #[default]
    Off,
    /// Очертить попросили, а геометрия ещё едет из каталога: рисовать пока
    /// нечего, но нажатие уже принято.
    Asking,
    /// Попросили, а геометрии у снимка нет вовсе — рисовать нечего и ждать
    /// нечего. Состояние отдельное от [`OnOutline::Asking`], потому что иначе
    /// значок обещал бы контур, который не приедет никогда.
    Blank,
    /// Спросить не вышло — сеть или отказ службы. Про сам снимок это не
    /// говорит ничего, и отделено от [`OnOutline::Blank`] поэтому: там
    /// переспрашивать нечего, а здесь только и остаётся.
    Failed,
    /// Нарисован.
    Drawn,
}

/// Чем снимок строки лежит на шаре.
///
/// Один ответ на все экраны и на все три списка. Знание это принадлежит
/// приложению, а не вкладке — шар один, а списков сколько угодно, — и
/// спрашивают его четверо: значок глобуса в строке (горит ли), полоса хода под
/// строкой, список слоёв и штриховка, которой место снимка помечено на шаре,
/// пока картинки нет (см. `handlers::outline::send`). Четырьмя
/// выражениями по месту они разошлись бы молча: значок горел бы у снятого слоя
/// ровно до следующей пересборки разметки.
///
/// Про контур здесь не сказано ничего: у него своё состояние — [`OnOutline`].
#[derive(Clone, Copy, PartialEq, Default)]
pub enum OnGlobe {
    #[default]
    /// Растром его на шаре нет.
    Off,
    /// Показ попросили, а продукт под этим ключом ещё восстанавливает каталог:
    /// наложения нет вовсе. Отдельно от [`OnGlobe::Assembling`], потому что
    /// наводить в этом положении не на что — ни слоя, ни его рамки ещё не
    /// заведено, — а во всём остальном они говорят одно и то же.
    Asked,
    /// Растры спрошены у провайдера или открываются: на шаре его ещё нет, но
    /// он туда едет.
    Assembling,
    /// Лежит растром. `hidden` — остаётся в наборе, но не рисуется; `progress`
    /// — ход добычи, каким его рассказал глобус.
    ///
    /// Ход целиком, а не одна доля из него: полосу рисует доля, а объясняет её
    /// подпись, и считаться они обязаны из одного и того же — иначе полоса и
    /// подсказка под одним значком говорят разное.
    Laid { hidden: bool, progress: Progress },
}

impl OnGlobe {
    /// Чем рисовать полосу под строкой (см. [`Pace`]). У сборки доли нет
    /// вовсе: растры ещё спрашивают и открывают, и считать в ней нечего — но
    /// сказать, что работа началась, надо именно тогда, потому что это первое,
    /// что должен увидеть нажавший значок.
    pub fn pace(self) -> Pace {
        match self {
            OnGlobe::Off => Pace::Idle,
            OnGlobe::Asked | OnGlobe::Assembling => Pace::Unknown,
            OnGlobe::Laid { progress, .. } => progress.pace(),
        }
    }

    /// Ход по частям, от старшей к младшей: что полоса отсчитывает, сколько
    /// прочитано и что едет прямо сейчас. Пусто — работа кончилась либо о ней
    /// ещё нечего сказать.
    ///
    /// Частями, потому что мест у этой фразы два и они разной ширины: в ячейку
    /// колонки влезают не все, в подсказку — все (см. `Progress::parts`).
    pub fn parts(self) -> Vec<String> {
        match self {
            OnGlobe::Off => Vec::new(),
            OnGlobe::Asked => vec!["спрашиваем каталог…".to_string()],
            OnGlobe::Assembling => vec!["растры открываются…".to_string()],
            OnGlobe::Laid { progress, .. } => progress.parts(),
        }
    }

    /// Тот же ход одной фразой. `None` — сказать нечего.
    pub fn said(self) -> Option<String> {
        let parts = self.parts();
        match parts.is_empty() {
            true => None,
            false => Some(parts.join(" · ")),
        }
    }

    /// Есть ли снимок на шаре хоть в каком-то виде — тем и горит значок, и
    /// тем же занято место под полосу.
    pub fn any(self) -> bool {
        !matches!(self, OnGlobe::Off)
    }

    /// Есть ли куда наводить камеру: заведён слой — у него посчитана рамка.
    /// Одной просьбы мало: пока каталог не ответил, о снимке не известно даже
    /// того, где он.
    pub fn laid(self) -> bool {
        matches!(self, OnGlobe::Assembling | OnGlobe::Laid { .. })
    }

}

/// Чем запись является для приложения. Сумма-тип, а не пара булевых полей:
/// «папка» и «снимок» — не два независимых свойства, а один род. Парой их не
/// выразить: снимок бывает и каталогом, и одним объектом, а «папка, которая не
/// снимок» и «снимок, который не папка» — это разные строки с разными
/// действиями, а не сочетания флагов.
#[derive(Clone, Copy, PartialEq)]
pub enum RowKind {
    File,
    /// Папка пути: внутри снимки или другие папки. Сама по себе не показывает
    /// ничего — в неё только заходят.
    Folder,
    /// Снимок — одна логическая единица. Лежит он развёрнутым каталогом
    /// (.SAFE, .SEN3) или одним объектом, показу безразлично: и то, и другое
    /// декодер собирает в один растр. `folder` говорит лишь о том, есть ли
    /// куда зайти внутрь.
    Product { folder: bool },
}

impl RowKind {
    /// Есть ли внутри содержимое, в которое заходят. У снимка-каталога оно
    /// есть, но заход — не главное его действие: смотрят снимок целиком.
    pub fn is_folder(self) -> bool {
        matches!(self, RowKind::Folder | RowKind::Product { folder: true })
    }

    /// Снимок ли это — то, что кладут на шар и смотрят одной картинкой.
    pub fn is_product(self) -> bool {
        matches!(self, RowKind::Product { .. })
    }
}

/// Голый ключ хранилища: без завершающего слэша, которым листинг помечает
/// папку.
///
/// Слэш — признак строки листинга, а не часть пути: один и тот же продукт
/// приходит из каталога со слэшем, а из выдачи поиска без него. Правило
/// написано здесь одно на всех — от него зависят и папка записи, и путь
/// листинга, и ключ снимка, и имя, которым его называют (см. [`last_segment`]),
/// и сравнение строки с ключом перехода, а пять его копий однажды сочли бы один
/// снимок за два.
pub fn bare(key: &str) -> &str {
    key.trim_end_matches('/')
}

/// Последний сегмент пути — то, чем путь называют человеку: имя файла у ключа
/// объекта, имя снимка у ключа каталога, заголовок у пути вкладки.
///
/// Слэш на конце в счёт не идёт (см. [`bare`]): он помечает строку листинга, а
/// не отделяет за собой пустой сегмент, и `eodata/S2B_X.SAFE/` зовётся тем же
/// именем, что и `eodata/S2B_X.SAFE`, — снимок за ними один. Пусто отвечается
/// только пути без единого сегмента: корню и пустому ключу, — и читается это
/// как «имя брать неоткуда», а не как «имя пустое».
pub fn last_segment(path: &str) -> &str {
    bare(path).rsplit('/').next().unwrap_or("")
}

/// Папка, в которой лежит ключ провайдера. Выводится из самого ключа — своего
/// поля под это нет ни у каталога, ни у библиотеки, — и написана здесь одна на
/// всех: её спрашивают и строка списка, и полоса под глобусом, а две копии
/// правила «где кончается путь» однажды ответят по-разному.
pub fn folder_of(identifier: &str) -> &str {
    let path = bare(identifier);
    match path.rfind('/') {
        Some(cut) => &path[..cut],
        None => "",
    }
}

/// Путь, которым листают содержимое этой записи: ключ папки — всегда со
/// слэшем. Без него листинг по префиксу показал бы саму папку вместо того, что
/// в ней лежит.
pub fn folder_path(identifier: &str) -> String {
    format!("{}/", bare(identifier))
}

/// Чем смотреть снимок, о котором известен один ключ.
///
/// Правило одно на всех, кто предлагает «смотреть» не из строки списка: полоса
/// под шаром и список слоёв. Строка решает это сама — род и запись библиотеки
/// записаны прямо в ней.
///
/// Порядок вопросов — от дешёвого к дорогому. Скачанное смотрят с диска: файл
/// под рукой, и ходить за ним по сети значит ждать того, что уже есть. Снимок,
/// лежащий каталогом, открывается через провайдера — растр внутри выбирает он,
/// потому что `GET` по пути каталога отвечает 404. Остальное открывается прямо.
pub fn preview_of(
    library: &LibraryState,
    identifier: &str,
    folder: bool,
) -> crate::module::ViewMsg {
    use crate::module::ViewMsg;
    if let Some(name) = library.local_name(identifier) {
        return ViewMsg::Preview(name.to_string());
    }
    match folder {
        true => ViewMsg::PreviewProduct(identifier.to_string()),
        false => ViewMsg::PreviewRemote(identifier.to_string()),
    }
}

/// Приставка ключа строки снимка, у которого упаковок несколько
/// (см. [`scene_key`]).
const SCENE: &str = "снимок:";

/// Ключ строки снимка, собранного из нескольких упаковок.
///
/// Путём продукта ему быть нельзя: показываемая упаковка стои́т под снимком
/// собственной строкой, и с одинаковыми ключами раскрывались бы обе разом.
/// Двоеточия в ключах хранилища не бывает, поэтому приставка ни с чем не
/// совпадёт.
pub fn scene_key(identifier: &str) -> String {
    format!("{}{}", SCENE, identifier)
}

/// Ключ строки снимка — не путь в хранилище: листать по нему нечего, упаковки
/// приехали вместе с ответом каталога.
pub fn is_scene_key(key: &str) -> bool {
    key.starts_with(SCENE)
}

pub struct Row {
    /// Ключ провайдера; пустой — неизвестен, тогда докачка и просмотр
    /// удалённого не предлагаются.
    pub identifier: String,
    /// Собственный ключ строки — только у снимка с несколькими упаковками
    /// (см. [`scene_key`]). У всех остальных `None`, и ключом им служит путь.
    pub group: Option<String>,
    /// Имя записи в библиотеке — ключ для удаления и просмотра. Пустое, если
    /// записи ещё нет (файл не скачан).
    pub name: String,
    /// Отображаемое имя.
    pub title: String,
    pub kind: RowKind,
    /// Очерчен ли снимок строки (см. [`OnOutline`]).
    pub outlined: OnOutline,
    /// Размер в байтах; 0 — неизвестен.
    pub size: u64,
    /// Время файла, unix-секунды; 0 — неизвестно.
    pub date: i64,
    /// Чем запись считает каталог: у снимка это тип продукта («S2MSI2A»).
    /// Пусто — каталог не сказал, и вид выводится из рода и имени
    /// (см. [`Row::format`]).
    pub product_type: String,
    /// Снимок, к которому относится запись; пусто — она сама по себе. Едет с
    /// ней до самой закачки: библиотека записывает файл в тот снимок, из
    /// которого его позвали, а вывести это из ключа она не может — раскладку
    /// бакета знает провайдер (см. `ListEntry.product`).
    pub product: String,
    /// Содержимое строки: файлы снимка или записи раскрытой папки.
    ///
    /// У сложенной строки они есть всегда — она из них и сложена, и по ним
    /// считается всё, что о ней известно (см. [`Row::stored`]); у папки
    /// каталога наполняются только раскрытые, потому что до раскрытия их и не
    /// спрашивали. Показывать содержимое или молчать о нём — дело не этого
    /// поля, а раскладки (см. `arrange::expand`).
    pub children: Vec<Row>,
    /// Строка сложена из записей библиотеки, а не является записью. Качать и
    /// открывать нужно её файлы: её ключ — путь снимка в хранилище, и послать
    /// его в закачку значит попросить скачать папку одним объектом.
    ///
    /// Отдельным полем, а не «есть дети»: детей заводит и раскрытая папка
    /// каталога, а она как раз обычная запись, и заход внутрь у неё никто не
    /// отнимает.
    pub folded: bool,
    /// Содержимое раскрытой папки ещё едет. Пустота под ней читается как
    /// «здесь ничего нет», и это неправда до конца листинга.
    pub loading: bool,
    /// Чем снимок этой строки лежит на шаре (см. [`OnGlobe`]). Стоит здесь, а
    /// не в разметке, потому что спрашивают это по ключу снимка, а ключ есть у
    /// строки.
    pub globe: OnGlobe,
    /// Почему «на глобус» не предлагать; пусто — предлагать. Сказал провайдер —
    /// только он знает и уровень обработки, и то, какие форматы открывает
    /// наложение (см. `DataProduct.unviewable`).
    ///
    /// Причиной, а не флагом: значок над тем, что заведомо не покажется, обещает
    /// то, чего не бывает, но убранный молча он оставляет гадать. С причиной он
    /// остаётся на месте выключенным и объясняет себя подсказкой — тем же
    /// способом, каким объясняется наводка на снимок, которого на шаре нет.
    /// В тесной панели объяснения нет: там значки уезжают пунктами меню, а
    /// выключенный не уезжает (см. `table::actions`).
    ///
    /// У строки, о которой провайдер ничего не говорил, пусто: молчание не
    /// повод отнимать действие (так стои́т скачанное — его собрала библиотека),
    /// а отказ, если он есть, объяснится после нажатия.
    pub unviewable: String,
    /// Почему показ этого снимка не вышел в прошлый раз; пусто — не пробовали
    /// либо вышло.
    ///
    /// Отдельно от [`Row::unviewable`], хотя обе строки говорят «не покажется»:
    /// разница в том, кто это знает и что делать дальше. Провайдер знает
    /// заранее и наверняка — значок гаснет, нажимать нечего. А эта причина —
    /// итог настоящей попытки, и попытка могла сорваться по сети: значок
    /// остаётся нажимаемым, но горит предупреждением и носит причину
    /// подсказкой. Сведённые в одно поле, они заставили бы выбирать между
    /// «нельзя вовсе» и «не вышло сейчас» одним видом.
    pub unshowable: String,
    pub status: RowStatus,
}

/// Строка, о которой ничего не известно: ключа нет, содержимого нет, на шаре её
/// нет. Заведено ради `..Default::default()` в сборках строки — полей у неё
/// полтора десятка, и выписанные по разу на сборку они расходятся молча:
/// приписанное поле достаётся не всем.
///
/// Написано руками, а не выведено: два поля отвечают не нулём своего типа. Род
/// умолчания — файл: это единственная запись, которая ничего в себе не
/// содержит; состояние — `Remote`: пока библиотека о записи не сказала, запись
/// лежит в хранилище. Причина отказа при этом пуста, и здесь ноль типа отвечает
/// сам за себя: молчание провайдера не повод отнимать действие
/// (см. [`Row::unviewable`]).
impl Default for Row {
    fn default() -> Row {
        Row {
            identifier: String::new(),
            group: None,
            name: String::new(),
            title: String::new(),
            kind: RowKind::File,
            outlined: OnOutline::Off,
            size: 0,
            date: 0,
            product_type: String::new(),
            product: String::new(),
            children: Vec::new(),
            folded: false,
            loading: false,
            globe: OnGlobe::Off,
            unviewable: String::new(),
            unshowable: String::new(),
            status: RowStatus::Remote,
        }
    }
}

impl Row {
    /// Устойчивое имя строки в списке — им она сопоставляется между кадрами
    /// (см. `Element::key`) и им же адресуется её меню. Ключ провайдера, а без
    /// него имя записи: пустыми оба сразу не бывают, иначе строке неоткуда
    /// взяться.
    pub fn key(&self) -> &str {
        if let Some(group) = &self.group {
            return group;
        }
        if self.identifier.is_empty() { &self.name } else { &self.identifier }
    }

    /// Ключ снимка, каким его знает провайдер (см. [`bare`]). Им снимок
    /// отмечают на шаре, кладут на него и открывают. Пуст у записи, которой
    /// провайдер не знает вовсе, — такую ни очертить, ни показать.
    pub fn snapshot_key(&self) -> &str {
        bare(&self.identifier)
    }

    /// Ключ снимка, которому эта строка принадлежит: собственный у снимка,
    /// корень продукта у файла внутри него.
    ///
    /// Спрашивают его там, где действие про снимок, а нажали на файл:
    /// на шар кладётся снимок, и наложение ложится под тем ключом, которым его
    /// зовёт провайдер (см. `handlers::overlay::on_located`). Позови мы его
    /// ключом файла — снять положенное этим же пунктом было бы уже нечем: под
    /// ключом файла на шаре нет ничего.
    ///
    /// Корень знает провайдер и присылает вместе с записью (`ListEntry.product`);
    /// вывести его из ключа мы не можем — раскладка бакета не наша. Пусто —
    /// провайдер промолчал, и тогда снимок это сама запись.
    pub fn product_key(&self) -> &str {
        match self.product.is_empty() {
            false => bare(&self.product),
            true => self.snapshot_key(),
        }
    }

    /// Та ли это строка, о которой говорит ключ перехода. Сравнивается голыми
    /// ключами по той же причине, по какой они и голые: слэш есть в каталоге и
    /// нет в выдаче, а строка одна и та же.
    ///
    /// Спрашивают и путём продукта: ведут к снимку отовсюду по ключу
    /// хранилища, а собственный ключ есть только у строки снимка с несколькими
    /// упаковками, и знают его одни её же дети.
    pub fn named(&self, key: &str) -> bool {
        !key.is_empty()
            && (bare(self.key()) == bare(key) || bare(&self.identifier) == bare(key))
    }

    /// Снимок ли это — то, у чего есть контур, растры и своя единица показа.
    /// Род отвечает на это почти всегда; исключение — скачанный файл, который
    /// сам себе снимок: записью библиотеки он остаётся файлом
    /// (см. `downloaded_rows`).
    pub fn is_snapshot(&self) -> bool {
        self.kind.is_product()
            || (!self.product.is_empty() && self.product == self.snapshot_key())
    }

    /// Ключ, которым строку выбирают коробочкой. У снимка это его ключ у
    /// провайдера — тот самый, которым он очерчивается на шаре, — у остального
    /// собственный ключ строки.
    ///
    /// Написано здесь одно на всех, кто спрашивает про выбор: коробочка строки,
    /// коробочка шапки и обработчик выбора. Две копии этого правила однажды
    /// сочли бы одну строку за две — отмеченную и неотмеченную сразу.
    pub fn choice_key(&self) -> &str {
        match self.is_snapshot() {
            true => self.snapshot_key(),
            false => self.key(),
        }
    }

    /// Можно ли выбрать эту строку.
    ///
    /// Папку пути — нельзя: в неё заходят, а выбирают то, что лежит в каталоге
    /// или на диске. Снимок, лежащий каталогом (.SAFE, .SEN3), папкой в этом
    /// смысле не считается — выбирают как раз его.
    ///
    /// Безымянную — тоже нельзя: ключ выбора и есть то, чем его потом
    /// адресуют, и пустым адресовать нечего.
    pub fn choosable(&self) -> bool {
        !matches!(self.kind, RowKind::Folder) && !self.choice_key().is_empty()
    }

    /// Есть ли что раскрыть под этой строкой: упаковки снимка, его файлы или
    /// содержимое папки. У папки каталога оно ещё не приехало — раскрытие его
    /// и спросит.
    pub fn expandable(&self) -> bool {
        self.folded || self.group.is_some() || self.kind.is_folder()
    }

    /// Путь папки, в которой лежит запись: по нему строки группируются, он же
    /// показывается в меню строки.
    pub fn folder(&self) -> &str {
        folder_of(&self.identifier)
    }

    /// Сколько байт этой записи уже на диске: у недокачанной — скачанное, у
    /// готовой — весь размер.
    pub fn stored(&self) -> u64 {
        // У сложенной строки своих байтов нет: она сумма своих файлов, и
        // спрашивать её собственный статус значило бы считать снимок пустым
        // ровно тогда, когда часть его уже лежит на диске.
        if self.folded {
            return self.children.iter().map(Row::stored).sum();
        }
        match &self.status {
            RowStatus::Complete => self.size,
            RowStatus::Downloading { done, .. } | RowStatus::Paused { done, .. } => *done,
            RowStatus::Remote | RowStatus::Partial { .. } => 0,
        }
    }

    /// Колонка «формат»: то, чем запись назвал каталог, а без этого — род
    /// записи, а без него — расширение прописными. Тип от каталога старше
    /// всего остального: снимок подписывается своим типом («S2MSI2A»), и
    /// только безымянному достаётся слово строчными — это не расширение, и
    /// набрано оно поэтому иначе.
    pub fn format(&self) -> String {
        if !self.product_type.is_empty() {
            return self.product_type.clone();
        }
        match self.kind {
            RowKind::Product { .. } => return "снимок".to_string(),
            RowKind::Folder => return "папка".to_string(),
            RowKind::File => {}
        }
        match self.title.rfind('.') {
            Some(dot) => self.title[dot + 1..].to_uppercase(),
            None => String::new(),
        }
    }

    /// Строка того, что содержит другое: папки пути или снимка, разложенного
    /// каталогом. Размера и времени за ней не стоит — в S3 папка это общий
    /// префикс ключей, и ни того, ни другого за ним нет; у снимка их знает
    /// каталог, и приписывает их вызывающий (см. `components::rows::from_key`).
    pub fn container_row(identifier: String, title: String, status: RowStatus, kind: RowKind) -> Row {
        Row { identifier, title, kind, status, ..Default::default() }
    }

    /// Строка из записи библиотеки — вид «Скачанное».
    pub fn from_entry(entry: &LibraryEntry) -> Row {
        let status = match status_of(entry) {
            LibraryStatus::LibDownloading => RowStatus::Downloading { done: entry.done, total: entry.total },
            LibraryStatus::LibPaused => RowStatus::Paused {
                done: entry.done,
                total: entry.total,
                trouble: entry.trouble.clone(),
            },
            LibraryStatus::LibComplete => RowStatus::Complete,
        };
        Row {
            identifier: entry.identifier.clone(),
            name: entry.name.clone(),
            // Имя записи — путь внутри снимка, и целиком оно в строке не нужно:
            // снимок уже назван строкой выше, а под ним читают файл. Ключом при
            // этом остаётся имя целиком (поле `name`) — им запись адресуют.
            title: last_segment(&entry.name).to_string(),
            kind: RowKind::File,
            size: if entry.total > 0 { entry.total } else { entry.done },
            date: entry.modified,
            // Снимок едет с записью дальше: продолжение закачки уходит тем же
            // сообщением, и потерянный здесь снимок стёр бы принадлежность в
            // сидкаре — файл вывалился бы из своего снимка молча.
            product: entry.product.clone(),
            status,
            ..Default::default()
        }
    }

    /// Строка объекта каталога провайдера: состояние подтягивается из
    /// библиотеки, если такая запись там уже есть.
    ///
    /// Род приходит параметром, а не выводится здесь: снимком объект делает
    /// раскладка хранилища, а знает её только провайдер (см. `s3::product_root`).
    pub fn remote(
        library: &LibraryState,
        identifier: String,
        title: String,
        size: u64,
        date: i64,
        kind: RowKind,
    ) -> Row {
        match library.by_identifier(&identifier) {
            Some(entry) => Row {
                title,
                kind,
                // Размер и время у каталога точнее: библиотека знает только то,
                // что успело лечь на диск.
                size: if size > 0 { size } else { entry.total.max(entry.done) },
                date: if date > 0 { date } else { entry.modified },
                ..Row::from_entry(entry)
            },
            // Записи у библиотеки нет — значит запись лежит в хранилище, и это
            // ровно то, чем строка отвечает по умолчанию.
            None => Row { identifier, title, kind, size, date, ..Default::default() },
        }
    }
}

/// Все строки вида «Скачанное»: файлы одного снимка сведены в одну строку, а
/// то, что снимку не принадлежит, остаётся само по себе.
///
/// Свёртка здесь, а не в библиотеке: она ведёт учёт файлам — это её предмет, и
/// один файл там одна запись, — а «снимок» это то, чем его показывают. Знание,
/// какому снимку файл принадлежит, библиотека при этом хранит и отдаёт
/// (`LibraryEntry.product`): вывести его из имени она не могла бы.
pub fn downloaded_rows(library: &LibraryState) -> Vec<Row> {
    let mut products: Vec<(String, Vec<Row>)> = Vec::new();
    let mut alone: Vec<Row> = Vec::new();

    for entry in &library.entries {
        let row = Row::from_entry(entry);
        if entry.product.is_empty() {
            alone.push(row);
            continue;
        }
        // Порядок снимков — порядок первой встреченной записи: библиотека
        // отдаёт их уже в своём порядке, и пересортировать их всё равно
        // предстоит показу (см. `arrange`).
        match products.iter_mut().find(|(key, _)| key == &entry.product) {
            Some((_, files)) => files.push(row),
            None => products.push((entry.product.clone(), vec![row])),
        }
    }

    products
        .into_iter()
        .map(|(key, mut files)| match files.len() == 1 && files[0].identifier == key {
            // Снимок из одного файла — это и есть тот файл: обёртка над ним
            // носила бы тот же ключ строки, и список сопоставлял бы состояние
            // виджетов между ней и её же ребёнком.
            true => files.remove(0),
            // Состав снимка спрашиваем у библиотеки, а не складываем здесь:
            // правило «сколько файлов в снимке» одно на всех, и второй его
            // носитель разошёлся бы с первым (см. `LibraryState::snapshot`).
            false => {
                let (_, siblings) = library.snapshot(&key);
                snapshot(key, siblings, files)
            }
        })
        .chain(alone)
        .collect()
}

/// Причина, общая всем остановленным файлам снимка. Пусто — причины у них
/// разные или её нет вовсе.
///
/// Нужна она затем, что снимок приходит свёрнутым, и в самом частом случае —
/// отказала подпись, и с ней все два десятка файлов сразу — человек видит
/// только эту строку. Одну причину из нескольких разных она назвать не может:
/// строка говорит за все файлы разом, и назвавшая одну соврала бы про
/// остальные.
fn common_trouble(files: &[Row]) -> String {
    let mut said: Option<&str> = None;
    for file in files {
        let RowStatus::Paused { trouble, .. } = &file.status else { continue };
        match said {
            None => said = Some(trouble),
            Some(before) if before == trouble => {}
            Some(_) => return String::new(),
        }
    }
    said.unwrap_or_default().to_string()
}

/// Строка снимка поверх его файлов: то, что о снимке можно сказать, сложено из
/// того, что известно о каждом файле, — кроме одного. Сколько файлов в снимке
/// всего (`siblings`), из записей не выводится: их столько, сколько качали.
fn snapshot(product: String, siblings: u32, files: Vec<Row>) -> Row {
    let done = files.iter().filter(|file| matches!(file.status, RowStatus::Complete)).count();
    let downloading = files.iter().any(|file| matches!(file.status, RowStatus::Downloading { .. }));
    let size = files.iter().map(Row::stored).sum();
    // Время снимка — время самого свежего его файла: снимок «появился на
    // диске» тогда, когда лёг последний из них.
    let date = files.iter().map(|file| file.date).max().unwrap_or(0);

    let status = match (downloading, done, files.len()) {
        // Пока хоть один файл едет, снимок едет весь: сумма байтов по частям
        // была бы правдой о файлах, а не о нём.
        (true, ..) => RowStatus::Downloading { done: size, total: 0 },
        // «На диске» — только когда известно, из скольки файлов снимок состоит,
        // и все они здесь и доведены. Без этого числа доведённым выглядел бы
        // всякий снимок, у которого доведено скачанное, — три файла из
        // двадцати шести читались бы как целый снимок.
        (false, done, total) if siblings > 0 && done == total && total as u32 == siblings => {
            RowStatus::Complete
        }
        // Не доведён ни один — снимок оборван, а не «частично скачан»: зелёная
        // строка с нулём на диске обещает то, чего нет, и проходит отбор «на
        // диске» вместе с целыми.
        (false, 0, _) => RowStatus::Paused { done: size, total: 0, trouble: common_trouble(&files) },
        (false, done, _) => RowStatus::Partial { done, trouble: common_trouble(&files) },
    };

    Row {
        identifier: product.clone(),
        title: last_segment(&product).to_string(),
        kind: RowKind::Product { folder: true },
        size,
        date,
        product,
        children: files,
        folded: true,
        status,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::data_library::{LibraryEntry, LibraryStatus};

    fn entry(name: &str, product: &str, status: LibraryStatus, done: u64) -> LibraryEntry {
        LibraryEntry {
            name: name.to_string(),
            identifier: format!("eodata/{}/{}", product, name),
            product: product.to_string(),
            done,
            total: done,
            status: status as i32,
            modified: 0,
            siblings: 0,
            trouble: String::new(),
        }
    }

    /// Те же записи, но снимок к этому времени обойдён: в нём столько файлов,
    /// сколько сказано. Число носит каждая запись — оно и приходит к ним по
    /// одной, в сидкары (см. data-library::catalog::on_snapshot).
    fn walked(files: u32, mut entries: Vec<LibraryEntry>) -> Vec<LibraryEntry> {
        for entry in &mut entries {
            entry.siblings = files;
        }
        entries
    }

    /// Одиночный файл выбирается наравне со снимком.
    ///
    /// Коробочка — это выбор строки, а не показ контура: пакетом удаляют как
    /// раз такие файлы, а контура у них не бывает вовсе.
    #[test]
    fn a_lone_file_is_chosen_just_like_a_snapshot() {
        let mut entries =
            walked(1, vec![entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10)]);
        let mut lone = entry("dem.tif", "", LibraryStatus::LibComplete, 7);
        lone.identifier = "eodata/dem/dem.tif".to_string();
        entries.push(lone);
        let rows = downloaded_rows(&LibraryState { entries });

        let (snapshot, lone) = (&rows[0], &rows[1]);
        assert!(snapshot.choosable(), "снимок выбирается");
        assert!(lone.choosable(), "файл вне снимка тоже выбирается");
        assert!(!lone.is_snapshot(), "снимком он при этом не притворяется");
        assert_eq!(lone.choice_key(), "eodata/dem/dem.tif", "файл адресуется своим ключом");
    }

    /// Ключ выбора снимка — тот же, которым его очерчивают, и там, где
    /// собственный ключ строки с ним расходится, выбор обязан идти за ключом
    /// снимка: иначе один снимок стал бы двумя — выбранным и очерченным
    /// порознь.
    ///
    /// Расходятся они в двух местах: строка сетевого каталога приезжает со
    /// слэшем на конце, а у снимка с несколькими упаковками собственный ключ
    /// — ключ сцены (см. [`Row::key`]).
    #[test]
    fn a_snapshot_is_chosen_by_the_key_it_is_outlined_by() {
        let listed = Row::container_row(
            "eodata/store/S2B_X.SAFE/".to_string(),
            "S2B_X.SAFE".to_string(),
            RowStatus::Remote,
            RowKind::Product { folder: true },
        );
        assert_eq!(listed.choice_key(), listed.snapshot_key());
        assert_ne!(listed.choice_key(), listed.key(), "слэш листинга в ключ выбора не идёт");

        let parted = Row {
            group: Some("сцена".to_string()),
            ..Row::container_row(
                "eodata/store/S2B_X.SAFE".to_string(),
                "S2B_X.SAFE".to_string(),
                RowStatus::Remote,
                RowKind::Product { folder: false },
            )
        };
        assert_eq!(parted.choice_key(), parted.snapshot_key());
        assert_ne!(parted.choice_key(), parted.key(), "ключ сцены в ключ выбора не идёт");
    }

    /// Папку пути не выбирают: в неё заходят. А снимок, лежащий каталогом, —
    /// выбирают, хотя зайти в него тоже можно: «папка» у него про укладку, а
    /// не про род. Безымянную строку не выбирают тоже: ключ выбора и есть то,
    /// чем её потом адресуют.
    #[test]
    fn a_path_folder_is_not_chosen_but_a_snapshot_folder_is() {
        let folder = Row::container_row(
            "eodata/store/".to_string(),
            "store".to_string(),
            RowStatus::Remote,
            RowKind::Folder,
        );
        let snapshot = Row::container_row(
            "eodata/store/S2B_X.SAFE".to_string(),
            "S2B_X.SAFE".to_string(),
            RowStatus::Remote,
            RowKind::Product { folder: true },
        );

        assert!(!folder.choosable());
        assert!(snapshot.choosable());
        assert!(!Row::default().choosable(), "адресовать безымянную строку нечем");
    }

    /// Файлы одного снимка сходятся в одну строку, а то, что снимку не
    /// принадлежит, остаётся само по себе — это и есть весь смысл свёртки.
    #[test]
    fn files_of_one_snapshot_fold_into_one_row() {
        let mut entries = walked(2, vec![
            entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10),
            entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 20),
        ]);
        entries.push(entry("dem.tif", "", LibraryStatus::LibComplete, 7));
        let rows = downloaded_rows(&LibraryState { entries });

        assert_eq!(rows.len(), 2, "снимок и одиночный файл");
        let snapshot = &rows[0];
        assert_eq!(snapshot.title, "S2B_X.SAFE");
        assert!(snapshot.kind.is_product());
        assert_eq!(snapshot.children.len(), 2);
        // Размер снимка — сумма его файлов, а не размер какого-то одного.
        assert_eq!(snapshot.size, 30);
        assert!(matches!(snapshot.status, RowStatus::Complete));

        // Одиночный файл снимком не притворяется и детей не заводит.
        assert_eq!(rows[1].title, "dem.tif");
        assert!(!rows[1].kind.is_product());
        assert!(rows[1].children.is_empty());
    }

    /// Пока хоть один файл едет, едет весь снимок: «3 на диске» рядом с идущей
    /// закачкой говорило бы о файлах, а не о нём.
    #[test]
    fn snapshot_is_downloading_while_any_file_is() {
        let rows = downloaded_rows(&LibraryState {
            entries: vec![
                entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10),
                entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibDownloading, 5),
            ],
        });
        assert!(matches!(rows[0].status, RowStatus::Downloading { .. }));

        // Ни один не идёт, но и не все доведены — сказано, сколько доведено.
        let rows = downloaded_rows(&LibraryState {
            entries: vec![
                entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10),
                entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibPaused, 5),
            ],
        });
        assert!(matches!(rows[0].status, RowStatus::Partial { done: 1, .. }));
    }

    /// Целым снимок называется только по обходу: доведённые файлы говорят,
    /// сколько скачали, а не сколько в снимке есть.
    #[test]
    fn snapshot_is_whole_only_when_its_size_is_known() {
        let files = vec![
            entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10),
            entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 20),
        ];
        // Снимок не обходили: два доведённых файла — это два файла на диске.
        let rows = downloaded_rows(&LibraryState { entries: files.clone() });
        assert!(matches!(rows[0].status, RowStatus::Partial { done: 2, .. }));
        // Обошли и насчитали три — двух мало.
        let rows = downloaded_rows(&LibraryState { entries: walked(3, files.clone()) });
        assert!(matches!(rows[0].status, RowStatus::Partial { done: 2, .. }));
        // Насчитали два — снимок на диске целиком.
        let rows = downloaded_rows(&LibraryState { entries: walked(2, files) });
        assert!(matches!(rows[0].status, RowStatus::Complete));
    }

    /// Снимок, у которого не доведён ни один файл, оборван — а не «частично
    /// скачан»: зелёная строка с нулём на диске обещала бы то, чего нет.
    #[test]
    fn snapshot_of_interrupted_files_says_it_is_interrupted() {
        let rows = downloaded_rows(&LibraryState {
            entries: vec![
                entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibPaused, 3),
                entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibPaused, 4),
            ],
        });
        assert!(matches!(rows[0].status, RowStatus::Paused { .. }));
        // Байты при этом не теряются: у строки с детьми они сумма детей.
        assert_eq!(rows[0].stored(), 7);
    }

    /// Причину срыва строка снимка берёт у файлов — но только если она у них
    /// одна: снимок приходит свёрнутым, и назвавшая одну из нескольких разных
    /// соврала бы про остальные.
    #[test]
    fn a_snapshot_speaks_the_trouble_only_when_its_files_agree() {
        let troubled = |name: &str, said: &str| LibraryEntry {
            trouble: said.to_string(),
            ..entry(name, "S2B_X.SAFE", LibraryStatus::LibPaused, 3)
        };

        let rows = downloaded_rows(&LibraryState {
            entries: vec![troubled("B1.TIF", "HTTP 403"), troubled("B2.TIF", "HTTP 403")],
        });
        match &rows[0].status {
            RowStatus::Paused { trouble, .. } => assert_eq!(trouble, "HTTP 403"),
            _ => panic!("ожидалась оборванная строка"),
        }

        let rows = downloaded_rows(&LibraryState {
            entries: vec![troubled("B1.TIF", "HTTP 403"), troubled("B2.TIF", "нет места")],
        });
        match &rows[0].status {
            RowStatus::Paused { trouble, .. } => assert!(trouble.is_empty(), "причины разные"),
            _ => panic!("ожидалась оборванная строка"),
        }
    }

    /// Снимок из одного файла — это и есть тот файл: обёртка над ним носила бы
    /// тот же ключ строки, что и её единственный ребёнок.
    ///
    /// Записью библиотеки он при этом остаётся файлом, а снимком его делает
    /// совпадение ключей: род об этом сказать не может, а контур и шар у него
    /// есть наравне с прочими снимками.
    #[test]
    fn single_file_snapshot_is_not_wrapped() {
        let mut only = entry("S1A_X.zip", "eodata/S1A_X.zip", LibraryStatus::LibComplete, 9);
        only.identifier = "eodata/S1A_X.zip".to_string();
        let rows = downloaded_rows(&LibraryState { entries: vec![only] });
        assert_eq!(rows.len(), 1);
        assert!(rows[0].children.is_empty(), "обёртки нет");
        assert_eq!(rows[0].name, "S1A_X.zip", "запись библиотеки осталась записью");
        assert!(rows[0].is_snapshot(), "снимком его делает совпадение ключей");
        assert!(!rows[0].expandable(), "раскрывать в нём нечего");
    }

    /// Сложенная строка раскрывается, но сама записью не является: её файлы
    /// качают и открывают по одному, а её собственный ключ — путь снимка.
    #[test]
    fn a_folded_snapshot_expands_but_is_not_a_record() {
        let rows = downloaded_rows(&LibraryState {
            entries: vec![
                entry("B1.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 10),
                entry("B2.TIF", "S2B_X.SAFE", LibraryStatus::LibComplete, 20),
            ],
        });
        assert!(rows[0].folded);
        assert!(rows[0].expandable());
        assert!(rows[0].name.is_empty(), "своего имени в библиотеке у неё нет");
        // Файлы под ней — обычные записи, и раскрывать в них нечего.
        assert!(!rows[0].children[0].folded);
        assert!(!rows[0].children[0].expandable());
    }

    /// Слэш папки в ключе перехода в счёт не идёт: в каталоге он есть, а в
    /// выдаче поиска нет — строка при этом одна и та же.
    #[test]
    fn a_row_answers_to_its_key_with_or_without_the_folder_slash() {
        let row = Row::container_row(
            "eodata/S2B_X.SAFE/".to_string(),
            "S2B_X.SAFE".to_string(),
            RowStatus::Remote,
            RowKind::Product { folder: true },
        );
        assert!(row.named("eodata/S2B_X.SAFE"));
        assert!(row.named("eodata/S2B_X.SAFE/"));
        assert!(!row.named("eodata/S2B_Y.SAFE"));
        assert!(!row.named(""), "пустой ключ не называет никого");
    }

    /// Имя пути одно на всех, кто его называет, и слэш папки в счёт не идёт: из
    /// каталога снимок приходит со слэшем, из выдачи поиска без него, а зовут
    /// его одинаково.
    #[test]
    fn the_last_segment_ignores_the_folder_slash() {
        assert_eq!(last_segment("eodata/S2B_X.SAFE/"), "S2B_X.SAFE");
        assert_eq!(last_segment("eodata/S2B_X.SAFE"), "S2B_X.SAFE");
        assert_eq!(last_segment("S2B_X.SAFE/GRANULE/B01.jp2"), "B01.jp2");
        assert_eq!(last_segment("dem.tif"), "dem.tif");
        // Сегмента нет вовсе — имя брать неоткуда: так выглядит корень бакета.
        assert_eq!(last_segment(""), "");
        assert_eq!(last_segment("/"), "");
    }

    /// Действие про снимок, нажатое на файле, адресуется снимку: наложение
    /// ложится под тем ключом, которым продукт зовёт провайдер, и ключом файла
    /// снять его потом было бы нечем.
    #[test]
    fn a_file_speaks_for_the_snapshot_it_belongs_to() {
        let snapshot = "eodata/store/S2B_X.SAFE";
        let mut file = Row {
            identifier: format!("{}/GRANULE/B01.jp2", snapshot),
            product: snapshot.to_string(),
            ..Default::default()
        };
        assert!(!file.is_snapshot(), "файл снимком не притворяется");
        assert_eq!(file.product_key(), snapshot);
        assert_ne!(file.product_key(), file.snapshot_key(), "файл позвал бы сам себя");

        // Снимок отвечает за себя сам — и тогда, когда каталог назвал его
        // продуктом, и тогда, когда о продукте промолчал. Слэш листинга в счёт
        // не идёт ни там, ни там: ключ у снимка один.
        let listed = Row {
            product: snapshot.to_string(),
            ..Row::container_row(
                format!("{}/", snapshot),
                "S2B_X.SAFE".to_string(),
                RowStatus::Remote,
                RowKind::Product { folder: true },
            )
        };
        assert!(listed.is_snapshot());
        assert_eq!(listed.product_key(), snapshot);
        assert_eq!(listed.product_key(), listed.snapshot_key());

        // Провайдер промолчал о снимке — записью и остаётся: звать её больше
        // нечем. Так приходит одиночный объект вне продуктов.
        file.product = String::new();
        assert_eq!(file.product_key(), file.snapshot_key());
    }

    /// Смотреть скачанное надо с диска: файл под рукой, и ходить за ним по
    /// сети значит ждать того, что уже есть.
    #[test]
    fn preview_prefers_the_downloaded_file() {
        use crate::module::ViewMsg;
        let key = "eodata/S1A_X.SAFE/quick-look.png";

        let done = LibraryState {
            entries: vec![entry("quick-look.png", "S1A_X.SAFE", LibraryStatus::LibComplete, 4)],
        };
        assert!(
            matches!(preview_of(&done, key, false), ViewMsg::Preview(name) if name == "quick-look.png")
        );

        // Недокачанного на диске ещё нет — смотреть его надо из хранилища.
        let started = LibraryState {
            entries: vec![entry("quick-look.png", "S1A_X.SAFE", LibraryStatus::LibPaused, 4)],
        };
        assert!(matches!(preview_of(&started, key, false), ViewMsg::PreviewRemote(_)));

        // Снимок, лежащий каталогом, открывает провайдер: GET по пути каталога
        // отвечает 404, и растр внутри выбирает он.
        assert!(matches!(
            preview_of(&started, "eodata/S1A_X.SAFE", true),
            ViewMsg::PreviewProduct(_)
        ));
    }
}
