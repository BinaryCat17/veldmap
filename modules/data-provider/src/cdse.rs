use extism_pdk::*;
use veldmap_rust_rpc::dataprovider::{SearchRequest, SearchResponse, DownloadRequest, DownloadResponse, ListPathRequest, ListPathResponse, DataProduct};
use veldmap_rust_rpc::host::host_log;
use serde_json::Value;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

pub fn search(request: SearchRequest) -> anyhow::Result<SearchResponse> {
    let mut url = "https://catalogue.dataspace.copernicus.eu/odata/v1/Products".to_string();
    let mut params = Vec::new();

    if !request.query.is_empty() {
        params.push(format!("contains(Name,'{}')", request.query));
    }

    for filter in request.filters {
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
    url.push_str("&$top=50&$expand=Attributes");

    let req = HttpRequest::new(url);
    let res = http::request::<()>(&req, None)?;

    if res.status() != 200 {
        return Err(anyhow::anyhow!("CDSE OData API returned status {}", res.status()));
    }

    let body = res.body();
    let json: Value = serde_json::from_slice(&body)?;

    let products: Vec<DataProduct> = json["value"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("No value in response"))?
        .iter()
        .map(|item| {
            let mut grid_id = String::new();
            if let Some(attrs) = item["Attributes"].as_array() {
                for attr in attrs {
                    if attr["Name"] == "gridId" {
                        grid_id = attr["Value"].as_str().unwrap_or("").to_string();
                        break;
                    }
                }
            }
            DataProduct {
                name: item["Name"].as_str().unwrap_or("Unknown").to_string(),
                path: item["S3Path"].as_str().unwrap_or("").to_string(),
                timestamp: item["ContentDate"]["Start"].as_str().unwrap_or("").to_string(),
                grid_id,
            }
        })
        .collect();

    Ok(SearchResponse {
        products,
        error: String::new(),
    })
}

pub fn download(request: DownloadRequest) -> anyhow::Result<DownloadResponse> {
    let download_url = format!(
        "https://catalogue.dataspace.copernicus.eu/odata/v1/Products({})/$value",
        request.identifier
    );

    Ok(DownloadResponse {
        success: true,
        error: String::new(),
        download_url,
    })
}

pub fn list_path(request: ListPathRequest) -> anyhow::Result<ListPathResponse> {
    let mut items = Vec::new();
    let prefix = request.path.trim_start_matches('/').to_string();
    
    if prefix.is_empty() {
        items.push("Sentinel-1/".to_string());
        items.push("Sentinel-2/".to_string());
        items.push("Sentinel-3/".to_string());
        items.push("Copernicus-DEM/".to_string());
        return Ok(ListPathResponse { items, next_token: String::new(), error: String::new() });
    }

    let url = format!(
        "https://eodata.dataspace.copernicus.eu/eodata/?list-type=2&delimiter=/&prefix={}",
        urlencoding::encode(&prefix)
    );

    host_log(&format!("S3 List: {}", url));
    let mut req = HttpRequest::new(url);
    req = req.with_header("User-Agent", "VeldMap/0.1.0");
    
    let res = http::request::<()>(&req, None)?;
    let body = res.body();

    if res.status() != 200 && res.status() != 0 {
        return Ok(ListPathResponse {
            items: Vec::new(),
            next_token: String::new(),
            error: format!("S3 status {}: {}", res.status(), String::from_utf8_lossy(&body)),
        });
    }

    let mut reader = Reader::from_reader(body.as_slice());
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut current_tag = String::new();
    let mut in_common_prefixes = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "CommonPrefixes" { in_common_prefixes = true; }
                current_tag = name;
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape()?.into_owned();
                if text.is_empty() || text == prefix || text == format!("{}/", prefix) {
                    continue;
                }
                match current_tag.as_str() {
                    "Prefix" if in_common_prefixes => {
                        items.push(text);
                    }
                    "Key" => {
                        items.push(text);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name_bytes = e.local_name();
                let name = String::from_utf8_lossy(name_bytes.as_ref());
                if name == "CommonPrefixes" { in_common_prefixes = false; }
                current_tag.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("XML parse error: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    host_log(&format!("Found {} items in S3", items.len()));
    Ok(ListPathResponse {
        items,
        next_token: String::new(),
        error: String::new(),
    })
}