use aws_config::BehaviorVersion;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Credentials, Region};
use anyhow::Result;
use std::env;

#[derive(Debug)]
pub struct CopernicusSource {
    client: Client,
    bucket: String,
    access_token: tokio::sync::RwLock<Option<String>>,
}

impl CopernicusSource {
    pub async fn new() -> Result<Self> {
        let access_key = env::var("COPERNICUS_ACCESS_KEY")
            .map_err(|_| anyhow::anyhow!("COPERNICUS_ACCESS_KEY not set. Please set it in env or .env file"))?;
        let secret_key = env::var("COPERNICUS_ACCESS_SECRET")
            .map_err(|_| anyhow::anyhow!("COPERNICUS_ACCESS_SECRET not set. Please set it in env or .env file"))?;
        
        let credentials = Credentials::new(access_key, secret_key, None, None, "env");
        
        let config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new("cdse"))
            .endpoint_url("https://eodata.dataspace.copernicus.eu")
            .credentials_provider(credentials)
            .load()
            .await;
        
        // Force path style for CDSE
        let s3_config = aws_sdk_s3::config::Builder::from(&config)
            .force_path_style(true)
            .build();
        
        let client = Client::from_conf(s3_config);
        
        Ok(Self {
            client,
            bucket: "eodata".to_string(), 
            access_token: tokio::sync::RwLock::new(None),
        })
    }

    async fn get_access_token(&self) -> Result<String> {
        // Check cache first
        {
            let read_guard = self.access_token.read().await;
            if let Some(token) = &*read_guard {
                return Ok(token.clone());
            }
        }

        // Cache miss, fetch new token
        let mut write_guard = self.access_token.write().await;
        // Double check after acquiring write lock
        if let Some(token) = &*write_guard {
            return Ok(token.clone());
        }

        let username = env::var("COPERNICUS_USERNAME")
            .map_err(|_| anyhow::anyhow!("COPERNICUS_USERNAME not set. Please set it in .env file"))?;
        let password = env::var("COPERNICUS_PASSWORD")
            .map_err(|_| anyhow::anyhow!("COPERNICUS_PASSWORD not set. Please set it in .env file"))?;

        println!("Fetching new access token for CDSE...");
        let client = reqwest::Client::new();
        let params = [
            ("client_id", "cdse-public"),
            ("username", &username),
            ("password", &password),
            ("grant_type", "password"),
        ];

        let response = client
            .post("https://identity.dataspace.copernicus.eu/auth/realms/CDSE/protocol/openid-connect/token")
            .form(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            let err = response.text().await?;
            return Err(anyhow::anyhow!("Auth failed: {}", err));
        }

        let json: serde_json::Value = response.json().await?;
        let token = json["access_token"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No access_token in response"))?;
        
        let token_str = token.to_string();
        println!("ACCESS_TOKEN: {}", token_str);
        *write_guard = Some(token_str.clone());
        
        Ok(token_str)
    }

    pub async fn resolve_s3_path(&self, lat: i32, lon: i32) -> Result<Option<String>> {
        let token = self.get_access_token().await?;
        
        let lat_char = if lat >= 0 { 'N' } else { 'S' };
        let lon_char = if lon >= 0 { 'E' } else { 'W' };
        let lat_str = format!("{}{:02}", lat_char, lat.abs());
        let lon_str = format!("{}{:03}", lon_char, lon.abs());
        
        // Search for Copernicus_DSM which is the expected prefix
        let filter = "Collection/Name eq 'COP-DEM' and contains(Name,'Copernicus_DSM')".to_string();
        
        println!("Searching OData for Copernicus_DSM in COP-DEM...");
        
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0")
            .build()?;

        let url = "https://catalogue.dataspace.copernicus.eu/odata/v1/Products";
        
        let response = client.get(url)
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", token))
            .query(&[("$filter", filter.as_str()), ("$top", "1")])
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        println!("OData response status: {}", status);
        if !status.is_success() {
            return Err(anyhow::anyhow!("OData request failed: {} - {}", status, body));
        }

        let json: serde_json::Value = serde_json::from_str(&body)?;
        println!("OData response body: {}", body);
        let products = json["value"].as_array().ok_or_else(|| anyhow::anyhow!("Invalid OData response structure"))?;
        
        if products.is_empty() {
            println!("No products found in OData for filter: {}", filter);
            return Ok(None);
        }

        let s3_path = products[0]["S3Path"].as_str().map(|s| s.to_string());
        let cleaned_path = s3_path.map(|p| p.trim_start_matches("/eodata/").to_string());
        
        Ok(cleaned_path)
    }

                pub async fn search_grid_id(&self, grid_id: &str) -> Result<Vec<(String, String)>> {

                    let token = self.get_access_token().await?;

                    let url = "https://catalogue.dataspace.copernicus.eu/odata/v1/Products";

                    

                    // Filter specifically for COP-DEM collection and DGED (DEM) products

                    let filter = format!(

                        "Collection/Name eq 'COP-DEM' and contains(Name, 'DGE') and Attributes/OData.CSC.StringAttribute/any(att:att/Name eq 'gridId' and att/OData.CSC.StringAttribute/Value eq '{}')",

                        grid_id

                    );

            

        

                let client = reqwest::Client::new();

                let response = client.get(url)

                    .header("Accept", "application/json")

                    .header("Authorization", format!("Bearer {}", token))

                    .query(&[("$filter", filter.as_str()), ("$top", "50")])

                    .send()

                    .await?;

        

                if !response.status().is_success() {

                    let body = response.text().await?;

                    return Err(anyhow::anyhow!("OData error: {}", body));

                }

        

                let json: serde_json::Value = response.json().await?;

                let products = json["value"].as_array()

                    .ok_or_else(|| anyhow::anyhow!("Invalid response"))?;

        

                let mut results = Vec::new();

                for p in products {

                    if let (Some(name), Some(path)) = (p["Name"].as_str(), p["S3Path"].as_str()) {

                        results.push((name.to_string(), path.to_string()));

                    }

                }

                

                        Ok(results)

                

                    }

                

                

                

                        pub async fn list_product_files(&self, s3_path: &str) -> Result<Vec<String>> {

                

                

                

                            // Remove leading /eodata/ if present for S3 API

                

                

                

                            let mut prefix = s3_path.trim_start_matches("/eodata/").to_string();

                

                

                

                            

                

                

                

                            // If it looks like a file (has extension), we might want to list its "parent" or just use it as a prefix

                

                

                

                            println!("Listing S3 objects with prefix: '{}' in bucket: '{}'", prefix, self.bucket);

                

                

                

                            

                

                

                

                            let result = self.client

                

                

                

                                .list_objects_v2()

                

                

                

                                .bucket(&self.bucket)

                

                

                

                                .prefix(&prefix)

                

                

                

                                .send()

                

                

                

                                .await;

                

                

                

                    

                

                

                

                            match result {

                

                

                

                                Ok(output) => {

                

                

                

                                    let files: Vec<String> = output.contents()

                

                

                

                                        .iter()

                

                

                

                                        .filter_map(|obj| obj.key().map(|k| k.to_string()))

                

                

                

                                        .collect();

                

                

                

                                    println!("S3 found {} objects", files.len());

                

                

                

                                    Ok(files)

                

                

                

                                },

                

                

                

                                Err(err) => {

                

                

                

                                    let service_err = err.into_service_error();

                

                

                

                                    eprintln!("S3 List Error: {:?}", service_err);

                

                

                

                                    Err(anyhow::anyhow!("S3 Error: {:?}", service_err))

                

                

                

                                },

                

                

                

                            }

                

                

                

                        }

                

                

                

                    

                

                

                

                    /// Download a specific 1x1 degree tile to a local buffer

                

                

            pub async fn download_tile(&self, lat: i32, lon: i32) -> Result<Option<Vec<u8>>> {

                let s3_key = match self.resolve_s3_path(lat, lon).await? {

                    Some(path) => path,

                    None => {

                        println!("Tile not found in Copernicus catalog for Lat:{}, Lon:{}", lat, lon);

                        return Ok(None);

                    }

                };

        

                println!("Downloading s3://{}/{}...", self.bucket, s3_key);

        

                let result = self.client

                    .get_object()

                    .bucket(&self.bucket)

                    .key(&s3_key)

                    .send()

                    .await;

        

                match result {

                    Ok(output) => {

                        let bytes = output.body.collect().await?.into_bytes();

                        println!("Downloaded {} bytes", bytes.len());

                        Ok(Some(bytes.to_vec()))

                    },

                    Err(err) => {

                        let service_err = err.into_service_error();

                        println!("Error downloading {}: {:?}", s3_key, service_err);

                        Err(anyhow::anyhow!("S3 Error: {:?}", service_err))

                    }

                }

            }

        }

        

    