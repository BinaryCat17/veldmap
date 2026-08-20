//! Контуры на поверхности: ломаные из присланных вершин.
//!
//! Вершина обычного контура та же, что у Земли и сетки (`mesh::Vertex`), —
//! точка с нормалью, — поэтому и раскладка вершинного буфера у них общая.
//! Отдельный буфер нужен не формату, а времени жизни: Земля строится раз, а
//! контуры меняются с каждым ответом на поиск.
//!
//! Выделенный контур — не линия, а лента: линия в пайплайне всегда в пиксель
//! шириной, а выделить один контур среди полусотни соседних нужно так, чтобы
//! это было видно. Ширина у ленты экранная, а не мировая: в мире она зависела
//! бы от приближения — на отлёте расплывалась бы в пятно, вблизи истончалась в
//! ту же линию. Раздаёт её поэтому вершинный шейдер (`vs_ribbon` в globe.wgsl),
//! а здесь лежит то, чего он сам знать не может: соседи вершины по обходу.
//!
//! У снимка, который на шар только едет, заштрихована вся занятая им область:
//! контур говорит, где он ляжет, а штриховка — сколько места займёт. Строится
//! она здесь же и из того же кольца — веером к его середине.

use glam::DVec3;

use crate::module::geodesy::{self, Geodetic};
use crate::module::gpu::RibbonVertex;
use crate::module::mesh::{self, Vertex};
use crate::proto::globe::{Outline, OutlineStyle};

/// Высота контуров над поверхностью — общий вынос: поделены они достаточно
/// мелко, чтобы на нём поместиться, и сходятся там со снимком, который обводят.
///
/// Высота у контура одна на все три вида. Разойдись линия с заливкой, и
/// штриховка вылезла бы за собственный край — не потому, что построена шире, а
/// потому, что нарисована ближе к глазу (см. `mesh::SURFACE_LIFT_M`).
fn height_m() -> f64 {
    mesh::lift_m(max_edge_deg() * 2.0)
}

/// Насколько мелко бить ребро. Ломаная соединяет вершины отрезками в
/// пространстве, а лежать должна на поверхности: чем длиннее ребро, тем глубже
/// отрезок уходит под неё.
///
/// Половина предельной хорды, а не её частное с √2, как у ровной решётки. Тем
/// же шагом идёт и веер заливки, а квад у веера ровным не бывает: на углу
/// кольца соседние вершины отстоят от середины кольца на целый шаг друг от
/// друга, и к шагу по радиусу эта разница прибавляется, а не складывается с
/// ним по теореме Пифагора. Отсюда двойка: у кольца, идущего от середины
/// прочь, диагональ квада выходит вдвое длиннее шага.
fn max_edge_deg() -> f64 {
    mesh::max_chord_deg() / 2.0
}

/// Насколько далеко от середины кольца может лежать его край, чтобы веер
/// заливки был построим, — косинусом угла, потому что сравнивается он со
/// скалярным произведением направлений.
///
/// Ограничение — четверть оборота, и оно не вкусовое: за ней точки кольца
/// расходятся на пол-Земли, середины у него нет вовсе, и веер из любой точки
/// пошёл бы поверх обратной стороны. Так записан пояс климатической сетки — от
/// −180 до 180; заливки у него не будет, а контур останется, и он и есть
/// главное.
///
/// Пять градусов до края взяты запасом: у кольца, дотянувшегося ровно до
/// четверти оборота, крайние узлы веера сходятся в точку, и считать по ним
/// нечего.
const FILL_REACH: f64 = 0.087; // cos 85°

pub struct Outlines {
    /// Сами контуры — линиями. Едущий на шар очерчен ими же: от обычного его
    /// отличает не линия, а штриховка внутри неё.
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Выделенный — лентой, своей парой буферов: у неё своя вершина и свой
    /// пайплайн, и делить их с линиями было бы нечем.
    pub ribbon: Vec<RibbonVertex>,
    pub ribbon_indices: Vec<u32>,
    /// Заштрихованные области едущих на шар. Вершина у них та же, что у линий
    /// и у Земли, — точка поверхности с нормалью, — а пара буферов своя:
    /// рисуются они треугольниками, а не линиями.
    pub hatch: Vec<Vertex>,
    pub hatch_indices: Vec<u32>,
}

impl Outlines {
    pub fn build(outlines: &[Outline]) -> Self {
        let mut built = Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            ribbon: Vec::new(),
            ribbon_indices: Vec::new(),
            hatch: Vec::new(),
            hatch_indices: Vec::new(),
        };

        for outline in outlines {
            // Уплотнение одно на все три вида: лента, линия и штриховка
            // обводят один и тот же контур, и разойдись они хоть на вершину —
            // выделенный снимок обводился бы не там, где стоял до выделения, а
            // штриховка вылезла бы за собственный край.
            let ring = ring(outline);
            if ring.len() < 3 {
                continue;
            }
            match outline.style() {
                OutlineStyle::OutlineSelected => built.ribbon(&ring),
                OutlineStyle::OutlinePlain => built.line(&ring),
                // Едущий очерчен обычной линией, а внутри неё заштрихован:
                // сказать надо не «этот снимок особенный», а «место занято».
                OutlineStyle::OutlinePending => {
                    built.line(&ring);
                    built.fill(&ring);
                }
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
            let here = (vertex.position, vertex.position_low);
            let at = |node: &Vertex| (node.position, node.position_low);
            // Смещениями, а не местами соседей: старшие половины у соседних
            // вершин контура совпадают, и разность, взятая в шейдере, дала бы
            // ноль там, где до соседа полсотни метров.
            let prev = geodesy::offset(at(&ring[(index + count - 1) % count]), here);
            let next = geodesy::offset(at(&ring[(index + 1) % count]), here);
            for side in [-1.0, 1.0] {
                self.ribbon.push(RibbonVertex {
                    position: vertex.position,
                    position_low: vertex.position_low,
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
    /// Заливка области, которую занимает снимок, — веер от середины кольца к
    /// его краю.
    ///
    /// Веером, а не разбором на уши: контуры снимков — четырёхугольники и
    /// полосы съёмки, то есть выпуклые, и у выпуклого кольца веер из его
    /// середины и есть точное разбиение. У вогнутого он вылезет за край, и на
    /// штриховке это читается как чуть более широкая кромка, а не как ошибка;
    /// разбор на уши стоил бы вдесятеро дороже ради случая, которого у съёмок
    /// не бывает.
    ///
    /// По радиусу веер разбит так же мелко, как уплотнено само кольцо, и по той
    /// же причине (см. [`max_edge_deg`]): треугольник от середины до края — это
    /// хорда, и уже на градусе дуги её середина уходит под вынос глубже, чем
    /// контур над ней поднят, где её съедает тест глубины.
    ///
    /// Потолка густоты у веера, в отличие от варп-сетки наложений, нет, и
    /// заводить его не под что. И по радиусу, и вдоль кольца шаг один и тот же
    /// ([`max_edge_deg`]), так что вершин выходит `360·sin θ·θ / шаг²`: у полосы
    /// Sentinel-5P — самой широкой из приезжающих сюда, 2600 км, то есть 23° —
    /// это сорок пять тысяч вершин и полтора мегабайта. У кольца, дотянувшегося
    /// до [`FILL_REACH`], вышло бы четыреста тысяч и пятнадцать мегабайт, но
    /// такому кольцу надо быть девятнадцать тысяч километров поперёк, а что
    /// шире — веером не заливается вовсе, отсекаясь выше.
    ///
    /// Появись такой снимок — лечится это тем же приёмом, что у наложений:
    /// потолок долей, а остаток провала — подъёмом.
    ///
    /// Ничего не рисуется, если середины у кольца нет: см. [`FILL_REACH`].
    fn fill(&mut self, ring: &[Vertex]) {
        let direction = |vertex: &Vertex| glam::Vec3::from(vertex.normal).as_dvec3();
        let Some(centre) = ring.iter().map(direction).sum::<DVec3>().try_normalize() else {
            return;
        };
        // Насколько далеко край — по самой дальней вершине. Им же меряется и
        // разбиение по радиусу, и построимость самого веера.
        let reach = ring.iter().map(direction).map(|point| centre.dot(point)).fold(1.0, f64::min);
        if reach < FILL_REACH {
            return;
        }
        let steps =
            (reach.clamp(-1.0, 1.0).acos().to_degrees() / max_edge_deg()).ceil().max(1.0) as u32;

        // Середина — одна вершина на весь веер: от неё расходится первое
        // кольцо треугольников, а дальше идут четырёхугольники между соседними
        // кольцами.
        let base = self.hatch.len() as u32;
        self.hatch.push(surface(centre));
        for step in 1..=steps {
            let part = f64::from(step) / f64::from(steps);
            for point in ring.iter().map(direction) {
                // Направления, а не точки: середина отсчёта — центр Земли, и
                // выправленное направление снова ложится на поверхность.
                // Долей пути по хорде, а не по дуге: шаг мелкий, а неровность
                // расстановки узлов заливке безразлична. Выродиться хорда не
                // может — противоположных точек в кольце нет (см. [`FILL_REACH`]).
                self.hatch.push(surface(centre.lerp(point, part).normalize()));
            }
        }

        // Кольца веера считаются от нуля, а не от единицы: середина стои́т
        // перед ними отдельной вершиной, и вычитание внутри счёта однажды
        // завернуло бы `u32` за ноль.
        let count = ring.len() as u32;
        let ring_at = |level: u32| base + 1 + level * count;
        for index in 0..count {
            let (here, ahead) = (ring_at(0) + index, ring_at(0) + (index + 1) % count);
            self.hatch_indices.extend_from_slice(&[base, here, ahead]);
        }
        for level in 0..steps - 1 {
            let (inner, outer) = (ring_at(level), ring_at(level + 1));
            for index in 0..count {
                let ahead = (index + 1) % count;
                self.hatch_indices.extend_from_slice(&[
                    inner + index, outer + index, inner + ahead,
                    inner + ahead, outer + index, outer + ahead,
                ]);
            }
        }
    }
}

/// Точка поверхности по направлению из центра Земли — на той же высоте, что и
/// контуры.
fn surface(direction: DVec3) -> Vertex {
    let (lat_deg, lon_deg) = geodesy::angles(direction);
    mesh::vertex(Geodetic { lat_deg, lon_deg, height_m: height_m() })
}

/// Вершины замкнутого контура, уплотнённые до [`max_edge_deg`]. Пусто —
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

/// Ребро от одной вершины до другой, уплотнённое до [`max_edge_deg`].
///
/// Чем именно вести ребро и как далеко разворачивать его долготу, решает
/// `geodesy`: тот же ответ нужен считающему попадание щелчком, и второй его
/// экземпляр разошёлся бы с этим молча (см. `geodesy::along`).
///
/// Конечная точка не кладётся: её положит следующее ребро, а последнее замкнёт
/// контур.
fn edge(ring: &mut Vec<Vertex>, from: (f64, f64), to: (f64, f64)) {
    let steps = (geodesy::edge_span(from, to) / max_edge_deg()).ceil().max(1.0) as u32;
    let height_m = height_m();
    for step in 0..steps {
        let (lat_deg, lon_deg) = geodesy::edge_point(from, to, f64::from(step) / f64::from(steps));
        ring.push(mesh::vertex(Geodetic { lat_deg, lon_deg, height_m }));
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

    /// Три вида разъезжаются по своим буферам: рисуются они разными
    /// пайплайнами, и слипшиеся буферы означали бы ленту вокруг обычного
    /// контура либо штриховку без края.
    ///
    /// Едущий на шар при этом рисуется дважды — линией и штриховкой: линия
    /// говорит, где он ляжет, штриховка — сколько места займёт.
    #[test]
    fn each_style_goes_to_its_own_draw() {
        let square = [(10.0, 10.0), (10.0, 12.0), (12.0, 12.0), (12.0, 10.0)];

        let plain = Outlines::build(&[outline(&square)]);
        assert!(!plain.indices.is_empty(), "очерченный рисуется линиями");
        assert!(plain.ribbon_indices.is_empty() && plain.hatch_indices.is_empty());

        let selected = Outlines::build(&[styled(&square, OutlineStyle::OutlineSelected)]);
        assert!(!selected.ribbon_indices.is_empty(), "выделенный рисуется лентой");
        assert!(selected.indices.is_empty() && selected.hatch_indices.is_empty());

        let pending = Outlines::build(&[styled(&square, OutlineStyle::OutlinePending)]);
        assert_eq!(pending.indices, plain.indices, "едущий очерчен той же линией");
        assert!(!pending.hatch_indices.is_empty(), "область не заштрихована");
        assert!(pending.ribbon_indices.is_empty(), "едущий не выделяют лентой");
    }

    /// Заливка лежит на поверхности, а не режет её хордой: соседние узлы веера
    /// расходятся не дальше, чем вершины самого контура.
    ///
    /// Ради этого веер и разбит по радиусу. Треугольник от середины кольца до
    /// его края — это хорда через десятки градусов дуги, и середина такой
    /// хорды уходит под поверхность на сотню километров, где её съедает тест
    /// глубины: от области осталась бы одна кромка.
    #[test]
    fn the_filled_area_follows_the_surface() {
        // Двадцать градусов поперёк — обычная гранула, и хорда через неё
        // проседает почти на сотню километров.
        let wide = [(0.0, 0.0), (0.0, 20.0), (20.0, 20.0), (20.0, 0.0)];
        let built = Outlines::build(&[styled(&wide, OutlineStyle::OutlinePending)]);

        let between = |left: u32, right: u32| {
            let ends = [left, right].map(|index| {
                let (lat, lon) = angles_of(&built.hatch[index as usize]);
                geodesy::unit(lat, lon)
            });
            ends[0].dot(ends[1]).clamp(-1.0, 1.0).acos().to_degrees()
        };
        let mut widest = 0.0f64;
        for triangle in built.hatch_indices.chunks(3) {
            widest = widest
                .max(between(triangle[0], triangle[1]))
                .max(between(triangle[1], triangle[2]))
                .max(between(triangle[2], triangle[0]));
        }

        // Меряется не сам шаг, а то, ради чего он выбран: середина хорды в
        // столько-то градусов проседает под поверхность на `1 − cos(θ/2)`
        // (радиус Земли здесь — единица), и просесть глубже, чем контур поднят,
        // ей нельзя — там её съест тест глубины.
        let sag = 1.0 - (widest.to_radians() / 2.0).cos();
        let lift = f64::from(geodesy::metres(height_m()));
        assert!(
            sag < lift,
            "ячейка в {} градусов проседает на {} при выносе {}",
            widest,
            sag,
            lift
        );
    }

    /// Контур садится на общий вынос — тот самый, на котором лежит снимок под
    /// ним. Разойдись они, и обвод поехал бы относительно обведённого тем
    /// сильнее, чем ближе к краю кадра.
    #[test]
    fn an_outline_lands_on_the_common_lift() {
        assert!(
            (height_m() - mesh::SURFACE_LIFT_M).abs() < 1e-9,
            "контур поднялся над общим выносом: {} м", height_m()
        );
    }

    /// Кольцу, разошедшемуся на пол-Земли, середины нет вовсе: веер из любой
    /// точки пошёл бы поверх обратной стороны. Такое кольцо остаётся одним
    /// контуром — он и есть главное, а заливка при нём необязательна.
    #[test]
    fn a_ring_around_the_earth_gets_no_fill() {
        let band = outline(&[
            (-70.0125, -180.0125),
            (70.0125, -180.0125),
            (70.0125, 179.9875),
            (-70.0125, 179.9875),
        ]);
        let built = Outlines::build(&[Outline {
            style: OutlineStyle::OutlinePending as i32,
            ..band
        }]);
        assert!(!built.indices.is_empty(), "контур пропал вместе с заливкой");
        assert!(built.hatch_indices.is_empty(), "веер построен поверх обратной стороны");
    }

    /// А шапка вокруг полюса Землю по долготе тоже обходит — и заливается:
    /// середина у неё есть, и это сам полюс.
    #[test]
    fn a_polar_cap_is_filled_all_the_same() {
        let cap: Vec<(f64, f64)> =
            (0..12).map(|step| (85.0, f64::from(step) * 30.0 - 180.0)).collect();
        let built = Outlines::build(&[styled(&cap, OutlineStyle::OutlinePending)]);
        assert!(!built.hatch_indices.is_empty(), "шапка осталась незалитой");
    }

    /// Контур, которого нет: двумя точками замкнутой ломаной не выйдет, и ни
    /// линии, ни заливки с ним не заводится.
    #[test]
    fn a_degenerate_outline_draws_nothing() {
        let built =
            Outlines::build(&[styled(&[(10.0, 10.0), (10.0, 12.0)], OutlineStyle::OutlinePending)]);
        assert!(built.indices.is_empty());
        assert!(built.hatch_indices.is_empty());
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
        // И уплотнён он по своей длине, а не по чужой: короткая сторона — двадцать
        // градусов долготы, длинная — триста сорок, и уплотнение по длинной
        // стоило бы кольца на порядок гуще.
        let long_way = (340.0 / max_edge_deg()).ceil() as usize;
        assert!(
            ring.len() < long_way / 3,
            "уплотнено как трёхсотградусное ребро: {} вершин против {} у длинного пути",
            ring.len(), long_way
        );
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
