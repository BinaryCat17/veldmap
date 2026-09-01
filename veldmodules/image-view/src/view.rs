//! Одна канва: что в ней показано, куда смотрит камера и как из тайлов
//! собирается кадр.
//!
//! Ячейка кадра — тайл текущего уровня. Рисуется она ровно одним квадом:
//! точным тайлом, а пока его нет — куском ближайшего имеющегося предка
//! (адресация пирамиды это позволяет: тайл уровня z+1 накрывает четыре тайла
//! уровня z, см. resample::halve у тайлера). Одна ячейка — один квад, поэтому
//! перекрытий нет и буфер глубины не нужен.

use veldmap_image_tiler_wrap::pyramid::{self, TILE};
use veldmap_image_tiler_wrap::tiles::{self, Fetch, Meta, Passes, Store};
use veldsdk::graphics::GrowingBuffer;

use super::camera::Camera;
use super::gpu::{self, Quad, Target};

pub struct View {
    pub label: String,
    pub target: Option<Target>,
    pub shown: Option<Shown>,
    /// Камера появляется первым вписыванием — когда известны и снимок, и
    /// размер канвы — и дальше живёт своей жизнью.
    pub camera: Option<Camera>,
    pub vertices: GrowingBuffer,
    /// Из чего собран последний записанный кадр; совпало — кадр пропускается.
    pub drawn: Option<Stamp>,
    /// Что уже спрошено, что производится и чего больше не просить — общий
    /// учёт потребителя тайлов (см. `veldmap_image_tiler_wrap::tiles`).
    pub fetch: Fetch,
    /// Актуальность описания: показ могли сменить, пока ответ шёл.
    pub describe: veldsdk::Latest,
    pub read_bytes: u64,
    pub total_bytes: u64,
    /// Снимок лежит на диске, а не за проводом. Своего смысла у признака здесь
    /// нет — его пересказывают тайлеру, у которого от этого зависит потолок
    /// источника, читаемого только целиком (см. `DescribeRequest.near`).
    pub near: bool,
    /// Смотреть не на что: ресурс не открылся или не описался.
    pub error: Option<String>,
    /// Кадр неполон: сорвался проход, отказал кэш. Снимок при этом жив, и
    /// показывать причину вместо него было бы неправдой — а держать её вечно
    /// незачем: снимается первым же приехавшим тайлом (см. [`View::landed`]).
    ///
    /// Слово о причине говорится здесь целиком: на провод обе жалобы уезжают
    /// одним полем, и заказчик показывает сказанное как есть.
    pub trouble: Option<String>,
    /// В прошлой сборке квадов было чему рябить. Полем, а не ответом на месте:
    /// отпечаток кадра сравнивается раньше, чем считаются квады, — а решать по
    /// нему, двигать ли фазу, надо до сравнения. Опоздание на кадр здесь
    /// безобидно: пока рябь идёт, квады пересчитываются каждым тиком и признак
    /// обновляется вместе с ними, а погаснув, он гасит и сам пересчёт.
    pub glowing: bool,
    /// Кадр не записался вовсе — это жалоба на сам рендер, а не на конвейер.
    /// Врозь с `trouble` затем, что снимают их разные события: конвейерную
    /// снимает приехавший тайл, эту — удавшийся кадр. Одним полем они гасили
    /// бы друг друга: успешная перерисовка дыры стирала бы «не доехало», а
    /// приехавший тайл — «кадр застыл», который никуда не делся.
    pub stuck: Option<String>,
}

/// Показанный ресурс. Метаданные приходят вторым тактом (on_described),
/// и до них снимок «есть, но рисовать нечего».
pub struct Shown {
    pub resource: veldsdk::OwnedResource,
    pub meta: Option<Meta>,
}


/// То, из чего собран кадр. Поколение хранилища тайлов заменяет сравнение
/// самих наборов: любой пришедший или вытесненный тайл его двигает.
#[derive(Clone, Copy, PartialEq)]
pub struct Stamp {
    pub camera: Camera,
    pub generation: u64,
    pub texture: u64,
    /// Фаза ряби, огрублённая до шага показа. Пока рябить нечему, она стои́т —
    /// и кадр пропускается ровно так же, как до неё.
    pub ripple: u32,
}

impl View {
    pub fn new(label: String) -> Self {
        Self {
            label,
            target: None,
            shown: None,
            camera: None,
            vertices: gpu::vertex_buffer(),
            drawn: None,
            fetch: Fetch::default(),
            describe: veldsdk::Latest::default(),
            read_bytes: 0,
            total_bytes: 0,
            near: false,
            error: None,
            trouble: None,
            stuck: None,
            glowing: false,
        }
    }

    pub fn meta(&self) -> Option<&Meta> {
        self.shown.as_ref()?.meta.as_ref()
    }

    /// Тайл лёг — жалоба на прошлое снимается. Картинка достраивается прямо
    /// сейчас, и подпись «не доехало» рядом с ней говорила бы о том, чего уже
    /// нет.
    pub fn landed(&mut self) {
        self.trouble = None;
    }

    /// Жалоба на провод: одна строка, а поводов три, и порядок у них по
    /// срочности. Застрявший кадр важнее неполноты — неполный кадр хотя бы
    /// рисуется; неполнота важнее предела детали — она про сейчас, а предел
    /// никуда не денется.
    ///
    /// Предел детали (`Meta::capped`) считается здесь, а не хранится в
    /// [`View::trouble`], потому что снимают их разные события: `trouble`
    /// снимает первый же приехавший тайл ([`View::landed`]), а предел детали —
    /// свойство самого источника, и снять его может только новое описание.
    /// Лёг бы он в то же поле — мигнул бы и пропал с первым тайлом, то есть
    /// ровно тогда, когда смотрящий начал бы разглядывать картинку.
    ///
    /// `settled` — канва показывает и работы за ней нет; иначе предел молчит.
    /// Заказчик показывает жалобу **вместо** хода добычи (`preview.rs`), а у
    /// JP2 обе ступени лестницы — это по полному чтению файла, десятки секунд
    /// на пустом месте. «Подробнее не будет» над пустой канвой читается как
    /// «загрузка кончилась, вот её потолок», то есть отвечает не на тот вопрос,
    /// который задан.
    ///
    /// Одной незанятости мало, и это померено: у описанного снимка, которому
    /// ещё не дали ни места, ни камеры, желаемого нет вовсе — а раз нечего
    /// хотеть, то и работы нет. Поэтому обе половины: показ идёт (`want` есть)
    /// и по нему всё добрано. Наложение на шаре считает `settled` тем же
    /// способом и по той же причине (`Overlay::said`).
    pub fn said(&self, settled: bool) -> String {
        self.stuck
            .clone()
            .or_else(|| self.trouble.clone())
            .or_else(|| match settled {
                true => self.meta()?.capped(),
                false => None,
            })
            .unwrap_or_default()
    }

    /// Показ идёт: описание в пути либо по пирамиде ещё есть работа. Второе
    /// считает общее правило (`tiles::working`) — то же, по которому отвечает
    /// на этот вопрос наложение на шаре.
    pub fn working<K: PartialEq>(&self, passes: &Passes<K>, want: Option<&tiles::Want>) -> bool {
        self.describe.is_pending()
            || self.meta().is_some_and(|meta| {
                tiles::working(&self.fetch, passes, &meta.fingerprint, want)
            })
    }
}

/// Что нужно канве прямо сейчас: цель, ступень к ней и её ячейки. `None` —
/// рисовать пока не из чего (нет места, снимка или камеры).
///
/// Считает это общее правило (`tiles::want`) — то же, по которому выбирает
/// уровень наложение на шаре. Канва приносит туда одно своё: уровень под
/// масштаб камеры и ячейки, накрывающие видимый прямоугольник.
///
/// Отвечает оно обоим — и запросу тайлов, и сборке кадра: рисовать не то, что
/// просили, значит либо просить невидимое, либо не показывать добытое.
pub fn wanted(view: &View, store: &Store, cap: u64) -> Option<tiles::Want> {
    let (target, camera, meta) = parts(view)?;
    let rect = camera.visible((meta.width, meta.height), (target.width, target.height));
    let empty = rect.0 >= rect.2 || rect.1 >= rect.3;

    Some(tiles::want(
        camera.level(meta.levels),
        meta.levels,
        meta.finest,
        cap,
        meta.reach,
        store,
        &view.fetch,
        &meta.fingerprint,
        |level| match empty {
            true => Vec::new(),
            false => {
                let (xs, ys) = cell_range(rect, level, meta);
                ys.flat_map(|y| xs.clone().map(move |x| (level, x, y))).collect()
            }
        },
    ))
}

/// Квады кадра: по одному на видимую ячейку, у которой нашёлся носитель —
/// точный тайл либо ближайший имеющийся предок (`Store::carrier`, общий с
/// наложениями). Обращения продлевают тайлам жизнь в бюджете хранилища.
pub fn quads(view: &View, store: &mut Store, cap: u64, phase: f32) -> Vec<Quad> {
    let Some((target, camera, meta)) = parts(view) else { return Vec::new() };
    let Some(want) = wanted(view, store, cap) else { return Vec::new() };

    // Сила ряби — доля внутри нынешней ступени, считанная тем же правилом, что
    // у шара: рябит ступень, а не отдельная ячейка, и о конкретной ячейке
    // известен ровно один бит — едет она или нет.
    let inside = tiles::inside(
        view.fetch.ordered(),
        (view.read_bytes, view.total_bytes),
        tiles::pointwise(meta, want.level),
    );
    let strength = ripple_strength(inside.share as f32);

    let mut quads = Vec::with_capacity(want.cells.len());
    for cell in want.cells {
        let Some((addr, stored)) = store.carrier(&meta.fingerprint, cell, meta.levels) else {
            continue;
        };
        let (bind, tex) = (stored.bind.clone(), (stored.width, stored.height));
        let rect = pyramid::cell_image_rect(cell.1, cell.2, want.level, meta.width, meta.height);
        quads.push(Quad {
            rect: to_ndc(rect, camera, target),
            uv: pyramid::cell_uv(rect, addr, tex.0, tex.1),
            bind,
            // Накрытая своим тайлом ячейка не рябит, даже когда её ждут: свой
            // тайл с ожидания снимается приездом.
            glow: match addr != cell && view.fetch.coming(cell) {
                true => glow_of(cell, phase, strength),
                false => 0.0,
            },
        });
    }
    quads
}

/// Период волны ряби в секундах и глубина её подмеса — те же, что у шара
/// (`module::RIPPLE_PERIOD_S`, `RIPPLE_DEPTH` в globe.wgsl): работа одна и та
/// же, и разная её скорость на двух экранах читалась бы как разные события.
pub const RIPPLE_PERIOD_S: f32 = 1.6;
const RIPPLE_DEPTH: f32 = 0.30;

/// Сколько разных сдвигов по фазе раздаётся ячейкам — столько же, сколько у
/// шара: соседи заметно расходятся, а рябь не рассыпается в шум.
const RIPPLE_OFFSETS: u32 = 16;

/// Насколько заметна рябь на ступени, дошедшей до такой доли.
///
/// Не самой долей: только начатая ступень идёт ничуть не меньше той, что
/// вот-вот кончится, и невидимая рябь в её начале означала бы «ничего не
/// происходит» ровно там, где ждать дольше всего.
fn ripple_strength(within: f32) -> f32 {
    (0.35 + 0.65 * within.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// Готовая сила подмеса для ячейки: волна, взятая в её сдвиге по фазе.
///
/// Сдвиг выводится из адреса, а не из порядка обхода: порядок ячеек меняется на
/// каждом движении камеры, и волна перескакивала бы с места на место при
/// неподвижной картинке.
fn glow_of(cell: tiles::Addr, phase: f32, strength: f32) -> f32 {
    let (_, x, y) = cell;
    let slot = (x.wrapping_mul(7).wrapping_add(y.wrapping_mul(11))) % RIPPLE_OFFSETS;
    let offset = (slot as f32 + 0.5) / RIPPLE_OFFSETS as f32;
    let wave = 0.5 + 0.5 * (std::f32::consts::TAU * (phase + offset)).sin();
    RIPPLE_DEPTH * strength * wave
}

fn parts(view: &View) -> Option<(&Target, &Camera, &Meta)> {
    Some((view.target.as_ref()?, view.camera.as_ref()?, view.meta()?))
}

/// Диапазоны ячеек уровня, накрытые прямоугольником снимка. Сама арифметика
/// ячейки (прямоугольник, носитель, UV) — в общем pyramid.rs: ею же считает
/// глобус, и правила у них расходиться не могут.
fn cell_range(
    rect: (f64, f64, f64, f64),
    level: u32,
    meta: &Meta,
) -> (std::ops::Range<u32>, std::ops::Range<u32>) {
    let cell = f64::from(TILE) * pyramid::level_px(level);
    let grid_w = pyramid::grid(pyramid::level_size(meta.width, level));
    let grid_h = pyramid::grid(pyramid::level_size(meta.height, level));
    let x0 = ((rect.0 / cell).floor() as u32).min(grid_w);
    let x1 = ((rect.2 / cell).ceil() as u32).min(grid_w);
    let y0 = ((rect.1 / cell).floor() as u32).min(grid_h);
    let y1 = ((rect.3 / cell).ceil() as u32).min(grid_h);
    (x0..x1, y0..y1)
}

/// Пиксели снимка → NDC кадра. Y переворачивается: у снимка вниз, у NDC вверх.
fn to_ndc(cell: [f64; 4], camera: &Camera, target: &Target) -> [f32; 4] {
    let (w, h) = (f64::from(target.width), f64::from(target.height));
    let sx = |ix: f64| (ix - camera.cx) * camera.scale + w / 2.0;
    let sy = |iy: f64| (iy - camera.cy) * camera.scale + h / 2.0;
    [
        (sx(cell[0]) / w * 2.0 - 1.0) as f32,
        (1.0 - sy(cell[1]) / h * 2.0) as f32,
        (sx(cell[2]) / w * 2.0 - 1.0) as f32,
        (1.0 - sy(cell[3]) / h * 2.0) as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(width: u32, height: u32) -> Meta {
        Meta {
            fingerprint: "t".into(),
            width,
            height,
            levels: pyramid::level_count(width, height),
            reach: crate::proto::image_tiler::Reach::Exact,
            finest: 0,
            windowed: pyramid::level_count(width, height),
        }
    }

    /// Показ с пределом детали: снимок «отдаётся» вчетверо мельче своего, и
    /// сказать об этом обязан кто-то — сам по себе он выглядит исправным.
    /// Слово это уезжает единственным полем жалобы, и складывает поводы
    /// [`View::said`].
    /// Стенд неквадратный и нечётный: подпись называет обе стороны, и
    /// квадратный не отличил бы ширину от высоты.
    fn shown_with(finest: u32) -> View {
        let mut view = View::new("снимок".into());
        view.shown = Some(Shown {
            resource: veldsdk::OwnedResource::from_raw_id(1),
            meta: Some(Meta { finest, ..meta(4001, 3001) }),
        });
        view
    }

    /// Строка сверяется целиком: числа в ней стоя́т в своих ролях, и
    /// перевёрнутая фраза содержит ровно те же два числа.
    #[test]
    fn a_capped_source_says_so() {
        assert_eq!(shown_with(0).said(true), "", "предела нет, а канва на что-то жалуется");
        assert_eq!(shown_with(1).said(true), "подробнее 2001×1501 из 4001×3001 не будет");
    }

    /// Пока канва не осела — предел молчит. Заказчик показывает жалобу
    /// **вместо** хода добычи, а у JP2 обе ступени лестницы это по полному
    /// чтению файла: «подробнее не будет» над пустым местом читается как
    /// «загрузка кончилась, вот её потолок». Наложение на шаре молчит здесь по
    /// той же причине, и правило у них общее.
    #[test]
    fn the_cap_keeps_quiet_until_the_view_settles() {
        assert_eq!(shown_with(1).said(false), "", "предел объявлен над пустой канвой");
        assert!(!shown_with(1).said(true).is_empty(), "осела, а предел так и не назван");
    }

    /// Случившееся главнее предела, а застрявший кадр главнее всего: неполный
    /// хотя бы рисуется. И главное — предел переживает приехавший тайл,
    /// который снимает `trouble`: лёг бы он в то же поле, мигнул бы и пропал
    /// ровно тогда, когда картинку начали разглядывать.
    #[test]
    fn the_detail_cap_outlives_a_landed_tile() {
        let mut view = shown_with(1);
        view.trouble = Some("неполно: сорвался проход".into());
        assert_eq!(view.said(true), "неполно: сорвался проход");

        view.stuck = Some("кадр застыл".into());
        assert_eq!(view.said(true), "кадр застыл", "застрявший кадр уступил неполноте");

        view.stuck = None;
        view.landed();
        assert_eq!(
            view.said(true),
            "подробнее 2001×1501 из 4001×3001 не будет",
            "предел детали снят приехавшим тайлом"
        );
    }

    #[test]
    fn cell_range_covers_visible_rect_exactly() {
        let meta = meta(10980, 10980);
        // Весь снимок на уровне 5 (344 px) — одна ячейка.
        let (xs, ys) = cell_range((0.0, 0.0, 10980.0, 10980.0), 5, &meta);
        assert_eq!((xs, ys), (0..1, 0..1));
        // Кусок нулевого уровня: пиксели 500..1300 → ячейки 0..3.
        let (xs, _) = cell_range((500.0, 0.0, 1300.0, 1.0), 0, &meta);
        assert_eq!(xs, 0..3);
    }

    /// Рябь бежит по ячейке волной, а не мигает: у соседей свой сдвиг по фазе,
    /// и привязан он к адресу, а не к порядку обхода — порядок меняется на
    /// каждом движении камеры, и волна перескакивала бы при неподвижной
    /// картинке.
    #[test]
    fn волна_ряби_привязана_к_ячейке_а_не_к_порядку() {
        let strength = ripple_strength(1.0);
        let at = |cell, phase| glow_of(cell, phase, strength);

        assert_eq!(at((0, 3, 4), 0.25), at((0, 3, 4), 0.25), "тот же адрес и та же фаза");
        assert_ne!(at((0, 3, 4), 0.25), at((0, 4, 4), 0.25), "сосед по строке светится в такт");
        assert_ne!(at((0, 3, 4), 0.25), at((0, 3, 5), 0.25), "сосед по столбцу светится в такт");
        assert_ne!(at((0, 3, 4), 0.0), at((0, 3, 4), 0.5), "волна не идёт");

        // Подмес не перебивает картинку: под рябью надо видеть снимок.
        for step in 0..16 {
            let glow = at((0, 1, 1), step as f32 / 16.0);
            assert!((0.0..=0.31).contains(&glow), "подмес {glow} вне меры");
        }
    }

    /// Сила растёт к концу ступени, но и в самом её начале рябь видна: только
    /// начатая ступень идёт ничуть не меньше той, что вот-вот кончится.
    #[test]
    fn рябь_видна_и_в_начале_ступени() {
        assert!(ripple_strength(0.0) > 0.2, "начало ступени не видно вовсе");
        assert!(ripple_strength(1.0) > ripple_strength(0.0), "конец не заметнее начала");
        assert!(ripple_strength(-1.0) >= 0.0 && ripple_strength(7.0) <= 1.0);
    }
}
