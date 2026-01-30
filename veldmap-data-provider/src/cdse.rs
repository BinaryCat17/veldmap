use anyhow::Result;
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::Client;
use std::env;
use log::info;
use veldmap_core::{RemoteDataSource, DataProduct, SearchFilter};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct CdseDataSource {
    client: Client,
    bucket: String,
    access_token: Arc<RwLock<Option<String>>>,
}

impl CdseDataSource {
    pub async fn new() -> Result<Self> {
        let access_key = env::var("COPERNICUS_ACCESS_KEY")?;
        let secret_key = env::var("COPERNICUS_ACCESS_SECRET")?;
        let credentials = Credentials::new(access_key, secret_key, None, None, "env");

        let config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new("cdse"))
            .endpoint_url("https://eodata.dataspace.copernicus.eu")
            .credentials_provider(credentials)
            .load()
            .await;

        let s3_config = aws_sdk_s3::config::Builder::from(&config)
            .force_path_style(true)
            .build();

        Ok(Self {
            client: Client::from_conf(s3_config),
            bucket: "eodata".to_string(),
            access_token: Arc::new(RwLock::new(None)),
        })
    }

    async fn get_access_token(&self) -> Result<String> {
        if let Some(token) = &*self.access_token.read().await {
            return Ok(token.clone());
        }

        let mut write_guard = self.access_token.write().await;
        if let Some(token) = &*write_guard {
            return Ok(token.clone());
        }

        let username = env::var("COPERNICUS_USERNAME")?;
        let password = env::var("COPERNICUS_PASSWORD")?;

        let response = reqwest::Client::new()
            .post("https://identity.dataspace.copernicus.eu/auth/realms/CDSE/protocol/openid-connect/token")
            .form(&[
                ("client_id", "cdse-public"),
                ("username", &username),
                ("password", &password),
                ("grant_type", "password"),
            ])
            .send()
            .await?;

        let json: serde_json::Value = response.json().await?;
        let token = json["access_token"].as_str().ok_or_else(|| anyhow::anyhow!("No access token"))?.to_string();
        *write_guard = Some(token.clone());
        Ok(token)
    }
}

#[async_trait]
impl RemoteDataSource for CdseDataSource {
    async fn search(&self, query: String, filters: Vec<SearchFilter>) -> Result<Vec<DataProduct>, String> {
        let token = self.get_access_token().await.map_err(|e| e.to_string())?;
        let url = "https://catalogue.dataspace.copernicus.eu/odata/v1/Products";

        let mut filter_parts = Vec::new();
        if !query.is_empty() {
            filter_parts.push(format!("contains(Name, '{}')", query));
        }

        for f in filters {
            match f.name.as_str() {
                "gridId" => filter_parts.push(format!("Attributes/OData.CSC.StringAttribute/any(att:att/Name eq 'gridId' and att/OData.CSC.StringAttribute/Value eq '{}')", f.value)),
                "Collection" => filter_parts.push(format!("Collection/Name eq '{}'", f.value)),
                _ => {}
            }
        }

        let filter_str = filter_parts.join(" and ");
        info!("CDSE OData Filter: {}", filter_str);

        let response = reqwest::Client::new()
            .get(url)
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", token))
            .query(&[("$filter", filter_str.as_str()), ("$top", "50"), ("$expand", "Attributes")])
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
        let mut results = Vec::new();
        if let Some(products) = json["value"].as_array() {
            for p in products {
                if let (Some(name), Some(path)) = (p["Name"].as_str(), p["S3Path"].as_str()) {
                    let mut found_grid_id = None;
                    if let Some(attrs) = p["Attributes"].as_array() {
                        for attr in attrs {
                            if attr["Name"] == "gridId" {
                                found_grid_id = attr["Value"].as_str().map(|s| s.to_string());
                                break;
                            }
                        }
                    }
                    results.push(DataProduct {
                        name: name.to_string(),
                        path: path.to_string(),
                        grid_id: found_grid_id,
                    });
                }
            }
        }
        Ok(results)
    }

    async fn list_path(&self, prefix: String, token: Option<String>) -> Result<veldmap_core::ListResult, String> {
        let mut req = self.client.list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .delimiter("/")
            .max_keys(50);

        if let Some(t) = token { req = req.continuation_token(t); }

        let res = req.send().await.map_err(|e| e.to_string())?;

        let mut items: Vec<String> = res.common_prefixes().iter()
            .filter_map(|cp| cp.prefix().map(|p| p.to_string()))
            .collect();

        let files: Vec<String> = res.contents().iter()
            .filter_map(|obj| obj.key().map(|k| k.to_string()))
            .collect();

        items.extend(files);
        let next = res.next_continuation_token().map(|t| t.to_string());
        Ok(veldmap_core::ListResult { items, next_token: next })
    }

    async fn download(&self, key: String, destination: String) -> Result<(), String> {
        let clean_key = key.trim_start_matches("/eodata/");
        let result = self.client.get_object()
            .bucket(&self.bucket)
            .key(clean_key)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let dest_path = std::path::Path::new(&destination);
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let mut body = result.body.into_async_read();
        let mut file = tokio::fs::File::create(dest_path).await.map_err(|e| e.to_string())?;

        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buffer = [0u8; 16384];
        while let Ok(n) = body.read(&mut buffer).await {
            if n == 0 { break; }
            file.write_all(&buffer[0..n]).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
