//! Наложения: снимки, натянутые на поверхность тайлами пирамиды.
//!
//! Наложение — это привязка и до двух растров по ролям: превью даёт картинку
//! сразу одним мелким файлом, к подробному конвейер идёт на приближении. Тайлы
//! добываются тем же танцем, что у канвы превью: describe → query кэшу →
//! produce промахов, — а рисуются патчами варп-сетки: узлы каждой ячейки
//! переводятся привязкой в градусы и дальше геодезией в мир, GPU интерполирует
//! между ними. Растр при этом не ресемплится вовсе — искажение проекции берёт
//! на себя сетка.
//!
//! Привязок три, и старшинство у них не равное: решётка опорных точек из самого
//! растра, рамка UTM от того, кто знает раскладку продукта, и — последним —
//! четырёхугольник футпринта из каталога. Последний отвечает на другой вопрос
//! (какой кусок Земли снят, а не каким пикселем куда) и потому остаётся
//! догадкой: пока описания растров не кончились, наложение на нём не рисуется
//! и тайлов не просит (см. [`Overlay::binding_pending`]).
//!
//! Ячейка рисуется ровно одним носителем — точным тайлом или куском ближайшего
//! предка (parent-fallback, та же арифметика pyramid.rs, что у канвы), поэтому
//! перекрытий внутри наложения нет.

use veldmap_image_tiler_wrap::pyramid;
use veldmap_image_tiler_wrap::tiles::{self, Addr, Fetch, Store};
use veldsdk::graphics::BindGroupId;

use super::camera;
use super::geodesy::{self, Geodetic, World};
use super::gpu::OverlayVertex;
use super::projection;

/// Высота наложения над поверхностью: над телом Земли (его глубина решает
/// заслонение дальней стороной), но под сеткой и контурами (2 км) — линии
/// обязаны читаться поверх снимка.
pub const HEIGHT_M: f64 = 1_000.0;

/// На сколько сегментов бьётся сторона ячейки варп-сетки. Восьми хватает:
/// даже у ячейки во всю гранулу (110 км) сегмент — 14 км, а хорда такой длины
/// проседает под поверхность на считаные метры против километра высоты.
const PATCH_SEGMENTS: u32 = 8;

/// Роль растра — те же две, что в types.proto.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Preview,
    Detailed,
}

/// Узел сетки привязки: доля растра (0..1) и место на Земле. Пиксели в доли
/// переводятся при сборке сетки — растров у наложения два, разного размера, а
/// лежат они одинаково.
#[derive(Clone, Copy)]
pub struct Tie {
    pub fx: f64,
    pub fy: f64,
    pub lat: f64,
    pub lon: f64,
}

/// Привязка растра к Земле.
pub enum Frame {
    /// Точная рамка в метрах зоны UTM; y1 — северный край.
    Utm { zone: projection::Zone, x0: f64, y0: f64, x1: f64, y1: f64 },
    /// Четырёхугольник футпринта по обходу растра: UL, UR, LR, LL. Долготы
    /// развёрнуты в непрерывную ветвь заранее (см. [`Frame::quad`]).
    ///
    /// Догадка, а не привязка: каталог говорит, какой кусок Земли снят, но не
    /// тем, каким пикселем куда, — порядок вершин у него свой. Годится, пока
    /// от растра не приехала сетка (см. [`Frame::Grid`]).
    Quad([(f64, f64); 4]),
    /// Решётка опорных точек из самого растра — единственная привязка, которую
    /// никто не выдумывал.
    Grid(Grid),
}

impl Frame {
    /// Квад из вершин по обходу растра. Долготы разворачиваются к первой
    /// вершине: футпринт через антимеридиан иначе интерполировался бы через
    /// всю Землю.
    pub fn quad(points: [(f64, f64); 4]) -> Self {
        let base = points[0].1;
        Self::Quad(points.map(|(lat, lon)| (lat, geodesy::unwind(base, lon))))
    }

    /// Доля растра (0..1 слева направо и сверху вниз) → широта и долгота.
    pub fn geodetic(&self, fx: f64, fy: f64) -> (f64, f64) {
        match self {
            Self::Utm { zone, x0, y0, x1, y1 } => {
                let x = x0 + fx * (x1 - x0);
                let y = y1 - fy * (y1 - y0);
                projection::to_geodetic(*zone, x, y)
            }
            // Между углами футпринта нет ничего, кроме поверхности, — значит и
            // идти между ними надо по ней, дугой: у гранулы Sentinel-1 прямая в
            // градусах отходит от снятого на 27 км (см. `geodesy::between`).
            Self::Quad([ul, ur, lr, ll]) => {
                let top = geodesy::between(*ul, *ur, fx);
                let bottom = geodesy::between(*ll, *lr, fx);
                geodesy::between(top, bottom, fy)
            }
            Self::Grid(grid) => grid.geodetic(fx, fy),
        }
    }

    /// Метров земли на пиксель растра шириной `width`. У квада и сетки — по
    /// верхнему ребру и грубой метрике градусов: спрашивают это, чтобы выбрать
    /// уровень пирамиды, а он меняется вдвое за раз.
    pub fn ground_m_per_px(&self, width: u32) -> f64 {
        let width = f64::from(width.max(1));
        match self {
            Self::Utm { x0, x1, .. } => (x1 - x0) / width,
            Self::Quad([ul, ur, ..]) => ground_span(*ul, *ur) / width,
            Self::Grid(grid) => grid.top_span() / width,
        }
    }
}

/// Решётка опорных точек: узлы стоят по сетке долей растра, между ними
/// показ интерполирует.
///
/// Сеткой, а не четырьмя углами: снимок радара лежит в геометрии съёмки, и
/// четырёхугольник её не описывает — кривизна набегает километрами. Файл несёт
/// решётку (у гранулы Sentinel-1 — 21×21), и она же отвечает на вопрос, каким
/// углом растр повёрнут: у квада футпринта порядок вершин свой, и совпадать с
/// обходом растра он не обязан.
pub struct Grid {
    /// Доли растра, на которых стоят узлы, по осям — возрастающие.
    xs: Vec<f64>,
    ys: Vec<f64>,
    /// Узлы построчно (ys × xs): широта и долгота. Долготы развёрнуты в одну
    /// непрерывную ветвь — иначе снимок через антимеридиан растянуло бы через
    /// всю Землю (то же правило, что у квада).
    nodes: Vec<(f64, f64)>,
}

impl Grid {
    /// Решётка из опорных точек. `None` — точки решётки не образуют: их меньше
    /// четырёх, они не полны или стоят вразброс. Догадываться тут не о чем —
    /// привязка остаётся прежней.
    pub fn new(ties: &[Tie]) -> Option<Self> {
        let axis = |values: Vec<f64>| {
            let mut values = values;
            values.sort_by(f64::total_cmp);
            values.dedup();
            values
        };
        let xs = axis(ties.iter().map(|tie| tie.fx).collect());
        let ys = axis(ties.iter().map(|tie| tie.fy).collect());
        if xs.len() < 2 || ys.len() < 2 || xs.len() * ys.len() != ties.len() {
            return None;
        }

        let base = ties[0].lon;
        let mut nodes = vec![None; ties.len()];
        for tie in ties {
            let col = xs.binary_search_by(|at| at.total_cmp(&tie.fx)).ok()?;
            let row = ys.binary_search_by(|at| at.total_cmp(&tie.fy)).ok()?;
            nodes[row * xs.len() + col] = Some((tie.lat, geodesy::unwind(base, tie.lon)));
        }
        let nodes: Option<Vec<(f64, f64)>> = nodes.into_iter().collect();
        Some(Self { xs, ys, nodes: nodes? })
    }

    fn geodetic(&self, fx: f64, fy: f64) -> (f64, f64) {
        let (col, tx) = cell(&self.xs, fx);
        let (row, ty) = cell(&self.ys, fy);
        let node = |row: usize, col: usize| self.nodes[row * self.xs.len() + col];
        let top = lerp(node(row, col), node(row, col + 1), tx);
        let bottom = lerp(node(row + 1, col), node(row + 1, col + 1), tx);
        lerp(top, bottom, ty)
    }

    /// Сколько земли приходится на всю ширину растра, метры: длина верхнего
    /// ребра решётки, растянутая с её доли на единицу.
    fn top_span(&self) -> f64 {
        ground_span(self.nodes[0], self.nodes[self.xs.len() - 1])
            / (self.xs[self.xs.len() - 1] - self.xs[0]).max(f64::EPSILON)
    }
}

/// Ячейка оси и доля внутри неё. За краем решётки доля выходит за [0, 1]:
/// крайние узлы стоят в центрах крайних пикселей, а показывать надо и половину
/// пикселя за ними — там сетка продолжается прямой.
fn cell(axis: &[f64], at: f64) -> (usize, f64) {
    let last = axis.len() - 2;
    let i = axis.partition_point(|edge| *edge <= at).saturating_sub(1).min(last);
    (i, (at - axis[i]) / (axis[i + 1] - axis[i]))
}

/// Линейно между узлами решётки — там, где идти по дуге незачем: узлы стоят в
/// двух десятках километров друг от друга, и дуга с хордой расходятся на метры
/// (у квада, где между углами четыре сотни километров, всё иначе).
fn lerp(a: (f64, f64), b: (f64, f64), t: f64) -> (f64, f64) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// Расстояние между точками по поверхности, метры — по шару с большой
/// полуосью: сжатие даёт треть процента, а спрашивают это ради выбора уровня
/// пирамиды, который меняется вдвое за раз.
fn ground_span(from: (f64, f64), to: (f64, f64)) -> f64 {
    geodesy::separation(from, to).to_radians() * geodesy::SEMI_MAJOR_M
}

/// Описанный растр — то же, что у канвы.
pub struct Meta {
    pub fingerprint: String,
    pub width: u32,
    pub height: u32,
    pub levels: u32,
}

/// Один растр наложения и все его ожидания.
pub struct Raster {
    pub role: Role,
    pub resource: veldsdk::OwnedResource,
    pub meta: Option<Meta>,
    pub describe: veldsdk::Latest,
    /// Что уже спрошено, что производится и чего больше не просить — общий
    /// учёт потребителя тайлов (см. `veldmap_image_tiler_wrap::tiles`).
    pub fetch: Fetch,
}

impl Raster {
    pub fn new(role: Role, resource: veldsdk::OwnedResource) -> Self {
        Self {
            role,
            resource,
            meta: None,
            describe: veldsdk::Latest::default(),
            fetch: Fetch::default(),
        }
    }
}

pub struct Overlay {
    pub key: String,
    pub label: String,
    pub frame: Frame,
    pub rasters: Vec<Raster>,
    /// Ресурсы, которыми наложение прислали, — в том виде, в каком они пришли.
    /// По ним и решается «то же самое наложение или другое»: сравнивать с
    /// принятыми нельзя, потому что принято бывает меньше присланного (растр,
    /// которому отказали в гранте, пропускается), и тогда набор расходился бы
    /// сам с собой навсегда — каждая пересылка выглядела бы новым наложением.
    pub sources: Vec<u64>,
    /// Прозрачность слоя, 0..1. Доезжает до пикселя множителем к альфе
    /// носителя, поэтому прозрачное поле квиклука так и остаётся прозрачным,
    /// а не проступает вполсилы.
    pub opacity: f32,
    /// Слой скрыт: ни патчей, ни запросов тайлов. Уже добытые тайлы остаются в
    /// хранилище на попечение вытеснения — показ обратно тогда мгновенный, а
    /// платить за них памятью приходится только пока их не вытеснили.
    pub hidden: bool,
}

/// Что рисовать у наложения прямо сейчас: какой растр и каким уровнем.
#[derive(Clone, PartialEq)]
pub struct Choice {
    pub role: Role,
    pub fingerprint: String,
    pub level: u32,
}

/// Он же вместе с тем, что из него видно: ячейки выбранного уровня, попавшие в
/// кадр. Одним значением, потому что считаются они вместе и порознь не имеют
/// смысла — уровень выбирается как раз по тому, сколько его ячеек видно.
pub struct Wanted {
    pub choice: Choice,
    pub cells: Vec<Addr>,
}

/// Взгляд, которым меряется желаемое: чем проецировать, откуда смотрят и какой
/// кусок земли приходится на пиксель кадра. Одной штукой — спрашивают их всегда
/// вместе и всегда об одном кадре.
pub struct Look {
    pub view_proj: camera::Mat4,
    pub eye: World,
    pub mpp: f64,
}

impl Overlay {
    pub fn raster_mut(&mut self, role: Role) -> Option<&mut Raster> {
        self.rasters.iter_mut().find(|raster| raster.role == role)
    }

    pub fn raster(&self, role: Role) -> Option<&Raster> {
        self.rasters.iter().find(|raster| raster.role == role)
    }

    /// Наложению ещё есть чего ждать: описание в пути или по растру идёт проход
    /// производителя. «Добыто всё, что просили» этого не заменяет: за добытой
    /// ступенью идёт следующая, и работа на ней не кончилась.
    pub fn busy(&self) -> bool {
        self.rasters
            .iter()
            .any(|raster| raster.describe.is_pending() || raster.fetch.waiting())
    }

    /// Привязка ещё может смениться, и потому наложение пока ни рисуют, ни
    /// просят под него тайлов.
    ///
    /// Квад футпринта — догадка о том, каким углом растр лёг на снятый кусок
    /// Земли, а описание растра может принести сетку из самого файла. Нарисовать
    /// по догадке значит показать снимок повёрнутым и через секунду повернуть
    /// его на глазах; спросить по ней тайлы — спросить не те ячейки: видимость
    /// считается той же привязкой. Рамка UTM — не догадка, и ждать ей нечего.
    fn binding_pending(&self) -> bool {
        matches!(self.frame, Frame::Quad(_))
            && self.rasters.iter().any(|raster| raster.describe.is_pending())
    }

    /// Что рисовать и чего хотеть под текущий взгляд: превью-база и, когда
    /// экран мельче её родного разрешения, подробный растр. Порядок и есть
    /// порядок отрисовки: база всегда внизу, цель поверх — так снимок виден
    /// квиклуком сразу, а подробные тайлы накрывают его по мере прихода
    /// (дешёвый старт из дизайна фазы).
    ///
    /// Ответ один на обоих спрашивающих — на запрос тайлов и на сборку
    /// патчей: заказанный уровень обязан быть тем же, который рисуют, а
    /// видимость ячейки — тем же, по чему её просили.
    pub fn wanted(&self, look: &Look, cap_tiles: u64, store: &Store) -> Vec<Wanted> {
        if self.binding_pending() {
            return Vec::new();
        }
        let mut wanted = Vec::new();
        let described =
            |role: Role| self.raster(role).and_then(|raster| raster.meta.as_ref());

        if let Some(meta) = described(Role::Preview) {
            wanted.push(self.at_level(Role::Preview, meta, look, cap_tiles, store));
            if look.mpp >= self.frame.ground_m_per_px(meta.width) {
                // Родного разрешения превью хватает — подробный не нужен.
                return wanted;
            }
        }
        if let Some(meta) = described(Role::Detailed) {
            wanted.push(self.at_level(Role::Detailed, meta, look, cap_tiles, store));
        }
        wanted
    }

    /// Ступень добычи под взгляд и видимые ячейки этой ступени.
    ///
    /// Целевой уровень — ближайший, чей пиксель не крупнее экранного; дальше он
    /// грубеет, пока видимого не станет меньше потолка. Потолок считается по
    /// видимому, а не по уровню целиком, и в этом вся разница: гранула
    /// 10000×10000 целым нулевым уровнем не помещается ни в какой бюджет, и мерь
    /// мы им — снимок не показал бы подробностей ни на каком приближении. Видно
    /// же от силы десяток ячеек, сколько ни приближай.
    ///
    /// Спрашивается при этом не сразу целевой, а самый грубый уровень, которого
    /// ещё не хватает: вершина пирамиды — это один тайл из самой мелкой копии
    /// файла, он приезжает за секунду и накрывает снимок целиком, а целевой из
    /// десятка тайлов по мегабайту едет полминуты, и всё это время на шаре была
    /// бы пустота. Каждая ступень к тому же становится предком для следующей
    /// (parent-fallback в [`patches`]), так что дыр между ними не бывает.
    fn at_level(
        &self,
        role: Role,
        meta: &Meta,
        look: &Look,
        cap_tiles: u64,
        store: &Store,
    ) -> Wanted {
        let (target, cells) = self.target_level(meta, look, cap_tiles);
        let fetch = self.raster(role).map(|raster| &raster.fetch);
        let (level, cells) = tiles::rung(
            target,
            meta.levels - 1,
            |level| match level == target {
                // Целевой уже посчитан потолком выше — считать его второй раз
                // значит второй раз проецировать все его ячейки.
                true => cells.clone(),
                false => self.visible(meta, level, look),
            },
            |addr| {
                store.contains(&meta.fingerprint, addr)
                    || fetch.is_some_and(|fetch| fetch.hopeless(addr))
            },
        );
        Wanted { choice: Choice { role, fingerprint: meta.fingerprint.clone(), level }, cells }
    }

    /// Уровень, к которому идут, и его видимые ячейки: ближайший, чей пиксель
    /// не крупнее экранного, загрублённый до потолка аппетита.
    fn target_level(&self, meta: &Meta, look: &Look, cap_tiles: u64) -> (u32, Vec<Addr>) {
        let mpp_raster = self.frame.ground_m_per_px(meta.width);
        let mut level = if look.mpp <= mpp_raster {
            0
        } else {
            (look.mpp / mpp_raster).log2().floor() as u32
        }
        .min(meta.levels - 1);

        let mut cells = self.visible(meta, level, look);
        while cells.len() as u64 > cap_tiles && level + 1 < meta.levels {
            level += 1;
            cells = self.visible(meta, level, look);
        }
        (level, cells)
    }

    /// Ячейки уровня, попавшие в кадр.
    fn visible(&self, meta: &Meta, level: u32, look: &Look) -> Vec<Addr> {
        let grid_w = pyramid::grid(pyramid::level_size(meta.width, level));
        let grid_h = pyramid::grid(pyramid::level_size(meta.height, level));
        let mut cells = Vec::new();
        for y in 0..grid_h {
            for x in 0..grid_w {
                if self.on_screen(meta, level, x, y, look) {
                    cells.push((level, x, y));
                }
            }
        }
        cells
    }

    /// Видна ли ячейка: её углы переводятся привязкой в точки мира и
    /// проецируются той же матрицей, которой рисуется кадр. Дальняя сторона
    /// Земли отсеивается сама — её углы отвёрнуты от глаза.
    ///
    /// Считается по углам, но решает не попадание угла в кадр, а пересечение:
    /// ячейка бывает крупнее экрана целиком, и тогда в кадре нет ни одного её
    /// угла, а видно только её.
    fn on_screen(&self, meta: &Meta, level: u32, x: u32, y: u32, look: &Look) -> bool {
        let cell = pyramid::cell_image_rect(x, y, level, meta.width, meta.height);
        let corners = [
            (cell[0], cell[1]),
            (cell[2], cell[1]),
            (cell[2], cell[3]),
            (cell[0], cell[3]),
        ];

        let (mut low, mut high) = ([f32::MAX; 2], [f32::MIN; 2]);
        let mut faces = false;
        for (px, py) in corners {
            let (lat, lon) = self
                .frame
                .geodetic(px / f64::from(meta.width), py / f64::from(meta.height));
            let point = geodesy::position(Geodetic { lat_deg: lat, lon_deg: lon, height_m: HEIGHT_M });
            faces |= faces_eye(point, look.eye);

            let clip = camera::project(&look.view_proj, point);
            // Угол за камерой: делить на такое w нельзя, а ячейка при этом
            // заведомо близко — считаем её видимой и не гадаем.
            if clip[3] <= 0.0 {
                return faces || faces_eye(point, look.eye);
            }
            for axis in 0..2 {
                let ndc = clip[axis] / clip[3];
                low[axis] = low[axis].min(ndc);
                high[axis] = high[axis].max(ndc);
            }
        }

        faces && low[0] <= 1.0 && high[0] >= -1.0 && low[1] <= 1.0 && high[1] >= -1.0
    }
}

/// Обращена ли точка поверхности к глазу. Считается по шару, а не по
/// эллипсоиду: сжатие даёт доли процента, а вопрос грубый — по эту сторону
/// горизонта точка или по ту.
fn faces_eye(point: World, eye: World) -> bool {
    let to_eye = [eye[0] - point[0], eye[1] - point[1], eye[2] - point[2]];
    point[0] * to_eye[0] + point[1] * to_eye[1] + point[2] * to_eye[2] > 0.0
}

/// Патчи наложения: по одному на видимую ячейку, у которой нашёлся носитель.
/// Вершины пишутся в общий буфер, отрисовки — диапазонами по носителю.
/// Обращения продлевают тайлам жизнь в бюджете.
///
/// Ячейки приходят те же самые, что уехали в запрос тайлов (см.
/// [`Overlay::wanted`]): рисовать не то, что просили, значит либо просить
/// невидимое, либо не показывать добытое.
pub fn patches(
    overlay: &Overlay,
    wanted: &Wanted,
    store: &mut Store,
    vertices: &mut Vec<OverlayVertex>,
    draws: &mut Vec<(BindGroupId, std::ops::Range<u32>)>,
) {
    let Some(meta) = overlay.raster(wanted.choice.role).and_then(|raster| raster.meta.as_ref())
    else {
        return;
    };
    let level = wanted.choice.level;

    for &(_, x, y) in &wanted.cells {
        // Ближайший имеющийся предок, начиная с точного тайла.
        for d in 0..=(meta.levels - 1 - level) {
            let addr = (level + d, x >> d, y >> d);
            let Some(stored) = store.touch(&meta.fingerprint, addr) else { continue };
            let cell = pyramid::cell_image_rect(x, y, level, meta.width, meta.height);
            let uv = pyramid::cell_uv(cell, addr, stored.width, stored.height);
            let bind = stored.bind.clone();
            let from = vertices.len() as u32;
            patch(&overlay.frame, meta, cell, uv, overlay.opacity, vertices);
            draws.push((bind, from..vertices.len() as u32));
            break;
        }
    }
}

/// Варп-сетка одной ячейки: PATCH_SEGMENTS² квадов, узлы — точной проекцией
/// привязки, UV — линейно по куску носителя.
///
/// Прозрачность едет вершиной, а не uniform'ом: все патчи наложений лежат в
/// одном буфере и рисуются одним пайплайном, отличаясь только диапазоном
/// вершин и bind group носителя, — своего uniform'а у слоя тут нет и не за что
/// его завести.
fn patch(
    frame: &Frame,
    meta: &Meta,
    cell: [f64; 4],
    uv: [f32; 4],
    alpha: f32,
    vertices: &mut Vec<OverlayVertex>,
) {
    let segments = PATCH_SEGMENTS;
    // Узлы решётки считаются один раз: у соседних квадов они общие, а проекция
    // — самая дорогая часть узла.
    let side = (segments + 1) as usize;
    let mut nodes = Vec::with_capacity(side * side);
    for row in 0..=segments {
        let ty = f64::from(row) / f64::from(segments);
        for col in 0..=segments {
            let tx = f64::from(col) / f64::from(segments);
            let fx = (cell[0] + (cell[2] - cell[0]) * tx) / f64::from(meta.width);
            let fy = (cell[1] + (cell[3] - cell[1]) * ty) / f64::from(meta.height);
            let (lat, lon) = frame.geodetic(fx, fy);
            let position =
                geodesy::position(Geodetic { lat_deg: lat, lon_deg: lon, height_m: HEIGHT_M });
            let u = uv[0] + (uv[2] - uv[0]) * tx as f32;
            let v = uv[1] + (uv[3] - uv[1]) * ty as f32;
            nodes.push(OverlayVertex { position, uv: [u, v], alpha });
        }
    }

    for row in 0..segments as usize {
        for col in 0..segments as usize {
            let a = nodes[row * side + col];
            let b = nodes[row * side + col + 1];
            let c = nodes[(row + 1) * side + col];
            let d = nodes[(row + 1) * side + col + 1];
            vertices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(width: u32, height: u32) -> Meta {
        Meta {
            fingerprint: "fp".into(),
            width,
            height,
            levels: pyramid::level_count(width, height),
        }
    }

    /// Пустое хранилище: в тестах тайлам взяться неоткуда — текстуры выдаёт
    /// хост, — и проверяется поэтому первая ступень, с которой всё начинается.
    /// Спуск по ступеням проверен там, где он и написан (`tiles::rung`).
    fn store() -> Store {
        Store::new(1 << 30)
    }

    fn overlay(rasters: Vec<Raster>) -> Overlay {
        Overlay {
            key: "k".into(),
            label: "снимок".into(),
            // Рамка T40WFC: 109.8 км на всю ширину растра.
            frame: Frame::Utm {
                zone: projection::Zone { number: 40, south: false },
                x0: 600_000.0,
                y0: 7_690_200.0,
                x1: 709_800.0,
                y1: 7_800_000.0,
            },
            rasters,
            sources: Vec::new(),
            opacity: 1.0,
            hidden: false,
        }
    }

    fn raster(role: Role, meta_of: Option<Meta>) -> Raster {
        let mut raster =
            Raster::new(role, veldsdk::OwnedResource::from_raw_id(u64::from(role as u32) + 1));
        raster.meta = meta_of;
        raster
    }

    /// Взгляд на снимок в упор: камера наведена на середину рамки, так что
    /// видно её целиком. Метраж экранного пикселя задаётся отдельно от камеры —
    /// проверяется выбор уровня, а не арифметика высоты.
    fn look_at(overlay: &Overlay, mpp: f64) -> Look {
        let (lat, lon) = overlay.frame.geodetic(0.5, 0.5);
        let mut camera = crate::module::camera::Camera::default();
        camera.focus(lat, lon, 1.0);
        Look { view_proj: camera.view_projection(1.0), eye: camera.eye(), mpp }
    }

    /// Выбор растров: далеко хватает превью одного; ближе его родного
    /// разрешения подробный добавляется ПОВЕРХ превью — база рисуется всегда,
    /// чтобы снимок был виден, пока подробные тайлы едут.
    #[test]
    fn choices_keep_preview_under_detail() {
        let overlay = overlay(vec![
            raster(Role::Preview, Some(meta(343, 343))),    // ~320 м/px
            raster(Role::Detailed, Some(meta(10980, 10980))), // 10 м/px
        ]);
        // Далеко: экранный пиксель — километр; превью одно.
        let far = overlay.wanted(&look_at(&overlay, 1000.0), u64::MAX, &store());
        assert_eq!(far.len(), 1);
        assert_eq!(far[0].choice.role, Role::Preview);
        // Экран мельче превью: база остаётся, подробный ложится поверх.
        let near = overlay.wanted(&look_at(&overlay, 40.0), u64::MAX, &store());
        assert_eq!(near.len(), 2);
        assert_eq!(near[0].choice.role, Role::Preview);
        assert_eq!(near[1].choice.role, Role::Detailed);
    }

    /// Целевой уровень — по экранному пикселю: 40 м/px против 10 м/px родных
    /// дают второй, вплотную — нулевой.
    #[test]
    fn target_level_follows_the_screen_pixel() {
        let overlay = overlay(vec![raster(Role::Detailed, Some(meta(10980, 10980)))]);
        let detailed = meta(10980, 10980);
        let level = |mpp| overlay.target_level(&detailed, &look_at(&overlay, mpp), u64::MAX).0;
        assert_eq!(level(40.0), 2);
        assert_eq!(level(5.0), 0);
    }

    /// Потолок аппетита загрубляет уровень — но меряется он видимым, а не
    /// уровнем целиком: снимок виден весь, и 22×22 ячейки нулевого уровня в
    /// потолок из ста не влезают, 11×11 первого — тоже, 6×6 второго — да.
    #[test]
    fn tile_cap_coarsens_level() {
        let overlay = overlay(vec![raster(Role::Detailed, Some(meta(10980, 10980)))]);
        let (level, cells) =
            overlay.target_level(&meta(10980, 10980), &look_at(&overlay, 5.0), 100);
        assert_eq!(level, 2);
        assert_eq!(cells.len(), 36, "видно все ячейки уровня");
    }

    /// Пустое хранилище — и просят вершину пирамиды, а не целевой уровень:
    /// снимок появляется целиком грубым, пока подробные тайлы едут (см.
    /// `tiles::rung`).
    #[test]
    fn empty_store_asks_for_the_top_of_the_pyramid() {
        let overlay = overlay(vec![raster(Role::Detailed, Some(meta(10980, 10980)))]);
        let wanted = overlay.wanted(&look_at(&overlay, 5.0), u64::MAX, &store());
        assert_eq!(wanted[0].choice.level, meta(10980, 10980).levels - 1);
        assert_eq!(wanted[0].cells.len(), 1, "вершина — один тайл");
    }

    /// Из кадра выпавшее не просят: камера, отведённая на другую сторону
    /// Земли, не хочет от этого снимка ни одной ячейки.
    #[test]
    fn hidden_side_wants_nothing() {
        let overlay = overlay(vec![raster(Role::Detailed, Some(meta(10980, 10980)))]);
        let (lat, lon) = overlay.frame.geodetic(0.5, 0.5);
        let mut camera = crate::module::camera::Camera::default();
        camera.focus(-lat, lon + 180.0, 1.0);
        let look = Look { view_proj: camera.view_projection(1.0), eye: camera.eye(), mpp: 5.0 };
        assert!(overlay.wanted(&look, u64::MAX, &store())[0].cells.is_empty());
    }

    /// Без описанных растров выбирать не из чего.
    #[test]
    fn no_meta_no_choices() {
        let overlay = overlay(vec![raster(Role::Preview, None)]);
        assert!(overlay.wanted(&look_at(&overlay, 100.0), u64::MAX, &store()).is_empty());
    }

    fn tie(fx: f64, fy: f64, lat: f64, lon: f64) -> Tie {
        Tie { fx, fy, lat, lon }
    }

    /// Сетка кладёт растр так, как сказано в нём самом, — и это не то же самое,
    /// что обход контура из каталога.
    ///
    /// Числа настоящие: гранула Sentinel-1 нисходящего витка. В файле первый
    /// пиксель — северо-восточный угол (радар смотрит вправо, то есть на запад,
    /// а строки идут с севера); контур того же продукта каталог отдаёт начиная
    /// с юго-восточного. Прочтя контур обходом растра, снимок кладут повёрнутым
    /// на четверть оборота — эта разница здесь и записана.
    #[test]
    fn grid_binds_the_raster_the_way_the_file_says() {
        let (ne, nw) = ((73.395, 2.707), (74.524, -10.409));
        let (sw, se) = ((70.979, -12.618), (70.005, -1.639));
        let grid = Grid::new(&[
            tie(0.0, 0.0, ne.0, ne.1),
            tie(1.0, 0.0, nw.0, nw.1),
            tie(0.0, 1.0, se.0, se.1),
            tie(1.0, 1.0, sw.0, sw.1),
        ])
        .expect("решётка 2×2");
        let frame = Frame::Grid(grid);
        assert_eq!(frame.geodetic(0.0, 0.0), ne, "первый пиксель — где сказал файл");
        assert_eq!(frame.geodetic(1.0, 1.0), sw);

        // Тот же продукт контуром каталога, прочитанным обходом растра: его
        // первая вершина — юго-восточный угол, и снимок оказывается повёрнут.
        let quad = Frame::quad([se, ne, nw, sw]);
        assert_eq!(quad.geodetic(0.0, 0.0), se);
        assert_ne!(quad.geodetic(0.0, 0.0), frame.geodetic(0.0, 0.0));
    }

    /// Между узлами показ идёт по решётке, а не по её углам: середина верхнего
    /// ребра — там, где стоит её узел. Этим сетка и отличается от квада —
    /// кривизну снимка в геометрии съёмки четырьмя углами не передать.
    #[test]
    fn grid_follows_its_nodes_between_the_corners() {
        let grid = Grid::new(&[
            tie(0.0, 0.0, 60.0, 0.0),
            tie(0.5, 0.0, 61.0, 10.0),
            tie(1.0, 0.0, 60.0, 20.0),
            tie(0.0, 1.0, 50.0, 0.0),
            tie(0.5, 1.0, 50.0, 10.0),
            tie(1.0, 1.0, 50.0, 20.0),
        ])
        .expect("решётка 3×2");
        let frame = Frame::Grid(grid);
        // Узел выпирает на градус к северу — квад по тем же углам дал бы 60.
        assert_eq!(frame.geodetic(0.5, 0.0), (61.0, 10.0));
        // Между узлами — билинейно: четверть пути вниз от выпирающего узла.
        let (lat, lon) = frame.geodetic(0.5, 0.25);
        assert!((lat - 58.25).abs() < 1e-12, "{}", lat);
        assert!((lon - 10.0).abs() < 1e-12, "{}", lon);
        // Метр на пиксель — по верхнему ребру целиком, 20° долготы на 100 px.
        let mpp = frame.ground_m_per_px(100);
        assert!((mpp - ground_span((60.0, 0.0), (60.0, 20.0)) / 100.0).abs() < 1e-9, "{}", mpp);
    }

    /// Точки, не образующие решётки, — не привязка: гадать по ним нечего, и
    /// наложение остаётся на прежней.
    #[test]
    fn grid_needs_a_full_lattice() {
        assert!(Grid::new(&[tie(0.0, 0.0, 1.0, 1.0)]).is_none(), "одна точка");
        assert!(
            Grid::new(&[tie(0.0, 0.0, 1.0, 1.0), tie(1.0, 0.0, 1.0, 2.0), tie(0.0, 1.0, 0.0, 1.0)])
                .is_none(),
            "угла не хватает"
        );
    }

    /// Долготы сетки разворачиваются в одну ветвь — тем же правилом, что у
    /// квада: иначе снимок через антимеридиан растянуло бы через всю Землю.
    #[test]
    fn grid_crosses_the_antimeridian_the_short_way() {
        let grid = Grid::new(&[
            tie(0.0, 0.0, 10.0, 179.0),
            tie(1.0, 0.0, 10.0, -179.0),
            tie(0.0, 1.0, 8.0, 179.0),
            tie(1.0, 1.0, 8.0, -179.0),
        ])
        .expect("решётка 2×2");
        let (lat, lon) = Frame::Grid(grid).geodetic(0.5, 0.0);
        assert!((lat - 10.0).abs() < 1e-12);
        assert!((lon - 180.0).abs() < 1e-9, "{}", lon);
    }

    /// Квад разворачивает долготы через антимеридиан в одну ветвь: середина
    /// между 179 и −179 — это 180, а не ноль. Широта середины при этом чуть
    /// севернее краёв — ребро идёт дугой, а не по параллели.
    #[test]
    fn quad_crosses_antimeridian_the_short_way() {
        let frame = Frame::quad([(10.0, 179.0), (10.0, -179.0), (8.0, -179.0), (8.0, 179.0)]);
        let (lat, lon) = frame.geodetic(0.5, 0.0);
        assert!((lon - 180.0).abs() < 1e-9, "{}", lon);
        assert!((lat - 10.0).abs() < 0.01 && lat > 10.0, "{}", lat);
    }

    /// Углы рамки UTM попадают в свои доли растра: (0,0) — северо-западный
    /// угол, (1,1) — юго-восточный, и наоборот через прямую проекцию.
    #[test]
    fn utm_frame_corners_roundtrip() {
        let overlay = overlay(vec![]);
        let (lat_nw, lon_nw) = overlay.frame.geodetic(0.0, 0.0);
        let zone = projection::Zone { number: 40, south: false };
        let (e, n) = projection::from_geodetic(zone, lat_nw, lon_nw);
        assert!((e - 600_000.0).abs() < 1e-6 && (n - 7_800_000.0).abs() < 1e-6);
        let (lat_se, _) = overlay.frame.geodetic(1.0, 1.0);
        assert!(lat_se < lat_nw);
    }
}
