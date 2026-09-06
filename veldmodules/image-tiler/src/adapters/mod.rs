//! Адаптеры форматов: описать растр по заголовку и произвести тайлы.
//!
//! Формат определяется по содержимому — расширение врёт чаще заголовка, — и
//! байты идут окнами из ресурса: файл это, сеть или память, отсюда не видно.
//! Разница между форматами одна: умеет ли он отдать произвольный тайл
//! дёшево. Отвечает на это таблица уровней (`table.rs`): точечный уровень
//! читает ровно нужные чанки (`grid.rs`), остальные идут проходом, который
//! строит каскадом все уровни разом (см. cascade.rs), и запрошенные тайлы
//! уезжают по мере прохода, сверху вниз.

use std::cell::Cell;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::rc::Rc;

use image::ImageFormat;

use super::cascade::Emit;
use super::pyramid;

pub mod codec;
pub mod excerpt;
pub mod full;
pub mod grid;
pub mod jp2;
pub mod jpeg;
pub mod netcdf;
pub mod png;
pub mod radiometry;
pub mod table;
pub mod tiff;

/// Тесты чтения на фальшивом хосте — свойства `describe`/`produce` по окнам,
/// вынесенные файлом ради фикстур: настоящие TIFF пишутся здесь же.
#[cfg(test)]
mod reads;

/// Потолок стороны источника. Стережёт он две разные вещи разом.
///
/// **Правдоподобие.** Растр с шестизначной стороной — это чаще не растр, а
/// разобранный не так заголовок, и узнать об этом дешевле до чтения.
///
/// **Цену каскада — у тех, кто её не считает.** Полосы каскада растут от
/// ширины (около пяти килобайт на колонку, [`crate::cascade::bytes`]): на этой
/// стороне они просят около трети свободной памяти инстанса, то есть проходят.
/// Сетка чанков складывает эту цену с полосой и свежим чанком сама
/// (`Grid::pass_peak`) и потому отвергает раньше; у PNG и JPEG своего счёта
/// нет, и по стороне это их единственная граница.
pub const MAX_SOURCE_SIDE: u32 = 65_536;

/// Потолок для путей без потоковой развёртки (gif/bmp/webp, Adam7-PNG,
/// декодированный кадр JPEG): такой кадр живёт в памяти целиком.
pub const FULL_DECODE_BUDGET: u64 = 256 * 1024 * 1024;

/// Влезает ли кадр `width`×`height` в RGBA под потолок кадра целиком. Одна
/// проверка на таблицу уровней и на сами пути: столбец «влезает» и отказ перед
/// выделением кадра обязаны сходиться.
pub fn frame_fits(width: u32, height: u32) -> bool {
    u64::from(width) * u64::from(height) * 4 <= FULL_DECODE_BUDGET
}

/// Что известно о растре без декодирования.
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub kind: Kind,
    /// Сетка геопривязки, если файл её несёт: где какой пиксель лежит на
    /// Земле. Пусто у форматов без места для неё — это не «неизвестно
    /// откуда», а «в файле не сказано».
    pub ties: Vec<Tie>,
    /// Привязка к проекции — у растра, лежащего не в градусах. Взаимоисключима
    /// с [`Info::ties`]: узлы объявлены в градусах, и метрам зоны в них места
    /// нет (см. [`Placement`]).
    pub placement: Option<Placement>,
    /// Отсчёт прибора, в котором записан сам растр. Нужен не ему, а
    /// координатам из соседнего файла: сетка там стои́т в своём отсчёте, и
    /// сойтись с растром они могут только через эту пару (см.
    /// [`netcdf::geolocation`]). Заполняет один NetCDF — остальные о таком
    /// не говорят.
    pub frame: Option<netcdf::Frame>,
    /// Файл о привязке заговорил, а взять её не удалось — причина словами.
    ///
    /// Пусто значит одно из двух, и различать их незачем: либо привязка взята,
    /// либо файл о ней не заговаривал вовсе. Оговорка нужна ровно там, где
    /// сказанное в файле не доехало: по пустым [`Info::ties`] «в файле не
    /// сказано» от «сказано, да не прочиталось» не отличить, а снимок во втором
    /// случае ляжет догадкой и будет выглядеть настоящим.
    ///
    /// Говорит адаптер, а не потребитель: почему не сложилось, знает только
    /// тот, кто читал файл.
    pub binding_trouble: Option<String>,
    /// Какая из величин файла показывается — у файла многих величин (NetCDF):
    /// выбор адаптера либо названная заказчиком (`wanted` у [`describe`]).
    /// Пусто у снимка: величина у него одна, и имени у неё нет.
    pub variable: Option<Variable>,
    /// Все величины файла, которые могли бы быть показаны, в порядке
    /// предпочтения адаптера; показанная — среди них. Годность здесь по
    /// заголовкам, не по отсчётам: пустая или однотонная в этой грануле
    /// величина в списке есть. Пусто у снимка.
    pub variables: Vec<Variable>,
}

/// Величина файла многих величин — показанная или одна из перечисленных: как
/// она называется в файле и как её назвал файл словами. Ровно то, что смотрящему нужно, чтобы понять,
/// на что он смотрит, и ничего для показа — растяг и раскладка лежат в
/// [`Kind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    /// Путь внутри файла (`/PRODUCT/carbonmonoxide_total_column`).
    pub path: String,
    /// `long_name` либо `standard_name`; пусто — файл не назвал.
    pub said: String,
    /// `units`, как записано в файле; пусто — не записано.
    pub units: String,
}

/// Узел сетки геопривязки: пиксель растра и его место на Земле, WGS84.
///
/// Сеткой, а не рамкой: радарный снимок лежит в геометрии съёмки, и прямоугольник
/// в градусах его не описывает — GeoTIFF такого растра несёт решётку опорных
/// точек (у гранулы Sentinel-1 это 21×21), а рамка получилась бы только у
/// проекции с севером кверху.
pub struct Tie {
    pub px: f64,
    pub py: f64,
    pub lat: f64,
    pub lon: f64,
}

/// Годна ли пара, записанная в узле.
///
/// Одна на всех поставщиков нарочно. Незаполненный отсчёт NetCDF помечен не
/// только `NaN`, но и числом — −999 у SYNERGY, 9.96921e36 у CF, — и по кругу
/// это видно; отдельной проверки на конечность круг не требует, сравнения с
/// `NaN` ложны, а бесконечность за край выходит. У GeoTIFF заполнителя в теге
/// не бывает, но число там читается из файла, а файл приходит из сети:
/// бесконечная долгота, доехав до потребителя, уводит разворот в арифметику,
/// которой у места нет.
///
/// Долгота проверяется наравне с широтой: у решётки, проверенной лишь по
/// широте, дыра прошла бы долготой. Круг у долготы полный с запасом — файлы
/// пишут её и как −180…180, и как 0…360, а развернёт её потребитель
/// (`Grid::unwind`).
pub fn placed(lat: f64, lon: f64) -> bool {
    (-90.0..=90.0).contains(&lat) && (-360.0..=360.0).contains(&lon)
}

/// Привязка растра, лежащего в проекции: код EPSG системы координат и линейное
/// преобразование пикселя в её метры.
///
/// Кодом, а не зоной: код записан в файле, а «зона 38 северная» — уже
/// толкование, и толкует его тот, кто знает Землю. Перевести здесь значило бы
/// завести вторую копию проекционной математики.
///
/// Шестёркой, а не парой «начало и шаг»: она покрывает обе привязки GeoTIFF —
/// и точку с шагом пикселя, и матрицу, — то есть повёрнутый растр тоже.
pub struct Placement {
    pub epsg: u32,
    /// `x = a[0]·i + a[1]·j + a[2]`, `y = a[3]·i + a[4]·j + a[5]`, где (i, j) —
    /// угол пикселя, ноль в левом верхнем углу растра, ось j вниз. Полпикселя
    /// RasterPixelIsPoint снято здесь же, как оно снимается у узлов.
    pub affine: [f64; 6],
}

pub enum Kind {
    /// PNG потоком по строкам; чересстрочный (Adam7) — кадром целиком.
    Png { interlaced: bool },
    Jpeg,
    /// JPEG 2000: тайлы кодстрима — чанки драйвера, копии — уровни разрешения.
    Jp2(jp2::Layout),
    Tiff(tiff::Layout),
    /// Форматы без потокового пути: декодируются целиком, они малы по природе.
    Full(ImageFormat),
    /// NetCDF-4: не картинка, а набор измеренных величин. Одна из них выбрана
    /// показываемой при описании, и её сетка — окна строк во всю ширину —
    /// чанки драйвера (см. `netcdf::Layout`).
    Netcdf(netcdf::Layout),
}

impl Info {
    /// Растр без геопривязки: места под опорные точки в файле нет вовсе. Так
    /// описываются все форматы, кроме тех двух, что о своём месте на Земле
    /// говорят сами, — GeoTIFF и NetCDF.
    pub fn plain(width: u32, height: u32, kind: Kind) -> Self {
        Self {
            width,
            height,
            kind,
            ties: Vec::new(),
            placement: None,
            frame: None,
            binding_trouble: None,
            variable: None,
            variables: Vec::new(),
        }
    }
}

/// Заголовок растра: формат, размеры, раскладка. Дешёвый и для гигабайтного
/// файла — читаются заголовки и каталоги, не пиксели.
pub fn describe(resource_id: u64, len: u64, wanted: &str, bytes: &Rc<Cell<u64>>) -> Result<Info, String> {
    let mut reader = Metered::new(resource_id, len, bytes.clone());
    let mut head = [0u8; 32];
    let read = reader.read(&mut head).map_err(|e| format!("чтение заголовка: {}", e))?;
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;

    // Величину называют файлу многих величин; у снимка она одна и без имени,
    // и названная там — ошибка заказчика, а не выбор.
    if !wanted.is_empty() && !head.starts_with(netcdf::MAGIC) {
        return Err(format!("величина '{}' спрошена у файла одной величины", wanted));
    }

    // Ни JPEG 2000, ни NetCDF, ни BigTIFF крейт `image` не знает — их сигнатуры
    // смотрятся до него.
    if head.starts_with(jp2::JP2_MAGIC) || head.starts_with(jp2::CODESTREAM_MAGIC) {
        let info = jp2::describe(reader, len)?;
        return checked(info);
    }
    if head.starts_with(netcdf::MAGIC) {
        // Ресурсом, а не читателем: HDF5 ходит по файлу вразброс абсолютными
        // смещениями, и оборачивать это в последовательный поток значило бы
        // отнять у него ровно то, чем он и дёшев (см. `netcdf::Resource`).
        drop(reader);
        let info = netcdf::describe(resource_id, len, wanted)?;
        return checked(info);
    }
    // Классический NetCDF-3 — другой формат, и читателя у него здесь нет.
    // Назван он отдельно затем, что общий отказ ниже перечисляет NetCDF среди
    // открываемых: сказанный над файлом `.nc`, он читается как поломка.
    if netcdf::CLASSIC.iter().any(|magic| head.starts_with(magic)) {
        return Err("это классический NetCDF-3, а читается NetCDF-4 — тот, что записан HDF5"
            .to_string());
    }
    if tiff::BIG_MAGIC.iter().any(|magic| head.starts_with(magic)) {
        let info = tiff::describe(reader)?;
        return checked(info);
    }

    // В отказе перечислено, что вообще открывается: файл без сигнатуры
    // изображения — обычное дело в каталоге (сырец L0, разметка, XML), и
    // «не распознан» без списка читается как поломка, а не как ответ.
    let format = image::guess_format(&head[..read]).map_err(|_| {
        "по заголовку это не изображение \
         (открываются PNG, JPEG, TIFF, JPEG 2000, NetCDF, GIF, BMP, WebP)"
            .to_string()
    })?;

    let info = match format {
        ImageFormat::Png => png::describe(reader),
        ImageFormat::Jpeg => jpeg::describe(reader),
        ImageFormat::Tiff => tiff::describe(reader),
        // Набор обязан совпадать с features крейта `image` в config.yaml —
        // фича без рукава молча даст «не поддерживается».
        ImageFormat::Gif | ImageFormat::Bmp | ImageFormat::WebP => full::describe(reader, format),
        other => Err(format!("формат {:?} не поддерживается", other)),
    }?;
    checked(info)
}

/// Общие пределы описанного растра — какой бы адаптер его ни описал.
fn checked(info: Info) -> Result<Info, String> {
    if info.width == 0 || info.height == 0 {
        return Err("пустой растр".to_string());
    }
    if info.width > MAX_SOURCE_SIDE || info.height > MAX_SOURCE_SIDE {
        return Err(format!(
            "{}×{}: сторона больше потолка {} — столько не бывает у растра, \
             а полосы прохода по нему не влезли бы в память инстанса",
            info.width, info.height, MAX_SOURCE_SIDE
        ));
    }
    // Источник, у которого в память не влезает ни один уровень, не
    // описывается: описание обещало бы предел детали на вершине, а тайлы не
    // пришли бы никогда. Отказ называет слагаемые самой дешёвой строки.
    let rows = info.levels();
    if let Some(top) = rows.last()
        && rows.iter().all(|row| !row.fits)
    {
        return Err(format!("ни один уровень не влезает в память: {}", top.peak.note()));
    }
    Ok(info)
}

/// Произвести тайлы уровня `level`. `wants` использует только точечный путь —
/// он читает ровно запрошенное; проход отдаёт в `emit` все тайлы всех
/// уровней, а кто из них кому нужен, решает приёмник.
///
/// Рукав выбирает строка таблицы уровней ([`Info::level`]): та же, что уехала
/// потребителю в описании (`Described.levels`), — обещанная ему цена и
/// заплаченная здесь считаны одной строкой.
pub fn produce(
    resource_id: u64,
    len: u64,
    info: &Info,
    level: u32,
    wants: &[(u32, u32)],
    bytes: &Rc<Cell<u64>>,
    emit: Emit,
) -> Result<(), String> {
    let reader = || Metered::new(resource_id, len, bytes.clone());
    let row = info.level(level).ok_or_else(|| {
        format!("уровня {} нет: у растра их {}", level, pyramid::level_count(info.width, info.height))
    })?;
    match (&info.kind, row.serve) {
        // Точечно — где окно тайла и правда окно. Решается это уровнем, а не
        // источником: полосная гранула читается точечно вблизи и проходом
        // издали, и грубый край закрывает проход. Уровень, взятый проходом,
        // приезжает медленно; уровень, взятый отказом, не приезжает никогда —
        // поэтому точечным таблица называет только то, что драйвер отдаст.
        (Kind::Tiff(layout), table::Serve::Pointwise) => {
            tiff::produce_direct(reader(), resource_id, info, layout, level, wants, emit)
        }
        (Kind::Tiff(layout), table::Serve::Pass { .. }) => {
            tiff::produce_pass(reader(), resource_id, info, layout, emit)
        }
        // Кодеку нужен свой поток на каждый фактор, и читателей JP2 заводит сам.
        (Kind::Jp2(layout), table::Serve::Pointwise) => {
            jp2::produce_direct(resource_id, bytes, info, layout, level, wants, emit)
        }
        (Kind::Jp2(layout), table::Serve::Pass { .. }) => jp2::produce_pass(resource_id, bytes, info, layout, emit),
        // HDF5 ходит по файлу вразброс абсолютными смещениями, и читателя
        // ресурса NetCDF заводит сам (см. `netcdf::Resource`).
        (Kind::Netcdf(layout), table::Serve::Pointwise) => {
            netcdf::produce_direct(resource_id, len, bytes, info, layout, level, wants, emit)
        }
        (Kind::Netcdf(layout), table::Serve::Pass { .. }) => {
            netcdf::produce_pass(resource_id, len, bytes, info, layout, emit)
        }
        // У остальных путь один, и строка таблицы говорит лишь, откуда он
        // начинается.
        (Kind::Png { .. }, _) => png::produce_pass(reader(), info, emit),
        (Kind::Jpeg, _) => jpeg::produce(reader(), info, level, emit),
        (Kind::Full(format), _) => full::produce(reader(), info, *format, emit),
    }
}

/// Развёртка сэмплов в RGBA8 — общая всем адаптерам: 1 канал — серый,
/// 2 — серый с альфой, 3 — RGB, 4 — как есть.
/// За сколько шагов растекается цвет под прозрачное. Фильтрация смешивает
/// тексель с непосредственными соседями, поэтому дальше второго кольца
/// растекаться незачем.
const BLEED_STEPS: u32 = 2;

/// Цвет под прозрачным — от ближайшего непрозрачного соседа.
///
/// Под полностью прозрачным пикселем лежит чёрный: цвета у него нет, а ноль
/// держит выход детерминированным (см. `resample`). На экране этот ноль виден.
/// И канва, и шар фильтруют текстуру линейно и смешивают непремультиплицированной
/// альфой, поэтому на кромке поля `nodata` интерполяция даёт половину цвета при
/// половине непрозрачности — тёмный ореол шириной в тексель. Ячейка,
/// нарисованная предком, растягивает его вдвое на каждую ступень, так что на
/// грубой ступени это заметная тёмная обводка вокруг снимка.
///
/// Альфа при этом не трогается: кромка остаётся ровно там же, где была, а
/// смешиваться начинает цвет с цветом, а не цвет с чернотой.
pub fn bleed_alpha(rgba: &mut [u8], width: u32, height: u32) {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return;
    }
    let mut coloured: Vec<bool> = (0..w * h).map(|at| rgba[at * 4 + 3] != 0).collect();
    // Сплошь непрозрачный тайл — кромки нет; сплошь прозрачный — брать цвет
    // неоткуда.
    if coloured.iter().all(|has| *has) || coloured.iter().all(|has| !has) {
        return;
    }

    for _ in 0..BLEED_STEPS {
        // Снимок прошлого кольца: без него цвет уезжал бы вглубь поля за один
        // проход, и растекание зависело бы от порядка обхода.
        let known = coloured.clone();
        let mut spread = false;
        for at in 0..w * h {
            if known[at] {
                continue;
            }
            let (x, y) = (at % w, at / w);
            let neighbours = [
                (x > 0).then(|| at - 1),
                (x + 1 < w).then_some(at + 1),
                (y > 0).then(|| at - w),
                (y + 1 < h).then_some(at + w),
            ];
            let Some(from) = neighbours.into_iter().flatten().find(|near| known[*near]) else {
                continue;
            };
            let colour = [rgba[from * 4], rgba[from * 4 + 1], rgba[from * 4 + 2]];
            rgba[at * 4..at * 4 + 3].copy_from_slice(&colour);
            coloured[at] = true;
            spread = true;
        }
        if !spread {
            break;
        }
    }
}

pub fn to_rgba(samples: &[u8], pixel: radiometry::Pixel, pixels: usize) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixels * 4);
    for px in samples.chunks_exact(pixel.channels).take(pixels) {
        let (rgb, alpha) = match (pixel.colors(), pixel.has_alpha()) {
            (1, false) => ([px[0], px[0], px[0]], 255),
            (1, true) => ([px[0], px[0], px[0]], px[pixel.channels - 1]),
            (_, false) => ([px[0], px[1], px[2]], 255),
            (_, true) => ([px[0], px[1], px[2]], px[pixel.channels - 1]),
        };
        rgba.extend_from_slice(&rgb);
        rgba.push(alpha);
    }
    rgba
}

/// Читатель ресурса со счётчиком прочитанного — из него растёт честный
/// прогресс долгого прохода. Счётчик — дальняя достигнутая позиция, а не
/// сумма чтений: повторные окна после seek назад не приближают конец файла
/// и прогрессом не являются, а сумма перевалила бы за размер. Разделён с
/// приёмником тайлов через Rc — инстанс однопоточный.
pub struct Metered {
    inner: veldsdk::ResourceReader,
    /// Текущая позиция: и read, и consume двигают её сами, потому что
    /// спрашивать её у ресурса — лишний ABI-вызов на каждое чтение.
    pos: u64,
    consumed: Rc<Cell<u64>>,
}

impl Metered {
    pub fn new(resource_id: u64, len: u64, consumed: Rc<Cell<u64>>) -> Self {
        Self { inner: veldsdk::ResourceReader::new(resource_id, len), pos: 0, consumed }
    }

    fn advance(&mut self, n: usize) {
        self.pos += n as u64;
        if self.pos > self.consumed.get() {
            self.consumed.set(self.pos);
        }
    }
}

impl Read for Metered {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.advance(n);
        Ok(n)
    }
}

impl BufRead for Metered {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.advance(amt);
        self.inner.consume(amt);
    }
}

impl Seek for Metered {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let at = self.inner.seek(pos)?;
        self.pos = at;
        Ok(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_rgba_expands_all_channel_counts() {
        assert_eq!(to_rgba(&[7], radiometry::Pixel::named(1), 1), vec![7, 7, 7, 255]);
        assert_eq!(to_rgba(&[7, 9], radiometry::Pixel::named(2), 1), vec![7, 7, 7, 9]);
        assert_eq!(to_rgba(&[1, 2, 3], radiometry::Pixel::named(3), 1), vec![1, 2, 3, 255]);
        assert_eq!(to_rgba(&[1, 2, 3, 4], radiometry::Pixel::named(4), 1), vec![1, 2, 3, 4]);
        // Хвост за пределами pixels отбрасывается — у краевых чанков TIFF
        // данные бывают шире полезной части.
        assert_eq!(to_rgba(&[1, 2, 3, 4, 5, 6], radiometry::Pixel::named(3), 1), vec![1, 2, 3, 255]);
    }

    /// Источник, у которого не влезает ни один уровень, описанием не проходит:
    /// иначе предел детали обещал бы вершину, а тайлы не пришли бы никогда.
    /// Полоса во весь растр 12000² держит проходом копию всего растра дважды.
    #[test]
    fn источник_без_влезающего_уровня_не_описывается() {
        let whole = Info::plain(
            12000,
            12000,
            Kind::Tiff(tiff::Layout::of(false, (12000, 12000), Vec::new(), 3)),
        );
        let why = checked(whole).err().expect("полоса во весь растр не влезает");
        assert!(why.contains("не влезает") && why.contains("полоса"), "{why}");
        assert!(checked(Info::plain(64, 64, Kind::Jpeg)).is_ok());
    }

    /// Под прозрачным оказывается цвет соседа, а не чернота, — иначе линейная
    /// фильтрация даёт по кромке `nodata` тёмный ореол. Сама прозрачность при
    /// этом остаётся на месте: кромка там же, где была.
    #[test]
    fn colour_bleeds_under_the_transparent_edge() {
        // Две колонки: левая — данные, правая — поле.
        let mut tile = vec![9, 9, 9, 255, 0, 0, 0, 0, 9, 9, 9, 255, 0, 0, 0, 0];
        bleed_alpha(&mut tile, 2, 2);
        assert_eq!(&tile[4..8], &[9, 9, 9, 0], "цвет пришёл, прозрачность осталась");
        assert_eq!(&tile[12..16], &[9, 9, 9, 0]);
        assert_eq!(&tile[0..4], &[9, 9, 9, 255], "данные не тронуты");

        // Сплошь прозрачный тайл брать цвет неоткуда — он и остаётся чёрным.
        let mut empty = vec![0u8; 16];
        bleed_alpha(&mut empty, 2, 2);
        assert_eq!(empty, vec![0u8; 16]);
    }
}
