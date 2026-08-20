//! Контуры на поверхности: ломаные из присланных вершин.
//!
//! Вершина обычного контура та же, что у Земли и сетки (`mesh::Vertex`), —
//! точка с нормалью, — поэтому и раскладка вершинного буфера у них общая.
//! Отдельный буфер нужен не формату, а времени жизни: Земля строится раз, а
//! контуры меняются с каждым ответом на поиск.
//!
//! Лентой рисуются два контура из трёх: линия в пайплайне всегда в пиксель
//! шириной, а выделить один контур среди полусотни соседних — или показать
//! место снимка, который ещё едет, — нужно так, чтобы это было видно. Ширина у
//! ленты экранная, а не мировая: в мире она зависела бы от приближения — на
//! отлёте расплывалась бы в пятно, вблизи истончалась в ту же линию. Разводит
//! края поэтому вершинный шейдер (`vs_ribbon` в globe.wgsl), а здесь лежит то,
//! чего он сам знать не может: соседи вершины по обходу и полуширина той
//! ленты, которой этот контур рисуется.

use crate::module::geodesy::{self, Geodetic};
use crate::module::gpu::RibbonVertex;
use crate::module::mesh::{self, Vertex};
use crate::proto::globe::{Outline, OutlineStyle};

/// Высота контуров над поверхностью — та же, что у сетки: они лежат на Земле
/// и не должны с ней спорить за глубину.
const HEIGHT_M: f64 = mesh::GRID_HEIGHT_M;

/// Насколько мелко бить ребро. Ломаная соединяет вершины отрезками в
/// пространстве, а лежать должна на поверхности: чем длиннее ребро, тем
/// глубже отрезок уходит под неё. Градус дуги — это около 111 км, и просадка
/// в середине такого отрезка меньше четверти километра, то есть много меньше
/// высоты, на которой контур висит.
const MAX_EDGE_DEG: f64 = 1.0;

/// Полуширина ленты выделенного контура в пикселях кадра.
const SELECTED_HALF_PX: f32 = 2.5;
/// Полуширина ленты едущего на шар. Тоньше выделенной, и не только ради
/// приличия: выделен снимок один, а едущих бывает несколько, и спорить с
/// выделенным за взгляд им незачем — от обычной линии их отличает штрих
/// (см. `fs_ribbon_pending`).
///
/// Снизу она ограничена шагом штриха, и это не вкус. Штрих кладётся косыми
/// полосами по кадру, поэтому поперёк ленты, идущей ровно вдоль них, набегает
/// её ширина, умноженная на корень из двух. Не перекрой этот отрезок половину
/// шага — лента такого направления уляжется целиком в выброшенную половину и
/// пропадёт с шара. Порог поэтому `2 * PENDING_HALF_PX * sqrt(2) > HATCH_PX /
/// 2`, и держит его тест (`the_hatch_step_fits_inside_the_pending_ribbon`), а
/// не это правило: `HATCH_PX` живёт в шейдере, и переписанный сюда числом он
/// разошёлся бы с настоящим молча.
const PENDING_HALF_PX: f32 = 2.0;

pub struct Outlines {
    /// Просто очерченные — линиями.
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Оба вида лент — одной парой буферов: вершина у них общая, и различает
    /// их только фрагментный шейдер, то есть отрезок индексов, которым их
    /// рисуют.
    pub ribbon: Vec<RibbonVertex>,
    pub ribbon_indices: Vec<u32>,
    /// Отрезок индексов ленты выделенного и отрезок ленты едущего. Порядок
    /// здесь не вкусовой: отрезки идут подряд, и разделить их можно только
    /// тем, что первый собран целиком раньше второго.
    pub selected: std::ops::Range<u32>,
    pub pending: std::ops::Range<u32>,
}

impl Outlines {
    pub fn build(outlines: &[Outline]) -> Self {
        let mut built = Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            ribbon: Vec::new(),
            ribbon_indices: Vec::new(),
            selected: 0..0,
            pending: 0..0,
        };

        // Уплотнение одно на все три вида: лента и линия обводят один и тот же
        // контур, и разойдись они хоть на вершину — выделенный снимок
        // обводился бы не там, где стоял до выделения. Считается оно один раз
        // и здесь: дальше контуры разбираются по видам тремя проходами, а
        // уплотнять один и тот же контур трижды незачем.
        let rings: Vec<(OutlineStyle, Vec<Vertex>)> = outlines
            .iter()
            .map(|outline| (outline.style(), ring(outline)))
            .filter(|(_, ring)| ring.len() >= 3)
            .collect();

        for (_, ring) in rings.iter().filter(|(style, _)| *style == OutlineStyle::OutlinePlain) {
            built.line(ring);
        }
        for (_, ring) in rings.iter().filter(|(style, _)| *style == OutlineStyle::OutlineSelected) {
            built.ribbon(ring, SELECTED_HALF_PX);
        }
        built.selected = 0..built.ribbon_indices.len() as u32;
        for (_, ring) in rings.iter().filter(|(style, _)| *style == OutlineStyle::OutlinePending) {
            built.ribbon(ring, PENDING_HALF_PX);
        }
        built.pending = built.selected.end..built.ribbon_indices.len() as u32;

        built
    }

    /// Замкнутая ломаная: каждая вершина соединена со следующей, последняя — с
    /// первой.
    fn line(&mut self, ring: &[Vertex]) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(ring);

        let count = ring.len() as u32;
        for i in 0..count {
            self.indices.extend_from_slice(&[base + i, base + (i + 1) % count]);
        }
    }

    /// Замкнутая лента: на каждую вершину по паре — левый край и правый, — а на
    /// каждое звено четырёхугольник из двух треугольников.
    ///
    /// В стороны вершины разводит шейдер, здесь они лежат одна в другой: в
    /// какую сторону смотрит край ленты, видно только там, где известен размер
    /// кадра. Отсюда едет одно — на сколько разводить (`half` — полуширина в
    /// пикселях кадра).
    fn ribbon(&mut self, ring: &[Vertex], half: f32) {
        let base = self.ribbon.len() as u32;
        let count = ring.len();
        for (index, vertex) in ring.iter().enumerate() {
            let prev = ring[(index + count - 1) % count].position;
            let next = ring[(index + 1) % count].position;
            for side in [-half, half] {
                self.ribbon.push(RibbonVertex {
                    position: vertex.position,
                    normal: vertex.normal,
                    prev,
                    next,
                    side,
                });
            }
        }

        for i in 0..count as u32 {
            let here = base + i * 2;
            let ahead = base + ((i + 1) % count as u32) * 2;
            self.ribbon_indices
                .extend_from_slice(&[here, here + 1, ahead, ahead, here + 1, ahead + 1]);
        }
    }
}

/// Вершины замкнутого контура, уплотнённые до [`MAX_EDGE_DEG`]. Пусто —
/// очерчивать нечего: двумя точками замкнутой линии не выйдет.
fn ring(outline: &Outline) -> Vec<Vertex> {
    // Замыкание кладём мы сами, поэтому повторённую последнюю точку
    // отбрасываем: иначе замыкающее ребро выродилось бы в точку.
    let points = match outline.points.as_slice() {
        [first, .., last] if first.lat == last.lat && first.lon == last.lon => {
            &outline.points[..outline.points.len() - 1]
        }
        all => all,
    };
    if points.len() < 3 {
        return Vec::new();
    }

    let mut ring = Vec::new();
    for (index, point) in points.iter().enumerate() {
        let next = &points[(index + 1) % points.len()];
        edge(&mut ring, (point.lat, point.lon), (next.lat, next.lon));
    }
    ring
}

/// Ребро от одной вершины до другой, уплотнённое до [`MAX_EDGE_DEG`].
///
/// Чем именно вести ребро и как далеко разворачивать его долготу, решает
/// `geodesy`: тот же ответ нужен считающему попадание щелчком, и второй его
/// экземпляр разошёлся бы с этим молча (см. `geodesy::along`).
///
/// Конечная точка не кладётся: её положит следующее ребро, а последнее замкнёт
/// контур.
fn edge(ring: &mut Vec<Vertex>, from: (f64, f64), to: (f64, f64)) {
    let steps = (geodesy::edge_span(from, to) / MAX_EDGE_DEG).ceil().max(1.0) as u32;
    for step in 0..steps {
        let (lat_deg, lon_deg) = geodesy::edge_point(from, to, f64::from(step) / f64::from(steps));
        ring.push(mesh::vertex(Geodetic { lat_deg, lon_deg, height_m: HEIGHT_M }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::globe::GeoPoint;

    fn outline(points: &[(f64, f64)]) -> Outline {
        Outline {
            points: points.iter().map(|&(lat, lon)| GeoPoint { lat, lon }).collect(),
            style: OutlineStyle::OutlinePlain as i32,
        }
    }

    fn styled(points: &[(f64, f64)], style: OutlineStyle) -> Outline {
        Outline { style: style as i32, ..outline(points) }
    }

    /// Три вида разъезжаются по своим буферам, а отрезки лент — по своим
    /// концам: рисуются они разными пайплайнами, и слипшиеся отрезки означали
    /// бы штрих на выделенном либо сплошную ленту на едущем.
    #[test]
    fn each_style_goes_to_its_own_draw() {
        let square = [(10.0, 10.0), (10.0, 12.0), (12.0, 12.0), (12.0, 10.0)];
        let built = Outlines::build(&[
            outline(&square),
            styled(&square, OutlineStyle::OutlineSelected),
            styled(&square, OutlineStyle::OutlinePending),
        ]);

        assert!(!built.indices.is_empty(), "очерченный рисуется линиями");
        assert!(!built.selected.is_empty(), "выделенный рисуется лентой");
        assert_eq!(built.selected.end, built.pending.start, "отрезки идут подряд");
        assert_eq!(built.pending.end, built.ribbon_indices.len() as u32);
        assert_eq!(
            built.selected.len(),
            built.pending.len(),
            "контур один и тот же — и лент из него выходит поровну"
        );

        // Толщину ленте задаёт вершина, и у двух видов она разная: одинаковая
        // означала бы, что штрих — единственное, чем они различаются.
        let width = |range: &std::ops::Range<u32>| {
            let vertex = built.ribbon_indices[range.start as usize] as usize;
            built.ribbon[vertex].side.abs()
        };
        assert!(width(&built.selected) > width(&built.pending));
    }

    /// Ширина штриховой ленты и шаг самого штриха связаны, а живут в разных
    /// файлах: полуширина здесь, шаг — в шейдере. Сойтись они обязаны
    /// механически, поэтому шаг читается из самого шейдера, а не переписан
    /// сюда числом: переписанное разошлось бы с настоящим молча, и лента,
    /// идущая вдоль штриха, пропала бы с шара целиком.
    #[test]
    fn the_hatch_step_fits_inside_the_pending_ribbon() {
        let shader = include_str!("globe.wgsl");
        let step: f32 = shader
            .lines()
            .find_map(|line| line.trim().strip_prefix("const HATCH_PX: f32 = "))
            .and_then(|tail| tail.trim_end_matches(';').parse().ok())
            .expect("в шейдере нет шага штриховки");

        // Полосы штриха идут под 45°, поэтому поперёк ленты, лежащей вдоль
        // них, набегает её ширина, умноженная на корень из двух. Перекрыть
        // этот отрезок обязан половину шага — ровно ту, что выброшена.
        let across = PENDING_HALF_PX * 2.0 * std::f32::consts::SQRT_2;
        assert!(
            across > step * 0.5,
            "лента укладывается в выброшенную половину ({} против {}): вдоль штриха она пропадёт",
            across,
            step * 0.5
        );
    }

    /// Контур, которого нет: двумя точками замкнутой ломаной не выйдет, и ни
    /// одна лента с ним не заводится.
    #[test]
    fn a_degenerate_outline_draws_nothing() {
        let built = Outlines::build(&[styled(&[(10.0, 10.0), (10.0, 12.0)], OutlineStyle::OutlinePending)]);
        assert!(built.ribbon_indices.is_empty());
        assert!(built.pending.is_empty());
    }

    /// Пара углов вершины — по её нормали: она единичная, и выводит из неё
    /// широту с долготой та же функция, что и всюду. Наружу нормаль уезжает
    /// в f32, поэтому здесь она расширяется обратно.
    fn angles_of(vertex: &Vertex) -> (f64, f64) {
        geodesy::angles(glam::Vec3::from(vertex.normal).as_dvec3())
    }

    /// Прямоугольник в градусной сетке обводится по своим краям, а не по дугам
    /// между углами. У глобального продукта края — это две полные параллели:
    /// сведи их дуга к хорде, и от контура остался бы один меридиан, потому что
    /// −180 и +180 — одна и та же точка шара.
    #[test]
    fn a_whole_earth_band_keeps_its_parallels() {
        let band = outline(&[
            (-70.0125, -180.0125),
            (70.0125, -180.0125),
            (70.0125, 179.9875),
            (-70.0125, 179.9875),
        ]);
        let ring = ring(&band);
        let lons: Vec<f64> = ring.iter().map(|v| angles_of(v).1).collect();
        let east = lons.iter().filter(|lon| **lon > 60.0).count();
        let west = lons.iter().filter(|lon| **lon < -60.0).count();
        assert!(east > 30 && west > 30, "контур собрался в меридиан: {} восточных, {} западных", east, west);

        // Меридианы при этом остаются меридианами и доходят до обоих краёв.
        let lats: Vec<f64> = ring.iter().map(|v| angles_of(v).0).collect();
        assert!(lats.iter().any(|lat| *lat > 69.0) && lats.iter().any(|lat| *lat < -69.0));
    }

    /// А коробка поперёк шва — не круг, и обводится она короткой стороной:
    /// записанная как 170 → −170, прочитанная буквально она шла бы вокруг всей
    /// Земли мимо снятого.
    #[test]
    fn a_box_across_the_seam_takes_the_short_way() {
        let box_ = outline(&[
            (65.0, 170.0),
            (65.0, -170.0),
            (60.0, -170.0),
            (60.0, 170.0),
        ]);
        let ring = ring(&box_);
        // Ни одна вершина не уходит за пределы снятого: 170°…190° (то же, что
        // 170°…−170°), а середина Земли остаётся нетронутой.
        let inside = |lon: f64| !(-160.0..=160.0).contains(&lon);
        assert!(
            ring.iter().all(|v| inside(angles_of(v).1)),
            "контур ушёл через полмира: {:?}",
            ring.iter().map(|v| angles_of(v).1).take(8).collect::<Vec<f64>>()
        );
        // И уплотнён он по своей длине, а не по чужой: двадцать градусов — это
        // десятки вершин, а не сотни.
        assert!(ring.len() < 120, "уплотнено как трёхсотградусное ребро: {}", ring.len());
    }

    /// А у полосы съёмки углы лежат где придётся, и её рёбра по-прежнему ведутся
    /// дугами: середина ребра севернее прямой в градусах — на этом и держится
    /// совпадение контура со снимком на высоких широтах.
    #[test]
    fn a_swath_edge_still_follows_the_arc() {
        let swath = outline(&[(73.0, 10.0), (73.4, 30.0), (72.0, 31.0), (71.6, 11.0)]);
        let ring = ring(&swath);
        let north = ring
            .iter()
            .map(angles_of)
            .filter(|(_, lon)| (19.0..21.0).contains(lon))
            .map(|(lat, _)| lat)
            .fold(f64::MIN, f64::max);
        assert!(north > 73.3, "ребро пошло локсодромой: {}", north);
    }
}
