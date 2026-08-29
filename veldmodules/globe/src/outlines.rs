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
            let loops = ring(outline);
            // Заливка — только там, где рисуется одна петля. Две петли — это
            // пояс, а у пояса середины нет: веер из любой точки пошёл бы
            // поверх того, чего пояс как раз и не покрывает (см.
            // [`FILL_REACH`]). По петле порознь его звать тем более нельзя —
            // параллель сама по себе кольцо вокруг полюса, и веер заштриховал
            // бы шапку вместо пояса.
            //
            // Считается по дожившим до отрисовки: у прямоугольника, упёршегося
            // в полюс, вторая петля вырождена в точку, а первая — честная
            // шапка, и заливать её надо.
            let drawn: Vec<&Vec<Vertex>> = loops.iter().filter(|run| run.len() >= 3).collect();
            let whole = drawn.len() == 1;
            for ring in drawn {
                match outline.style() {
                    OutlineStyle::OutlineSelected => built.ribbon(ring),
                    OutlineStyle::OutlinePlain => built.line(ring),
                    // Едущий очерчен обычной линией, а внутри неё заштрихован:
                    // сказать надо не «этот снимок особенный», а «место занято».
                    OutlineStyle::OutlinePending => {
                        built.line(ring);
                        if whole {
                            built.fill(ring);
                        }
                    }
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
    /// ([`max_edge_deg`]), так что вершин выходит `360·sin θ·θ / шаг²`, где θ —
    /// **радиус** кольца. У полосы Sentinel-5P, самой широкой из приезжающих
    /// сюда (2600 км поперёк, то есть радиус около двенадцати градусов), это
    /// двенадцать тысяч вершин и полмегабайта. У кольца, дотянувшегося до
    /// [`FILL_REACH`], вышло бы четыреста тысяч и пятнадцать мегабайт — но
    /// такому кольцу надо быть девятнадцать тысяч километров поперёк, а что
    /// шире, веером не заливается вовсе, отсекаясь выше.
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

/// Петли контура, уплотнённые до [`max_edge_deg`]. Пусто — очерчивать нечего:
/// двумя точками замкнутой линии не выйдет.
///
/// Петля обычно одна: контур снимка — четырёхугольник или полоса съёмки.
/// Больше их у контура, записанного прямоугольником поперёк всей Земли: боковых
/// краёв у него не существует вовсе, они взялись от способа записи
/// (см. [`seam_cuts`]).
fn ring(outline: &Outline) -> Vec<Vec<Vertex>> {
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

    let at = |index: usize| {
        let point = &points[index % points.len()];
        (point.lat, point.lon)
    };
    let places: Vec<(f64, f64)> = (0..points.len()).map(at).collect();
    let sweeps: Vec<f64> =
        (0..points.len()).map(|index| geodesy::sweep(at(index), at(index + 1))).collect();
    let cuts = seam_cuts(&places, &sweeps);

    // Обход начинается сразу после разреза: пробег, начатый с середины,
    // распался бы на два — начало кольца попало бы в одну петлю, а его
    // продолжение в другую.
    let start = cuts.first().map_or(0, |cut| cut + 1);
    let (mut loops, mut ring) = (Vec::new(), Vec::new());
    for step in 0..points.len() {
        let index = (start + step) % points.len();
        if cuts.contains(&index) {
            // Пустой пробег — два разреза подряд: так выходит у контура с
            // повторённой угловой вершиной, и петли за ним нет.
            if !ring.is_empty() {
                loops.push(std::mem::take(&mut ring));
            }
            continue;
        }
        edge(&mut ring, at(index), at(index + 1));
    }
    if !ring.is_empty() {
        loops.push(ring);
    }
    loops
}

/// Рёбра-разрезы: те, что не ведут контур, а только переходят с одного его
/// края на другой.
///
/// У пояса, обошедшего Землю, боковых краёв нет: прямоугольник записан
/// четырьмя углами, и его боковые рёбра — одна и та же линия шара, пройденная
/// вверх и вниз. Долготы они не несут, и по этому их и узнают.
///
/// Одного нуля мало: нулевое ребро бывает и настоящим краем. Спрашивают
/// пробеги между нулями, и спрашивают о двух вещах.
///
/// **Замкнулся ли пробег сам.** Шов — это переход с края на край, а значит
/// то, что он соединяет, и без него одно и то же место шара. Не сошлись концы
/// — нули здесь настоящие рёбра, и выбросить их значило бы стянуть петлю
/// хордой через нетронутое.
///
/// **Обошёл ли он Землю** ([`geodesy::FULL_CIRCLE_DEG`]). У глобальной сетки,
/// не сомкнувшейся на десять градусов, боковые рёбра тоже стоя́т по меридиану,
/// а пробеги дают ±350° — резать нечего. У пояса из четырёх вершин они дают
/// ±360°. Пробег с нулевым размахом (вроде отростка «туда и обратно») в счёт
/// не идёт вовсе: круга он не обходит, но и мешать соседям не должен.
///
/// От числа вершин признак не зависит, а вот от того, где они стоя́т, —
/// зависит: `sweep` разворачивает каждое ребро к ближней ветви, и сумма
/// сохраняется, только пока ни одно из них не длиннее полукруга. Пояс,
/// записанный четвертями, режется; он же, разбитый пополам, — уже нет.
fn seam_cuts(places: &[(f64, f64)], sweeps: &[f64]) -> Vec<usize> {
    let cuts: Vec<usize> =
        (0..sweeps.len()).filter(|index| sweeps[*index] == 0.0).collect();
    if cuts.is_empty() {
        return Vec::new();
    }

    let mut carried = false;
    for pair in 0..cuts.len() {
        let (from, to) = (cuts[pair], cuts[(pair + 1) % cuts.len()]);
        // До самого разреза, но не включая его: пробег, захвативший своё же
        // ребро, замкнулся бы тождественно и спрашивать о нём было бы нечего.
        let run: Vec<usize> = (1..sweeps.len())
            .map(|step| (from + step) % sweeps.len())
            .take_while(|index| *index != to)
            .collect();
        let Some((&first, &last)) = run.first().zip(run.last()) else {
            continue;
        };
        let (opens, closes) = (places[first], places[(last + 1) % places.len()]);
        if opens.0 != closes.0 || (opens.1 - closes.1).rem_euclid(360.0) != 0.0 {
            return Vec::new();
        }
        let span: f64 = run.iter().map(|index| sweeps[*index]).sum();
        match span.abs() >= geodesy::FULL_CIRCLE_DEG {
            true => carried = true,
            // Отросток: сам себя прошёл туда и обратно. Круга не обошёл, но и
            // против разреза не голосует.
            false if span == 0.0 => continue,
            false => return Vec::new(),
        }
    }
    match carried {
        true => cuts,
        false => Vec::new(),
    }
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
    // Шагов не больше, чем нужно ребру в полный круг: длиннее ребра не бывает,
    // а приведение к `u32` насыщается вместо переполнения — из числа, пришедшего
    // по шине, вышло бы четыре миллиарда шагов с вершиной на каждом.
    //
    // Предел ставится шагам, а не размаху: `f64::min` не-число пропускает
    // вторым доводом, и зажатый им размах дал бы полный круг там, где вести
    // нечего вовсе.
    let edge_deg = max_edge_deg();
    let most = (geodesy::TURN_DEG / edge_deg).ceil();
    let steps = (geodesy::edge_span(from, to) / edge_deg).ceil().max(1.0).min(most) as u32;
    let height_m = height_m();
    for step in 0..steps {
        let (lat_deg, lon_deg) = geodesy::edge_point(from, to, f64::from(step) / f64::from(steps));
        ring.push(mesh::vertex(Geodetic { lat_deg, lon_deg, height_m }));
    }
}

#[cfg(test)]
mod tests {

    /// Ребро, размах которого не с Земли, ведётся числом шагов, а не миллиардом.
    ///
    /// Числа приходят по шине, и приведение к `u32` насыщается вместо
    /// переполнения: незажатое, оно дало бы четыре миллиарда вершин. А ребро,
    /// размаха у которого нет вовсе, ведётся одним шагом — не полным кругом.
    #[test]
    fn an_edge_wider_than_the_earth_is_not_drawn_forever() {
        let mut endless = Vec::new();
        edge(&mut endless, (0.0, 0.0), (1.0e9, 0.0));
        let most = (geodesy::TURN_DEG / max_edge_deg()).ceil() as usize;
        assert_eq!(endless.len(), most, "вершин {}", endless.len());

        let mut nowhere = Vec::new();
        edge(&mut nowhere, (0.0, 0.0), (f64::NAN, 0.0));
        assert_eq!(nowhere.len(), 1, "вести нечего — один шаг");
    }

    use super::*;
    use crate::proto::globe::GeoPoint;

    fn outline(points: &[(f64, f64)]) -> Outline {
        Outline {
            points: points.iter().map(|&(lat, lon)| GeoPoint { lat, lon }).collect(),
            style: OutlineStyle::OutlinePlain as i32,
        }
    }

    /// Лента везёт соседей смещениями, и смещения эти — настоящие: `next`
    /// смотрит к следующей вершине по обходу, `prev` к предыдущей, и одно
    /// обратно другому. Перепутай их местами — и на экране не изменится ничего
    /// (обе стороны ленты симметричны, излом отражается целиком), а направление
    /// звена станет обратным. Тестов на саму ленту не было вовсе.
    #[test]
    fn a_ribbon_carries_true_offsets_to_its_neighbours() {
        let built =
            Outlines::build(&[styled(&[(0.0, 0.0), (0.0, 1.0), (1.0, 0.5)], OutlineStyle::OutlineSelected)]);
        // На каждый узел кольца — пара вершин, левый край и правый.
        let count = built.ribbon.len() / 2;
        assert!(count >= 3, "кольцо из {} узлов", count);

        let apart = |a: geodesy::World, b: geodesy::World| {
            f64::from((glam::Vec3::from(a) - glam::Vec3::from(b)).length()) * geodesy::SEMI_MAJOR_M
        };
        let at = |node: usize| {
            let vertex = built.ribbon[node * 2];
            (vertex.position, vertex.position_low)
        };
        for index in 0..count {
            let ahead = (index + 1) % count;
            let step = geodesy::offset(at(ahead), at(index));
            let span = f64::from(glam::Vec3::from(step).length()) * geodesy::SEMI_MAJOR_M;
            assert!(span > 1.0, "узел {}: звено в {} м — проверять нечего", index, span);

            assert!(apart(built.ribbon[index * 2].next, step) < 1e-3, "узел {}: next", index);
            // Обратный ход у соседа — то же звено, взятое назад.
            let back = geodesy::offset(at(index), at(ahead));
            assert!(apart(built.ribbon[ahead * 2].prev, back) < 1e-3, "узел {}: prev", ahead);
            // И обе копии узла везут одно и то же: в стороны их разводит шейдер.
            assert_eq!(built.ribbon[index * 2].next, built.ribbon[index * 2 + 1].next);
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
        let loops = ring(&band);
        assert_eq!(loops.len(), 2, "пояс — это две параллели, и петель у него две");

        let ring = loops.concat();
        let lons: Vec<f64> = ring.iter().map(|v| angles_of(v).1).collect();
        let east = lons.iter().filter(|lon| **lon > 60.0).count();
        let west = lons.iter().filter(|lon| **lon < -60.0).count();
        assert!(east > 30 && west > 30, "контур собрался в меридиан: {} восточных, {} западных", east, west);

        // А вершин на шве не осталось ни одной: боковых краёв у пояса нет, они
        // записаны только потому, что прямоугольник записан четырьмя углами.
        // Проверяется широтами, а не долготами: шов — это меридиан, и на нём
        // лежат все широты между параллелями.
        let stray = ring
            .iter()
            .map(|v| angles_of(v).0)
            .filter(|lat| (-69.0..=69.0).contains(lat))
            .count();
        assert_eq!(stray, 0, "на шве осталось {} вершин", stray);

        // И каждая петля — целая параллель: своя широта и весь круг долгот.
        for run in &loops {
            let lats: Vec<f64> = run.iter().map(|v| angles_of(v).0).collect();
            let spread = lats.iter().fold(f64::MIN, |a, b| a.max(*b))
                - lats.iter().fold(f64::MAX, |a, b| a.min(*b));
            assert!(spread < 0.1, "петля гуляет по широте на {}°", spread);
            let angles: Vec<(f64, f64)> = run.iter().map(angles_of).collect();
            assert!(geodesy::encircles(&angles), "петля не обошла Землю");
        }
    }

    /// Пояс режется одинаково, сколькими бы вершинами его ни записали.
    ///
    /// Тот же пояс, у которого параллели разбиты на четверти: полного круга нет
    /// теперь ни в одном ребре, и признак «ребро в круг» на нём бы отказал. А
    /// пробег между разрезами круг обходит по-прежнему — сумма от дробления не
    /// меняется. Ровно эту независимость от записи держит у щелчка
    /// `footprint::covers_a_band_written_with_extra_vertices`.
    #[test]
    fn a_band_is_cut_the_same_however_densely_it_is_written() {
        let dense = outline(&[
            (-70.0125, -180.0125),
            (70.0125, -180.0125),
            (70.0125, -90.0),
            (70.0125, 0.0),
            (70.0125, 90.0),
            (70.0125, 179.9875),
            (-70.0125, 179.9875),
            (-70.0125, 90.0),
            (-70.0125, 0.0),
            (-70.0125, -90.0),
        ]);
        let loops = ring(&dense);
        assert_eq!(loops.len(), 2, "гуще записанный пояс не разрезан");
        let stray = loops
            .concat()
            .iter()
            .map(|v| angles_of(v).0)
            .filter(|lat| (-69.0..=69.0).contains(lat))
            .count();
        assert_eq!(stray, 0, "на шве осталось {} вершин", stray);
    }

    /// А поясу с прорезью боковые рёбра — настоящий край, и резать их нельзя.
    ///
    /// Сетка, не сомкнувшаяся на десять градусов, Землю по долготе всё ещё
    /// обходит — меридиана, свободного от неё, нет. Но пробег круга не
    /// набирает, и выброшенные края замкнули бы каждую петлю хордой прямо
    /// через прорезь, то есть через то, чего в снимке нет.
    #[test]
    fn a_band_with_a_gap_keeps_its_edges() {
        let mut points = vec![(-70.0, 10.0), (70.0, 10.0)];
        for lon in [90.0, 170.0, -110.0, -30.0, 0.0] {
            points.push((70.0, lon));
        }
        points.push((-70.0, 0.0));
        for lon in [-30.0, -110.0, 170.0, 90.0] {
            points.push((-70.0, lon));
        }
        let gapped = outline(&points);

        let ring = ring(&gapped);
        assert_eq!(ring.len(), 1, "настоящий край выброшен как шов");
        let angles: Vec<(f64, f64)> = ring.concat().iter().map(angles_of).collect();
        assert!(geodesy::encircles(&angles), "проверять было бы нечего: круга нет");
    }

    /// Пояс режется одинаково, с какой бы вершины поставщик его ни начал.
    ///
    /// Обход начинается сразу после разреза, а не с головы списка: начатый с
    /// середины пробег распался бы надвое — его начало попало бы в одну петлю,
    /// а продолжение в другую, и параллель нарисовалась бы двумя дугами с
    /// хордой между ними.
    #[test]
    fn a_band_is_cut_the_same_wherever_its_list_begins() {
        let corners = [
            (-70.0125, -180.0125),
            (70.0125, -180.0125),
            (70.0125, -90.0),
            (70.0125, 0.0),
            (70.0125, 90.0),
            (70.0125, 179.9875),
            (-70.0125, 179.9875),
            (-70.0125, 90.0),
            (-70.0125, 0.0),
            (-70.0125, -90.0),
        ];
        for turn in 0..corners.len() {
            let mut points = corners.to_vec();
            points.rotate_left(turn);
            let loops = ring(&outline(&points));
            assert_eq!(loops.len(), 2, "начав с {}-й вершины, получили {} петель", turn, loops.len());
        }
    }

    /// Пробег, не вернувшийся в ту же точку шара, разрезом не считается.
    ///
    /// Шов — это переход с края на край, то есть то, что он соединяет, и без
    /// него одно место. Кольцо, обошедшее Землю по параллели и спустившееся на
    /// десять градусов, тоже имеет ребро без долготы — но это его настоящий
    /// край, и выброшенный, он стянул бы петлю хордой через нетронутое.
    #[test]
    fn a_run_that_does_not_close_is_no_seam() {
        let dented = outline(&[(70.0, 0.0), (70.0, 120.0), (70.0, -120.0), (60.0, -120.0)]);
        let loops = ring(&dented);
        assert_eq!(loops.len(), 1, "настоящий край выброшен как шов");

        // Ребро в десять градусов стои́т по меридиану −120°, и вершины на нём
        // лежат только у него: соседнее ребро уходит от той же точки на восток.
        let standing = loops
            .concat()
            .iter()
            .map(angles_of)
            .filter(|(lat, lon)| (60.5..69.5).contains(lat) && (lon + 120.0).abs() < 1e-6)
            .count();
        assert!(standing > 20, "ребро в десять градусов пропало: вершин {}", standing);
    }

    /// Повторённая угловая вершина ничего не меняет: между двумя разрезами
    /// подряд петли нет, и пустой пробег не мешает соседям.
    #[test]
    fn a_repeated_corner_does_not_split_the_band() {
        let doubled = outline(&[
            (-70.0125, -180.0125),
            (70.0125, -180.0125),
            (70.0125, -180.0125),
            (70.0125, 179.9875),
            (-70.0125, 179.9875),
        ]);
        let loops = ring(&doubled);
        assert_eq!(loops.len(), 2, "повторённый угол сбил разрез");
        let stray = loops
            .concat()
            .iter()
            .map(|v| angles_of(v).0)
            .filter(|lat| (-69.0..=69.0).contains(lat))
            .count();
        assert_eq!(stray, 0, "на шве осталось {} вершин", stray);
    }

    /// Заливки нет и у неразрезанного кольца, разошедшегося на пол-Земли.
    ///
    /// Пояс с прорезью петлёй остаётся одной — резать у него нечего, — а
    /// середины у него всё равно нет: веер из любой точки пошёл бы поверх
    /// обратной стороны. Держит это [`FILL_REACH`], и держать больше некому:
    /// у сомкнувшегося пояса до него дело не доходит, там раньше отвечают
    /// петли.
    #[test]
    fn a_gapped_band_is_whole_and_still_gets_no_fill() {
        let mut points = vec![(-70.0, 10.0), (70.0, 10.0)];
        for lon in [90.0, 170.0, -110.0, -30.0, 0.0] {
            points.push((70.0, lon));
        }
        points.push((-70.0, 0.0));
        for lon in [-30.0, -110.0, 170.0, 90.0] {
            points.push((-70.0, lon));
        }
        let gapped = outline(&points);
        assert_eq!(ring(&gapped).len(), 1, "проверять было бы нечего: кольцо разрезано");

        let built =
            Outlines::build(&[Outline { style: OutlineStyle::OutlinePending as i32, ..gapped }]);
        assert!(!built.indices.is_empty(), "контур пропал");
        assert!(built.hatch_indices.is_empty(), "веер построен поверх обратной стороны");
    }

    /// А честная шапка заливается, даже когда записана прямоугольником до
    /// полюса: вторая петля у неё вырождена в точку, рисуется одна, и середина
    /// у этой одной есть.
    #[test]
    fn a_cap_written_to_the_pole_keeps_its_fill() {
        let polar = outline(&[
            (60.0, -180.0125),
            (90.0, -180.0125),
            (90.0, 179.9875),
            (60.0, 179.9875),
        ]);
        let built =
            Outlines::build(&[Outline { style: OutlineStyle::OutlinePending as i32, ..polar }]);
        assert!(!built.hatch_indices.is_empty(), "шапка осталась незалитой");
    }

    /// Петля, выродившаяся в точку, не рисуется.
    ///
    /// У прямоугольника, упёршегося в полюс, верхняя параллель — сама точка:
    /// круг долгот там сходится в ноль дуги. Разрез такую петлю рождает
    /// законно, а рисовать её нечем — двумя вершинами замкнутой линии не
    /// выйдет.
    #[test]
    fn a_loop_shrunk_to_a_point_draws_nothing() {
        let polar = outline(&[
            (60.0, -180.0125),
            (90.0, -180.0125),
            (90.0, 179.9875),
            (60.0, 179.9875),
        ]);
        let loops = ring(&polar);
        assert_eq!(loops.len(), 2, "полюс не отрезан от параллели");
        assert!(loops.iter().any(|run| run.len() < 3), "вырожденной петли нет");

        let built = Outlines::build(&[Outline { style: OutlineStyle::OutlinePlain as i32, ..polar }]);
        let lats: Vec<f64> = built.vertices.iter().map(angles_of).map(|(lat, _)| lat).collect();
        assert!(!lats.is_empty(), "контур пропал целиком");
        assert!(lats.iter().all(|lat| *lat < 89.0), "вырожденная петля нарисована");
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
        let loops = ring(&box_);
        assert_eq!(loops.len(), 1, "обычная коробка разрезана");
        let ring = loops.concat();
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
        let loops = ring(&swath);
        assert_eq!(loops.len(), 1, "полоса съёмки разрезана");
        let ring = loops.concat();
        let north = ring
            .iter()
            .map(angles_of)
            .filter(|(_, lon)| (19.0..21.0).contains(lon))
            .map(|(lat, _)| lat)
            .fold(f64::MIN, f64::max);
        assert!(north > 73.3, "ребро пошло локсодромой: {}", north);
    }
}
