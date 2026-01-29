use anyhow::Result;
use aws_config::BehaviorVersion;
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::Client;
use std::env;

#[derive(Debug)]
pub struct CopernicusSource {
    client: Client,
    bucket: String,
    access_token: tokio::sync::RwLock<Option<String>>,
}

impl CopernicusSource {
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
            access_token: tokio::sync::RwLock::new(None),
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

    // Returns (Name, S3Path, GridId)
    pub async fn fetch_full_product_index(&self) -> Result<Vec<(String, String)>> {
        let token = self.get_access_token().await?;
        let url = "https://catalogue.dataspace.copernicus.eu/odata/v1/Products";
        
        // Filter for all DEM products (both 30 and 90, DGED and DTED)
        // We look for 'COP-DEM' collection OR 'CCM' with 'DEM' in name
        let filter = "contains(Name, 'DEM1_SAR')";
        
        let mut all_products = Vec::new();
        let mut skip = 0;
        
        loop {
            println!("Fetching index page, skip={}...", skip);
            let response = reqwest::Client::new()
                .get(url)
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", token))
                .query(&[("$filter", filter), ("$top", "1000"), ("$skip", &skip.to_string())])
                .send()
                .await?;

            if !response.status().is_success() {
                break;
            }

            let json: serde_json::Value = response.json().await?;
            let products = json["value"].as_array();
            
            if let Some(list) = products {
                if list.is_empty() { break; }
                
                for p in list {
                    if let (Some(name), Some(path)) = (p["Name"].as_str(), p["S3Path"].as_str()) {
                        all_products.push((name.to_string(), path.to_string()));
                    }
                }
                
                if list.len() < 1000 { break; }
                skip += 1000;
                
                // Safety break for testing (remove later for full index)
                // if skip >= 5000 { break; } 
            } else {
                break;
            }
        }
        
        Ok(all_products)
    }

    pub async fn check_access(&self) -> Result<()> {
        let private_root = "auxdata/CopDEM/COP-DEM_GLO-30-DGED/";
        println!("Checking access to private root: {}", private_root);
        
        let result = self.client.list_objects_v2()
            .bucket(&self.bucket)
            .prefix(private_root)
            .max_keys(1)
            .send()
            .await;

        match result {
            Ok(_) => {
                println!("SUCCESS: Access to private folder allowed!");
                Ok(())
            },
            Err(e) => {
                let service_err = e.into_service_error();
                println!("ACCESS DENIED: {:?}", service_err);
                Err(anyhow::anyhow!("Access denied"))
            }
        }
    }

    pub async fn search_grid_id(&self, grid_id: &str) -> Result<Vec<(String, String, Option<String>)>> {
        let token = self.get_access_token().await?;
        let url = "https://catalogue.dataspace.copernicus.eu/odata/v1/Products";

        let filter = format!(
            "contains(Name, 'DGE') and Attributes/OData.CSC.StringAttribute/any(att:att/Name eq 'gridId' and att/OData.CSC.StringAttribute/Value eq '{}')",
            grid_id
        );

        println!("OData searching for gridId: {}", grid_id);

        let response = reqwest::Client::new()
            .get(url)
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {}", token))
            .query(&[("$filter", filter.as_str()), ("$top", "20"), ("$expand", "Attributes")])
            .send()
            .await?;

        let json: serde_json::Value = response.json().await?;
        let mut results = Vec::new();
        if let Some(products) = json["value"].as_array() {
            for p in products {
                if let (Some(name), Some(path)) = (p["Name"].as_str(), p["S3Path"].as_str()) {
                    // Try to extract gridId from attributes to be precise
                    let mut found_grid_id = None;
                    if let Some(attrs) = p["Attributes"].as_array() {
                        for attr in attrs {
                            if attr["Name"] == "gridId" {
                                found_grid_id = attr["Value"].as_str().map(|s| s.to_string());
                                break;
                            }
                        }
                    }
                    // Fallback to query grid_id if attribute not found (though unlikely with filter)
                    let gid = found_grid_id.or_else(|| Some(grid_id.to_string()));
                    
                    results.push((name.to_string(), path.to_string(), gid));
                }
            }
        }
        Ok(results)
    }

    pub async fn list_browser_path(&self, prefix: &str, token: Option<String>) -> Result<(Vec<String>, Option<String>)> {
        let mut req = self.client.list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .delimiter("/")
            .max_keys(20);
            
        if let Some(t) = token { req = req.continuation_token(t); }
        
        let res = req.send().await?;
        
        // Collect common prefixes (folders)
        let mut items: Vec<String> = res.common_prefixes()
            .iter()
            .filter_map(|cp| cp.prefix().map(|p| p.to_string()))
            .collect();
            
        // Also collect files (objects) at this level
        let files: Vec<String> = res.contents()
            .iter()
            .filter_map(|obj| obj.key().map(|k| k.to_string()))
            .collect();
            
        items.extend(files);
            
        let next = res.next_continuation_token().map(|t| t.to_string());
        Ok((items, next))
    }

    pub async fn find_tile_global(&self, grid_id: &str) -> Result<Option<String>> {
        let parts: Vec<&str> = grid_id.split('_').collect();
        if parts.len() != 2 { return Ok(None); }
        
        // We will search in both GLO-30 (10) and GLO-90 (30) public buckets
        let search_targets = [
            ("auxdata/CopDEM/COP-DEM_GLO-30-DGED_PUBLIC/", format!("Copernicus_DSM_10_{}_00_{}_00", parts[0], parts[1])),
            ("auxdata/CopDEM/COP-DEM_GLO-90-DGED_PUBLIC/", format!("Copernicus_DSM_30_{}_00_{}_00", parts[0], parts[1])),
        ];
        
        println!("Global Scan: Searching for {} in ALL public folders...", grid_id);
        
        for (public_root, tile_id) in search_targets {
            println!("Scanning root: {} for tile: {}", public_root, tile_id);
            
            let mut prefixes = Vec::new();
            let mut continuation_token = None;

            loop {
                let mut req = self.client.list_objects_v2()
                    .bucket(&self.bucket)
                    .prefix(public_root)
                    .delimiter("/");
                
                if let Some(token) = continuation_token {
                    req = req.continuation_token(token);
                }

                let resp = req.send().await?;
                
                let common_prefixes = resp.common_prefixes();
                for cp in common_prefixes {
                    if let Some(p) = cp.prefix() {
                        prefixes.push(p.to_string());
                    }
                }

                if resp.is_truncated.unwrap_or(false) {
                    continuation_token = resp.next_continuation_token().map(|s| s.to_string());
                } else {
                    break;
                }
            }

            println!("Scanning {} folders in {}...", prefixes.len(), public_root);

            use futures_util::StreamExt;

            let checks = futures_util::stream::iter(prefixes)
                .map(|prefix| {
                    let client = self.client.clone();
                    let bucket = self.bucket.clone();
                    let tid = tile_id.clone();
                    // Box::pin needed here
                    Box::pin(async move {
                        let path = format!("{}{}/DEM/{}_DEM.tif", prefix, tid, tid);
                        let resp = client.head_object().bucket(bucket).key(&path).send().await;
                        if resp.is_ok() { Some(path) } else { None }
                    })
                })
                .buffer_unordered(50); // Check 50 folders at a time

            // Return the first match
            let results: Vec<Option<String>> = checks.collect().await;
            let found = results.into_iter().flatten().next();

            if let Some(ref p) = found {
                println!("Global Scan SUCCESS: {}", p);
                return Ok(found);
            }
        }

        println!("Global Scan: Tile not found in any public folder.");
        Ok(None)
    }

    pub async fn list_product_files(&self, s3_path: &str, grid_id: Option<&str>) -> Result<Vec<String>> {
        let clean_path = s3_path.trim_start_matches("/eodata/").to_string();
        println!("Listing S3: '{}'", clean_path);
        
        // 1. Try direct list
        let mut files = self.fetch_keys(&clean_path).await?;

        // 2. Smart Lookup in PUBLIC
        if files.is_empty() {
            if let Some(gid) = grid_id {
                println!("Searching PUBLIC for gridId: {}", gid);
                let parts: Vec<&str> = gid.split('_').collect();
                if parts.len() == 2 {
                    let tile_id = format!("Copernicus_DSM_10_{}_00_{}_00", parts[0], parts[1]);
                    println!("Constructed tile_id: {}", tile_id);
                    
                    if let Some(product_name) = clean_path.split('/').last() {
                        // Product Name: DEM1_SAR_DGE_30_START_END_...
                        // Extract unique time range: START_END
                        // Example: 20110113T130607_20130428T130703
                        
                        let unique_part = if product_name.len() > 40 {
                            let p_parts: Vec<&str> = product_name.split('_').collect();
                            if p_parts.len() > 5 {
                                // parts[4] is start time, parts[5] is end time
                                format!("{}_{}", p_parts[4], p_parts[5])
                            } else { "".to_string() }
                        } else { "".to_string() };

                        if !unique_part.is_empty() {
                            println!("Smart Search: Looking for unique range {} in PUBLIC...", unique_part);
                            // We use the start DATE as prefix to narrow down S3 listing, then filter by unique_part
                            let date_prefix = &unique_part[0..8]; // YYYYMMDD
                            
                        // Construct exact path
                        let search_roots = [
                            "auxdata/CopDEM/COP-DEM_GLO-30-DGED_PUBLIC/",
                            "auxdata/CopDEM/COP-DEM_GLO-90-DGED_PUBLIC/",
                            // Private folders (full archive)
                            "auxdata/CopDEM/COP-DEM_GLO-30-DGED/",
                            "auxdata/CopDEM/COP-DEM_GLO-90-DGED/",
                        ];

                        for root in search_roots {
                                // Determine if we need DGE_30 or DGE_90 based on the root path or product name
                                let res_type = if root.contains("GLO-90") { "90" } else { "30" };
                                
                                // Only search if the product name matches the resolution of the folder we are checking
                                if product_name.contains(&format!("DGE_{}", res_type)) {
                                    let search_prefix = format!("{}DEM1_SAR_DGE_{}_{}", root, res_type, date_prefix);
                                    println!("Searching S3 prefix: {}", search_prefix);
                                    
                                    let list = self.client.list_objects_v2()
                                        .bucket(&self.bucket)
                                        .prefix(&search_prefix)
                                        .delimiter("/")
                                        .send()
                                        .await?;

                                    for cp in list.common_prefixes() {
                                        if let Some(p) = cp.prefix() {
                                            println!("  Checking candidate: {}", p);
                                                                                    // Now check if this folder contains our full unique timestamp range
                                                                                    if p.contains(&unique_part) {
                                                                                        println!("  MATCH! {}", p);
                                                                                        // Construct exact expected path to TIF
                                                                                        
                                                                                        let tile_id_fixed = if res_type == "90" {
                                                                                            tile_id.replace("Copernicus_DSM_10", "Copernicus_DSM_30")
                                                                                        } else {
                                                                                            tile_id.clone()
                                                                                        };
                                            
                                                                                        let tile_path = format!("{}{}/DEM/{}_DEM.tif", p, tile_id_fixed, tile_id_fixed);
                                                                                        println!("  Checking file existence: {}", tile_path);
                                                                                        
                                                                                        if self.head_object(&tile_path).await {
                                                                                            println!("  >>> FOUND: {}", tile_path);
                                                                                            return Ok(vec![tile_path]);
                                                                                        } else {
                                                                                            println!("  --- Tile missing in this product folder. Listing all files...");
                                                                                            // Fallback: List ALL files in this matching folder so the user sees what's inside
                                                                                            let all_files = self.fetch_keys(p).await?;
                                                                                            if !all_files.is_empty() {
                                                                                                return Ok(all_files);
                                                                                            }
                                                                                        }
                                                                                    }
                                             else {
                                                println!("  No match for unique part: {}", unique_part);
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            println!("Could not extract time range from product name: {}", product_name);
                        }
                    }
                    
                    // 3. Fallback: Global Scan (Brute Force)
                    // If we are here, smart search failed. Let's try to find it anywhere in PUBLIC.
                    if let Ok(Some(global_path)) = self.find_tile_global(gid).await {
                        return Ok(vec![global_path]);
                    }
                }
            } else {
                println!("No gridId provided for smart lookup");
            }
        }
        
        Ok(files)
    }

    async fn fetch_keys(&self, prefix: &str) -> Result<Vec<String>> {
        println!("DEBUG: list_objects_v2 bucket={} prefix={}", self.bucket, prefix);
        let result = self.client.list_objects_v2().bucket(&self.bucket).prefix(prefix).send().await;
        
        match result {
            Ok(output) => {
                let keys: Vec<String> = output.contents()
                    .iter()
                    .filter_map(|o| o.key().map(|k| k.to_string()))
                    .collect();
                println!("DEBUG: Found {} keys", keys.len());
                Ok(keys)
            },
            Err(e) => {
                let service_err = e.into_service_error();
                println!("DEBUG: S3 List Error: {:?}", service_err);
                // Return empty to allow fallback, but we logged the error now
                Ok(Vec::new())
            }
        }
    }

    async fn head_object(&self, key: &str) -> bool {
        self.client.head_object().bucket(&self.bucket).key(key).send().await.is_ok()
    }

    pub async fn resolve_s3_path(&self, _lat: i32, _lon: i32) -> Result<Option<String>> { Ok(None) }
    
    pub async fn download_file(&self, s3_key: &str, destination: &std::path::Path) -> Result<()> {
        let clean_key = s3_key.trim_start_matches("/eodata/");
        println!("Downloading {} to {:?}", clean_key, destination);

        let result = self.client.get_object()
            .bucket(&self.bucket)
            .key(clean_key)
            .send()
            .await?;

        let len = result.content_length.unwrap_or(0);
        println!("DEBUG: Content-Length: {} bytes", len);

        if len == 0 {
            println!("WARNING: Downloading 0-byte file. Is this a directory?");
        }

        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut body = result.body.into_async_read();
        let mut file = tokio::fs::File::create(destination).await?;
        
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buffer = [0u8; 8192];
        let mut total_bytes = 0;
        
        loop {
            let n = body.read(&mut buffer).await?;
            if n == 0 { break; }
            file.write_all(&buffer[0..n]).await?;
            total_bytes += n;
            if total_bytes > 0 && total_bytes % (5 * 1024 * 1024) == 0 {
                print!("\rDownloaded: {:.2} MB", total_bytes as f64 / 1024.0 / 1024.0);
                use std::io::Write;
                std::io::stdout().flush().unwrap();
            }
        }
        println!("\nDownload complete: {} bytes written", total_bytes);

        Ok(())
    }

        pub fn generate_preview(file_path: &std::path::Path) -> Result<Vec<u8>> {

            use std::fs::File;

            use tiff::decoder::{Decoder, DecodingResult};

            use image::{ImageBuffer, Rgba};

            use std::io::Cursor;

    

            let file = File::open(file_path)?;

            let mut decoder = Decoder::new(file)?;

            let (w, h) = decoder.dimensions()?;

            

            let data = match decoder.read_image()? {

                DecodingResult::F32(v) => v,

                DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),

                _ => return Err(anyhow::anyhow!("Unsupported TIFF format")),

            };

    

            // Normalize data for visualization

            let mut min_val = f32::MAX;

            let mut max_val = f32::MIN;

            

            for &val in &data {

                if val > -10000.0 && val < 10000.0 { // Filter nodata

                    if val < min_val { min_val = val; }

                    if val > max_val { max_val = val; }

                }

            }

            

            if min_val >= max_val { max_val = min_val + 1.0; }

            

            let range = max_val - min_val;

            let mut img_buf: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(w, h);

    

            for (i, val) in data.iter().enumerate() {

                let x = (i as u32) % w;

                let y = (i as u32) / w;

                

                let pixel = if *val <= -10000.0 {

                    Rgba([0, 0, 0, 0]) // Transparent nodata

                } else {

                    let norm = (*val - min_val) / range;

                    let v = (norm * 255.0) as u8;

                    Rgba([v, v, v, 255]) // Grayscale

                };

                

                img_buf.put_pixel(x, y, pixel);

            }

    

            let mut png_data = Vec::new();

            img_buf.write_to(&mut Cursor::new(&mut png_data), image::ImageFormat::Png)?;

            

            Ok(png_data)

        }

    

        pub async fn download_tile(&self, _lat: i32, _lon: i32) -> Result<Option<Vec<u8>>> { Ok(None) }

    }

    