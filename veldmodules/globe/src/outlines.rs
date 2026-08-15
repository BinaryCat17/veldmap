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

/// Ребро от одной вершины до другой, уплотнённое до [`MAX_EDGE_DEG`].
///
/// Промежуточные точки берутся по дуге, а не линейной интерполяцией широты и
/// долготы: контур здесь — это край снимка, а край снимка идёт по поверхности.
/// Прямая в градусах — локсодрома, и у гранулы Sentinel-1 (414 км на 73° с. ш.)
/// она отходит от снятого на 27 км: контур висел бы заметно в стороне от
/// собственного снимка, а на стыке соседних гранул между ними зиял бы серп.
/// Дуга по тем же углам ошибается на 0.6 км (см. `geodesy::between`).
///
/// Короткую сторону через антимеридиан дуга выбирает сама — разворачивать
/// долготу руками больше не нужно. Конечная точка не кладётся: её положит
/// следующее ребро, а последнее замкнёт контур.
fn edge(ring: &mut Vec<Vertex>, from: (f64, f64), to: (f64, f64)) {
    let steps = (geodesy::separation(from, to) / MAX_EDGE_DEG).ceil().max(1.0) as u32;
    for step in 0..steps {
        let (lat_deg, lon_deg) = geodesy::between(from, to, f64::from(step) / f64::from(steps));
        ring.push(mesh::vertex(Geodetic { lat_deg, lon_deg, height_m: HEIGHT_M }));
    }
}
