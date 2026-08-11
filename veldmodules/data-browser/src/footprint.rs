//! Контур снимка: где он лежит и что в него попадает.
//!
//! Две задачи, и обе про место, а не про рисование: навести на снимок камеру и
//! понять, в него ли ткнули. Рисующему модулю их не поручить — он не знает про
//! снимки вовсе; провайдеру тем более — он отдаёт геометрию и на этом всё.
//! Сводить одно с другим может только тот, у кого на экране и список, и шар.
//!
//! Считается на градусах, а не на эллипсоиде: контур приходит в EPSG:4326 и так
//! же понимается рисующим (см. globe/outlines.rs) — рёбра там прямые в широте с
//! долготой. «Внутри» здесь поэтому значит ровно то, что видно на экране, а
//! честный счёт по эллипсоиду разошёлся бы с картинкой.

use crate::proto::data_provider::Ring;

/// Круг, в который вписан контур.
pub struct Frame {
    pub lat: f64,
    pub lon: f64,
    /// Угловой радиус в градусах дуги: 1° — около 111 км.
    pub radius_deg: f64,
}

/// Куда смотреть, чтобы увидеть эти кольца целиком. `None` — колец нет: у
/// снимка не было геометрии, и наводить камеру не на что.
pub fn frame(rings: &[Ring]) -> Option<Frame> {
    let first = rings.iter().flat_map(|ring| &ring.points).next()?;

    let (mut south, mut north) = (first.lat, first.lat);
    let (mut west, mut east) = (0.0_f64, 0.0_f64);
    // Долготы разворачиваем относительно первой точки: контур у 180-го меридиана
    // каталог отдаёт разрезанным, и куски по обе стороны шва иначе усреднились
    // бы в точку на противоположной стороне Земли.
    let unwrapped: Vec<(f64, f64)> = rings
        .iter()
        .flat_map(|ring| &ring.points)
        .map(|point| (point.lat, wrap(point.lon - first.lon)))
        .collect();
    for &(lat, lon) in &unwrapped {
        south = south.min(lat);
        north = north.max(lat);
        west = west.min(lon);
        east = east.max(lon);
    }

    let lat = (south + north) * 0.5;
    let lon = first.lon + (west + east) * 0.5;
    // Радиус — по самой дальней вершине, а не по половине рамки: у контура,
    // вытянутого по диагонали, рамка вдвое уже того, что в него не влезло.
    let radius_deg = unwrapped
        .iter()
        .map(|&(vertex_lat, vertex_lon)| arc(lat, (west + east) * 0.5, vertex_lat, vertex_lon))
        .fold(0.0, f64::max);

    Some(Frame { lat, lon: wrap(lon), radius_deg })
}

/// Накрывают ли кольца эту точку.
///
/// Дырки полигона приходят кольцами наравне с внешним контуром, и здесь это
/// незаметно: точка в дырке лежит внутри обоих, а вопрос — «попал ли я в
/// снимок», и ответ на него от дырки не меняется.
pub fn covers(rings: &[Ring], lat: f64, lon: f64) -> bool {
    rings.iter().any(|ring| inside(ring, lat, lon))
}

/// Чётность пересечений: луч из точки на восток пересекает границу нечётное
/// число раз — точка внутри.
fn inside(ring: &Ring, lat: f64, lon: f64) -> bool {
    let mut inside = false;
    let points = &ring.points;
    for (index, from) in points.iter().enumerate() {
        let to = &points[(index + 1) % points.len()];
        // Всё считается от самой точки: так луч идёт вдоль нулевой широты, а
        // разворот долгот приходится на противоположный от неё меридиан — там,
        // где контура заведомо нет (он разрезан по 180-му, а точка внутри него).
        let (from_lon, from_lat) = (wrap(from.lon - lon), from.lat - lat);
        let (to_lon, to_lat) = (wrap(to.lon - lon), to.lat - lat);

        // Ребро должно пересекать широту точки — строго, чтобы вершина ровно на
        // ней не сосчиталась дважды.
        if (from_lat > 0.0) != (to_lat > 0.0) {
            let crossing = from_lon + (to_lon - from_lon) * (-from_lat) / (to_lat - from_lat);
            if crossing > 0.0 {
                inside = !inside;
            }
        }
    }
    inside
}

/// Дуга между двумя точками, градусы. По шару: на размере снимка сжатие
/// эллипсоида даёт доли процента, а нужно это число только затем, чтобы
/// отойти от снимка на расстояние, с которого он виден целиком.
fn arc(from_lat: f64, from_lon: f64, to_lat: f64, to_lon: f64) -> f64 {
    let (from, to) = (from_lat.to_radians(), to_lat.to_radians());
    let delta = (to_lon - from_lon).to_radians();
    (from.sin() * to.sin() + from.cos() * to.cos() * delta.cos())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

/// Разница долгот в −180..180 — та же сторона, с которой её видит глаз.
fn wrap(degrees: f64) -> f64 {
    degrees - 360.0 * ((degrees + 180.0) / 360.0).floor()
}
