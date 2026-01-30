mod cdse;

use std::sync::Arc;
use veldmap_core::data_provider_module::RemoteDataSource;

#[derive(Debug, Clone)]
pub struct CdseConfig {
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
    pub endpoint: String,
}

pub async fn create_cdse_provider(config: CdseConfig) -> anyhow::Result<Arc<dyn RemoteDataSource>> {
    Ok(Arc::new(cdse::CdseDataSource::new(config).await?))
}
