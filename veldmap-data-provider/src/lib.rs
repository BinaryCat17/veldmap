mod cdse;

use std::sync::Arc;
use veldmap_core::RemoteDataSource;

pub async fn create_cdse_provider() -> anyhow::Result<Arc<dyn RemoteDataSource>> {
    Ok(Arc::new(cdse::CdseDataSource::new().await?))
}