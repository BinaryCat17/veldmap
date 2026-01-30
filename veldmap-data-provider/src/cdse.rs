use async_trait::async_trait;
use aws_config::Region;
use aws_sdk_s3::{Client as S3Client, config::Credentials};
use log::{info, error};
use veldmap_core::data_provider_module::{RemoteDataSource, DataProduct, SearchFilter, ListResult};
use crate::CdseConfig;

#[derive(Debug)]
pub struct CdseDataSource {
    s3: S3Client,
}

impl CdseDataSource {
    pub async fn new(config: CdseConfig) -> anyhow::Result<Self> {
        let creds = Credentials::new(
            &config.access_key,
            &config.secret_key,
            None,
            None,
            "Static",
        );

        let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(Region::new(config.region))
            .credentials_provider(creds)
            .endpoint_url(config.endpoint)
            .load()
            .await;

        let s3 = S3Client::new(&sdk_config);
        Ok(Self { s3 })
    }
}

#[async_trait]
impl RemoteDataSource for CdseDataSource {
    async fn search(&self, query: String, filters: Vec<SearchFilter>) -> Result<Vec<DataProduct>, String> {
        let mut url = "https://catalogue.dataspace.copernicus.eu/odata/v1/Products".to_string();
        let mut params = Vec::new();

        if !query.is_empty() {
            params.push(format!("contains(Name,'{}')", query));
        }

        for filter in filters {
            if filter.name == "gridId" {
                params.push(format!("Attributes/OData.CSC.StringAttribute/any(att:att/Name eq 'gridId' and att/Value eq '{}')", filter.value));
            } else if filter.name == "Collection" {
                params.push(format!("Collection/Name eq '{}'", filter.value));
            }
        }

        if !params.is_empty() {
            url.push_str("?$filter=");
            url.push_str(&params.join(" and "));
        }

        let client = reqwest::Client::new();
        let res = client.get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error (OData): {}. Check VPN.", e))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| format!("JSON error: {}", e))?;

        let products = res["value"].as_array().ok_or("No value in response")?.iter().map(|item| {
            DataProduct {
                name: item["Name"].as_str().unwrap_or("Unknown").to_string(),
                path: item["S3Path"].as_str().unwrap_or("").to_string(),
                timestamp: item["ContentDate"]["Start"].as_str().map(|s| s.to_string()),
            }
        }).collect();

        Ok(products)
    }

    async fn list_path(&self, path: String, token: Option<String>) -> Result<ListResult, String> {
        let bucket = "eodata";
        let prefix = if path.is_empty() { String::new() } else if path.ends_with('/') { path } else { format!("{}/", path) };

        let mut req = self.s3.list_objects_v2()
            .bucket(bucket)
            .prefix(&prefix)
            .delimiter("/");

        if let Some(t) = token {
            req = req.continuation_token(t);
        }

        let res = req.send().await.map_err(|e| {
            error!("S3 List Error: {:?}", e);
            format!("S3 error: {}. Check VPN/Credentials.", e)
        })?;

        let mut items: Vec<String> = res.common_prefixes().iter()
            .filter_map(|cp| cp.prefix().map(|p| p.to_string()))
            .collect();

        let files: Vec<String> = res.contents().iter()
            .filter_map(|obj| obj.key().map(|k| k.to_string()))
            .collect();

        items.extend(files);
        let next = res.next_continuation_token().map(|t| t.to_string());
        Ok(ListResult { items, next_token: next })
    }

    async fn download(&self, identifier: String, destination: String) -> Result<(), String> {
        info!("Downloading {} to {}", identifier, destination);

        let (bucket, key) = if identifier.starts_with("s3://") {
            let parts: Vec<&str> = identifier[5..].splitn(2, '/').collect();
            (parts[0].to_string(), parts[1].to_string())
        } else {
            ("eodata".to_string(), identifier)
        };
        
        let res = self.s3.get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| format!("S3 download error: {}. Check VPN.", e))?;

        let mut body = res.body;
        let mut file = std::fs::File::create(destination).map_err(|e| e.to_string())?;

        use futures_util::StreamExt;
        while let Some(bytes) = body.next().await {
            let bytes = bytes.map_err(|e| e.to_string())?;
            std::io::Write::write_all(&mut file, &bytes).map_err(|e| e.to_string())?;
        }

        Ok(())
    }
}