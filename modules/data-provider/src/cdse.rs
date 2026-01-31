use extism_pdk::*;
use veldmap_rust_rpc::services::{SearchRequest, SearchResponse, DownloadRequest, DownloadResponse};
use veldmap_rust_rpc::common::DataProduct;
use serde_json::Value;

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

    let req = HttpRequest::new(url);
    let res = http::request::<()>(&req, None)?;

    if res.status() != 200 {
        return Err(anyhow::anyhow!("CDSE API returned status {}", res.status()));
    }

    let body = res.body();
    let json: Value = serde_json::from_slice(&body)?;

    let products: Vec<DataProduct> = json["value"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("No value in response"))?
        .iter()
        .map(|item| {
            DataProduct {
                name: item["Name"].as_str().unwrap_or("Unknown").to_string(),
                path: item["S3Path"].as_str().unwrap_or("").to_string(),
                timestamp: item["ContentDate"]["Start"].as_str().unwrap_or("").to_string(),
            }
        })
        .collect();

    Ok(SearchResponse {
        products,
        error: String::new(),
        sync: None,
    })
}

pub fn download(request: DownloadRequest) -> anyhow::Result<DownloadResponse> {
    // Формируем прямую ссылку на скачивание продукта в CDSE
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