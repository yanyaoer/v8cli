use serde_json::json;
use std::sync::OnceLock;
use std::time::Duration;

static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

fn client() -> &'static reqwest::blocking::Client {
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .cookie_store(true)
            .timeout(Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (compatible; v8cli/0.1; agent-browser-lite)")
            .build()
            .expect("http client")
    })
}

/// req_json: {"url", "method"?, "headers"?, "body"?}
/// returns: {"status", "url", "headers", "body"} or {"error"}
pub fn fetch(req_json: &str) -> String {
    match do_fetch(req_json) {
        Ok(v) => v.to_string(),
        Err(e) => json!({ "error": e }).to_string(),
    }
}

fn do_fetch(req_json: &str) -> Result<serde_json::Value, String> {
    let req: serde_json::Value = serde_json::from_str(req_json).map_err(|e| e.to_string())?;
    let url = req["url"].as_str().ok_or("url required")?;
    let method: reqwest::Method = req["method"]
        .as_str()
        .unwrap_or("GET")
        .to_uppercase()
        .parse()
        .map_err(|_| "invalid method".to_string())?;

    let mut rb = client().request(method, url);
    if let Some(headers) = req["headers"].as_object() {
        for (k, v) in headers {
            if let Some(v) = v.as_str() {
                rb = rb.header(k, v);
            }
        }
    }
    if let Some(body) = req["body"].as_str() {
        rb = rb.body(body.to_string());
    }

    let resp = rb.send().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let final_url = resp.url().to_string();
    let mut headers = serde_json::Map::new();
    for (k, v) in resp.headers() {
        headers.insert(k.as_str().into(), v.to_str().unwrap_or("").into());
    }
    let body = resp.text().map_err(|e| e.to_string())?;
    Ok(json!({ "status": status, "url": final_url, "headers": headers, "body": body }))
}
