use async_trait::async_trait;
use std::fmt::Debug;

#[derive(Debug, Clone)]
pub struct DataProduct {
    pub name: String,
    pub path: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SearchFilter {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct ListResult {
    pub items: Vec<String>,
    pub next_token: Option<String>,
}

#[async_trait]
pub trait RemoteDataSource: Send + Sync + Debug {
    async fn search(&self, query: String, filters: Vec<SearchFilter>) -> Result<Vec<DataProduct>, String>;
    async fn list_path(&self, path: String, token: Option<String>) -> Result<ListResult, String>;
    // Убрали упоминание S3, теперь это просто идентификатор ресурса
    async fn download(&self, identifier: String, destination: String) -> Result<(), String>;
}
