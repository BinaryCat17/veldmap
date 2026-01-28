mod math;

use std::sync::Arc;
use veldmap_core::geo_math_module::GeoMath;
use crate::math::VeldmapGeoMath;

/// Фабрика для создания модуля географической математики.
pub fn create_geo_math() -> Arc<dyn GeoMath> {
    Arc::new(VeldmapGeoMath)
}
