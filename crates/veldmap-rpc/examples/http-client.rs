use std::error::Error;
use std::io::{self, Write};

use wasip3::http::types::{Fields, Method, Request, RequestOptions, Response, Scheme};
use wasip3::wit_bindgen::StreamResult;

const CHUNK_SIZE: usize = 4;
const RANGE_BODY: &[u8] = b"0123456789abcdef";

type ExampleResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExampleResult<()> {
    execute().await
}

async fn execute() -> ExampleResult<()> {
    let origin = std::env::args()
        .nth(1)
        .ok_or("usage: http-client <origin> <denied-origin>")?;
    let denied_origin = std::env::args().nth(2).ok_or("missing denied origin")?;
    let range = get(&origin, "/range", false)?;
    if range.get_status_code() != 206 {
        return Err(format!("range status was {}", range.get_status_code()).into());
    }
    let body = read_body(range, None).await?;
    if body != RANGE_BODY[4..12] {
        return Err("range response body did not match".into());
    }
    marker("HTTP_TLS_RANGE_STREAM_OK")?;
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    })
    .await?;
    marker("HTTP_TOKIO_TIMER_OK")?;

    match get(&origin, "/redirect", true) {
        Err(error) if error.to_string() == "signed request rejected HTTP 302" => {
            marker("HTTP_REDIRECT_REJECTED")?
        }
        Err(error) => return Err(error),
        Ok(_) => return Err("signed request did not reject its redirect".into()),
    }

    let cancelling = get(&origin, "/cancel", false)?;
    drop(read_body(cancelling, Some(1)).await?);
    marker("HTTP_BODY_DROPPED")?;

    match get(&denied_origin, "/", false) {
        Err(error)
            if matches!(
                error.downcast_ref::<wasip3::http::types::ErrorCode>(),
                Some(wasip3::http::types::ErrorCode::HttpRequestDenied)
            ) =>
        {
            marker("HTTP_EGRESS_DENIED")?
        }
        Err(error) => {
            return Err(format!("denied egress failed for another reason: {error:?}").into());
        }
        Ok(_) => return Err("host allowed a denied egress origin".into()),
    }
    Ok(())
}

fn get(origin: &str, path: &str, signed: bool) -> ExampleResult<Response> {
    wasip3::wit_bindgen::block_on(async {
        let (scheme, authority) = origin
            .split_once("://")
            .ok_or("origin must contain a scheme")?;
        let scheme = match scheme {
            "https" => Scheme::Https,
            "http" => Scheme::Http,
            _ => return Err("origin scheme must be HTTP or HTTPS".into()),
        };
        let mut headers = vec![("range".to_owned(), b"bytes=4-11".to_vec())];
        if signed {
            headers.push((
                "authorization".to_owned(),
                b"AWS4-HMAC-SHA256 test".to_vec(),
            ));
        }
        let headers = Fields::from_list(&headers)?;
        let (trailers_writer, trailers_reader) = wasip3::wit_future::new(|| Ok(None));
        let options = RequestOptions::new();
        options.set_connect_timeout(Some(5_000_000_000))?;
        options.set_first_byte_timeout(Some(5_000_000_000))?;
        options.set_between_bytes_timeout(Some(5_000_000_000))?;
        let (request, transmission) = Request::new(headers, None, trailers_reader, Some(options));
        drop(trailers_writer);
        drop(transmission);
        request
            .set_method(&Method::Get)
            .map_err(|()| "method rejected")?;
        request
            .set_scheme(Some(&scheme))
            .map_err(|()| "scheme rejected")?;
        request
            .set_authority(Some(authority))
            .map_err(|()| "authority rejected")?;
        request
            .set_path_with_query(Some(path))
            .map_err(|()| "path rejected")?;
        let response = wasip3::http::client::send(request).await?;
        let status = response.get_status_code();
        if signed && (300..400).contains(&status) {
            return Err(format!("signed request rejected HTTP {status}").into());
        }
        if !signed && status != 200 && status != 206 {
            return Err(format!("unexpected HTTP status {status}").into());
        }
        Ok(response)
    })
}

async fn read_body(response: Response, stop_after: Option<usize>) -> ExampleResult<Vec<u8>> {
    let (mut body, trailers, result_writer) = wasip3::wit_bindgen::block_on(async move {
        let (result_writer, result_reader) = wasip3::wit_future::new(|| Ok(()));
        let (body, trailers) = Response::consume_body(response, result_reader);
        (body, trailers, result_writer)
    });
    let mut bytes = Vec::new();
    let mut chunks = 0;
    loop {
        let (result, chunk) =
            wasip3::wit_bindgen::block_on(body.read(Vec::with_capacity(CHUNK_SIZE)));
        match result {
            StreamResult::Complete(_) => {
                chunks += 1;
                bytes.extend(chunk);
                if stop_after == Some(chunks) {
                    wasip3::wit_bindgen::block_on(async move {
                        drop(body);
                        drop(trailers);
                        drop(result_writer);
                    });
                    return Ok(bytes);
                }
                tokio::task::yield_now().await;
            }
            StreamResult::Dropped => {
                wasip3::wit_bindgen::block_on(async move {
                    let result = trailers.await;
                    drop(result_writer);
                    result
                })?;
                if chunks < 2 && stop_after.is_none() {
                    return Err("response did not exercise streaming reads".into());
                }
                return Ok(bytes);
            }
            StreamResult::Cancelled => {
                wasip3::wit_bindgen::block_on(async move {
                    drop(body);
                    drop(trailers);
                    drop(result_writer);
                });
                return Err("response body was cancelled".into());
            }
        }
    }
}

fn marker(value: &str) -> io::Result<()> {
    println!("{value}");
    io::stdout().flush()
}
