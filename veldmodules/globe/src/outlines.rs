//! Контуры на поверхности: ломаные из присланных вершин.
//!
//! Вершина обычного контура та же, что у Земли и сетки (`mesh::Vertex`), —
//! точка с нормалью, — поэтому и раскладка вершинного буфера у них общая.
//! Отдельный буфер нужен не формату, а времени жизни: Земля строится раз, а
//! контуры меняются с каждым ответом на поиск.
//!
//! Выбранный контур — не линия, а лента: линия в пайплайне всегда в пиксель
//! шириной, а выделить один контур среди полусотни соседних нужно так, чтобы
//! это было видно. Ширина у ленты экранная, а не мировая: в мире она зависела
//! бы от приближения — на отлёте расплывалась бы в пятно, вблизи истончалась в
//! ту же линию. Раздаёт её поэтому вершинный шейдер (`vs_ribbon` в globe.wgsl),
//! а здесь лежит то, чего он сам знать не может: соседи вершины по обходу, из
//! которых считается направление ленты.

use crate::module::geodesy::{self, Geodetic};
use crate::module::gpu::RibbonVertex;
use crate::module::mesh::{self, Vertex};
use crate::proto::globe::Outline;

/// Высота контуров над поверхностью — та же, что у сетки: они лежат на Земле
/// и не должны с ней спорить за глубину.
const HEIGHT_M: f64 = mesh::GRID_HEIGHT_M;

/// Насколько мелко бить ребро. Ломаная соединяет вершины отрезками в
/// пространстве, а лежать должна на поверхности: чем длиннее ребро, тем
/// глубже отрезок уходит под неё. Градус дуги — это около 111 км, и просадка
/// в середине такого отрезка меньше четверти километра, то есть много меньше
/// высоты, на которой контур висит.
const MAX_EDGE_DEG: f64 = 1.0;

pub struct Outlines {
    /// Невыбранные — линиями.
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Выбранные — лентой, своей парой буферов: у неё своя вершина и свой
    /// пайплайн, и делить их с линиями было бы нечем.
    pub ribbon: Vec<RibbonVertex>,
    pub ribbon_indices: Vec<u32>,
}

impl Outlines {
    pub fn build(outlines: &[Outline]) -> Self {
        let mut built = Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            ribbon: Vec::new(),
            ribbon_indices: Vec::new(),
        };

        for outline in outlines {
            // Уплотнение одно на оба вида: лента и линия обводят один и тот же
            // контур, и разойдись они хоть на вершину — выбранный снимок
            // обводился бы не там, где стоял до выбора.
            let ring = ring(outline);
            if ring.len() < 3 {
                continue;
            }
            match outline.selected {
                true => built.ribbon(&ring),
                false => built.line(&ring),
            }
        }

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
    /// В стороны вершины разводит шейдер, здесь они лежат одна в другой: на
    /// сколько разводить, известно только там, где известен размер кадра.
    fn ribbon(&mut self, ring: &[Vertex]) {
        let base = self.ribbon.len() as u32;
        let count = ring.len();
        for (index, vertex) in ring.iter().enumerate() {
            let prev = ring[(index + count - 1) % count].position;
            let next = ring[(index + 1) % count].position;
            for side in [-1.0, 1.0] {
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

/// Чем ведётся ребро контура.
///
/// Решает это само ребро, и вопрос здесь не о вкусе. Край снимка, снятого
/// полосой, идёт по поверхности кратчайшим путём — дугой большого круга; прямая
/// в широте с долготой на его месте (локсодрома) у гранулы Sentinel-1 в 414 км
/// на 73° с. ш. отходит от снятого на 27 км, а дуга по тем же углам — на 0.6 км.
///
/// А у растра, лежащего в градусной сетке, края — это в точности параллели и
/// меридианы, и дуга между их концами проходит мимо: у ребра вдоль параллели она
/// срезает к полюсу, а у ребра в целых 360° вырождается вовсе, потому что концы
/// его — одна и та же точка шара. Тогда от всего контура глобального продукта
/// остаётся один меридиан.
///
/// Отличается одно от другого по самому ребру: у параллели совпадают широты
/// концов, у меридиана — долготы. У полосы съёмки не совпадает ни то, ни другое:
/// углы её лежат где придётся, и совпадение до последнего разряда там не
/// случается.
enum Along {
    Parallel,
    Meridian,
    Arc,
}

/// Насколько разность долгот должна приблизиться к полному кругу, чтобы ребро
/// считалось им. Глобальная сетка обводится краями вида −180,0125 → 179,9875,
/// то есть на полпикселя не дотягивает до ровных 360°.
const FULL_CIRCLE_DEG: f64 = 359.0;

/// Ребро от одной вершины до другой, уплотнённое до [`MAX_EDGE_DEG`].
///
/// Короткую сторону через антимеридиан дуга выбирает сама, а у параллели
/// выбирают за неё, и выбор этот один: полный круг остаётся кругом, всё
/// прочее разворачивается к ближней ветви. Полный круг сворачивать нельзя —
/// концы его одна и та же точка шара, и от контура остался бы меридиан; а не
/// разворачивать прочее нельзя тем более: коробка в двадцать градусов поперёк
/// шва записана как 170 → −170, и прочитанная буквально она обводится вокруг
/// всей Земли.
///
/// Конечная точка не кладётся: её положит следующее ребро, а последнее замкнёт
/// контур.
fn edge(ring: &mut Vec<Vertex>, from: (f64, f64), to: (f64, f64)) {
    let along = match (from.0 == to.0, from.1 == to.1) {
        (true, _) => Along::Parallel,
        (_, true) => Along::Meridian,
        _ => Along::Arc,
    };
    let sweep = match (to.1 - from.1).abs() >= FULL_CIRCLE_DEG {
        true => to.1 - from.1,
        false => geodesy::unwind(from.1, to.1) - from.1,
    };
    let span = match along {
        // Длина параллели меряется не разностью долгот: у 70-й она вдвое
        // короче экватора, и уплотнять её вдвое гуще незачем.
        Along::Parallel => sweep.abs() * from.0.to_radians().cos(),
        Along::Meridian => (to.0 - from.0).abs(),
        Along::Arc => geodesy::separation(from, to),
    };
    let steps = (span / MAX_EDGE_DEG).ceil().max(1.0) as u32;
    for step in 0..steps {
        let t = f64::from(step) / f64::from(steps);
        let (lat_deg, lon_deg) = match along {
            Along::Parallel => (from.0, from.1 + sweep * t),
            Along::Meridian => (from.0 + (to.0 - from.0) * t, from.1),
            Along::Arc => geodesy::between(from, to, t),
        };
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
            selected: false,
        }
    }

    /// Пара углов вершины — по её нормали: она единичная, и выводит из неё
    /// широту с долготой та же функция, что и всюду. Наружу нормаль уезжает
    /// в f32, поэтому здесь она расширяется обратно.
    fn angles_of(vertex: &Vertex) -> (f64, f64) {
        let unit = vertex.normal;
        geodesy::angles([f64::from(unit[0]), f64::from(unit[1]), f64::from(unit[2])])
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
