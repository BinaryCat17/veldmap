//! Контуры на поверхности: ломаные из присланных вершин.
//!
//! Вершина здесь та же, что у Земли и сетки (`mesh::Vertex`), — точка с
//! нормалью, — поэтому и раскладка вершинного буфера у контуров общая с ними.
//! Отдельный буфер нужен не формату, а времени жизни: Земля строится раз, а
//! контуры меняются с каждым ответом на поиск.
//!
//! Все контуры лежат в одной паре буферов, а рисуются двумя вызовами: цветов у
//! них два — обычный и выделенный, — и это единственное, чем они друг от друга
//! отличаются. Отсюда и раскладка: сначала все обычные, потом все выделенные,
//! так что каждому вызову достаётся сплошной кусок индексов.

use std::ops::Range;

use crate::module::geodesy::Geodetic;
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
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Куски индексного буфера под каждый из двух вызовов. Выделенные —
    /// последние: глубину линии не пишут, и кто поверх кого, решает порядок
    /// отрисовки, а выделенный обязан быть виден поверх накрывших его соседей.
    pub plain: Range<u32>,
    pub picked: Range<u32>,
}

impl Outlines {
    pub fn build(outlines: &[Outline]) -> Self {
        let mut built =
            Self { vertices: Vec::new(), indices: Vec::new(), plain: 0..0, picked: 0..0 };

        for outline in outlines.iter().filter(|outline| !outline.selected) {
            built.push(outline);
        }
        built.plain = 0..built.indices.len() as u32;

        let picked_start = built.indices.len() as u32;
        for outline in outlines.iter().filter(|outline| outline.selected) {
            built.push(outline);
        }
        built.picked = picked_start..built.indices.len() as u32;

        built
    }

    /// Один замкнутый контур отрезками.
    fn push(&mut self, outline: &Outline) {
        // Замыкание кладём мы сами, поэтому повторённую последнюю точку
        // отбрасываем: иначе замыкающее ребро выродилось бы в точку.
        let points = match outline.points.as_slice() {
            [first, .., last] if first.lat == last.lat && first.lon == last.lon => {
                &outline.points[..outline.points.len() - 1]
            }
            all => all,
        };
        // Двумя точками контур не очертить: замкнутой линии там нет.
        if points.len() < 3 {
            return;
        }

        let base = self.vertices.len() as u32;
        for (index, point) in points.iter().enumerate() {
            let next = &points[(index + 1) % points.len()];
            self.edge((point.lat, point.lon), (next.lat, next.lon));
        }

        // Вершины `edge` кладёт подряд и конечную точку ребра не дублирует,
        // поэтому линия просто соединяет каждую с соседкой, а последнюю — с
        // первой: этим контур и замыкается.
        let count = self.vertices.len() as u32 - base;
        for i in 0..count {
            self.indices.extend_from_slice(&[base + i, base + (i + 1) % count]);
        }
    }

    /// Ребро от одной вершины до другой, уплотнённое до [`MAX_EDGE_DEG`].
    ///
    /// Промежуточные точки берутся линейной интерполяцией широты и долготы, а
    /// не дугой большого круга: контур в EPSG:4326 принято понимать именно так,
    /// и ровно так же параметризована поверхность (см. `mesh`). Конечная точка
    /// не кладётся — её положит следующее ребро, а последнее замкнёт контур.
    fn edge(&mut self, from: (f64, f64), to: (f64, f64)) {
        let (lat, lon) = from;
        // Долготу разворачиваем в короткую сторону: контур через 180-й меридиан
        // каталог отдаёт разрезанным, но пришедшее «с той стороны» ребро иначе
        // обежало бы Землю по длинной дуге поперёк всей карты.
        let mut dlon = to.1 - lon;
        if dlon > 180.0 {
            dlon -= 360.0;
        } else if dlon < -180.0 {
            dlon += 360.0;
        }
        let dlat = to.0 - lat;

        let span = dlat.abs().max(dlon.abs());
        let steps = (span / MAX_EDGE_DEG).ceil().max(1.0) as u32;
        for step in 0..steps {
            let t = step as f64 / steps as f64;
            self.point(lat + dlat * t, lon + dlon * t);
        }
    }

    fn point(&mut self, lat_deg: f64, lon_deg: f64) {
        self.vertices.push(mesh::vertex(Geodetic { lat_deg, lon_deg, height_m: HEIGHT_M }));
    }
}
