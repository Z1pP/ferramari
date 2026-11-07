//! Client for the first-party Ferramari Catalog API.
//!
//! The API is deliberately small and independent of the existing third-party
//! catalogue scraper. A deployment can host it separately; a plugin manifest
//! may point at any compatible endpoint. See `docs/CATALOG_API.md`.
//!
//! `ureq` is already a dependency for the winzoro catalogue, so this client
//! adds no new crate. Responses are size-limited and every URL is validated
//! before it reaches WebKit or mpv.

use std::io::Read;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const MAX_BODY_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ITEMS: usize = 200;
const MAX_URL_CHARS: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WallpaperItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub preview: String,
    pub download: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub next: Option<u32>,
    pub items: Vec<WallpaperItem>,
}

pub struct Client {
    base: String,
}

impl Client {
    pub fn new(base: &str) -> Result<Self, String> {
        let base = base.trim().trim_end_matches('/');
        if !(base.starts_with("https://") || base.starts_with("http://")) {
            return Err("catalogue API must be an http:// or https:// URL".to_string());
        }
        if base.chars().count() > MAX_URL_CHARS {
            return Err("catalogue API URL is too long".to_string());
        }
        Ok(Self {
            base: base.to_string(),
        })
    }

    pub fn list(
        &self,
        page: u32,
        query: Option<&str>,
        category: Option<&str>,
    ) -> Result<Page, String> {
        if page == 0 {
            return Err("page must be >= 1".to_string());
        }
        let mut url = format!("{}/wallpapers?page={page}", self.base);
        if let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) {
            url.push_str("&q=");
            url.push_str(&percent_encode(query));
        }
        if let Some(category) = category
            .map(str::trim)
            .filter(|category| !category.is_empty())
        {
            url.push_str("&category=");
            url.push_str(&percent_encode(category));
        }
        let response = ureq::get(&url)
            .timeout(Duration::from_secs(15))
            .set("User-Agent", "Ferramari/catalog-api")
            .call()
            .map_err(|err| format!("catalogue request failed: {err}"))?;
        if response.status() >= 400 {
            return Err(format!("catalogue returned HTTP {}", response.status()));
        }
        let mut body = Vec::new();
        response
            .into_reader()
            .take(MAX_BODY_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|err| format!("could not read catalogue response: {err}"))?;
        if body.len() as u64 > MAX_BODY_BYTES {
            return Err("catalogue response is larger than 2 MiB".to_string());
        }
        let mut page: Page = serde_json::from_slice(&body)
            .map_err(|err| format!("invalid catalogue JSON: {err}"))?;
        validate_page(&page)?;
        // Do not keep more than the protocol allows even if the server sends
        // a larger array.
        page.items.truncate(MAX_ITEMS);
        Ok(page)
    }
}

fn validate_page(page: &Page) -> Result<(), String> {
    if page.items.len() > MAX_ITEMS {
        return Err(format!("catalogue page has more than {MAX_ITEMS} items"));
    }
    for item in &page.items {
        if item.id.trim().is_empty() || item.id.chars().count() > 200 {
            return Err("catalogue item has an invalid id".to_string());
        }
        if item.name.trim().is_empty() || item.name.chars().count() > 300 {
            return Err("catalogue item has an invalid name".to_string());
        }
        if !is_http_url(&item.download) {
            return Err(format!(
                "catalogue item {} has a non-http download URL",
                item.id
            ));
        }
        if !item.preview.is_empty() && !is_http_url(&item.preview) {
            return Err(format!(
                "catalogue item {} has a non-http preview URL",
                item.id
            ));
        }
    }
    Ok(())
}

fn is_http_url(value: &str) -> bool {
    (value.starts_with("https://") || value.starts_with("http://"))
        && value.chars().count() <= MAX_URL_CHARS
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn serve_once(response: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
            request
        });
        (format!("http://{address}/v1"), handle)
    }

    #[test]
    fn fetches_and_validates_a_page() {
        let body = r#"{
            "page": 2,
            "next": 3,
            "items": [{
                "id": "neon",
                "name": "Neon City",
                "author": "A",
                "preview": "https://cdn.example/neon.jpg",
                "download": "https://cdn.example/neon.mp4",
                "tags": ["city", "neon"],
                "resolution": "1920x1080",
                "size": 123456
            }]
        }"#;
        let (base, request) = serve_once(body);
        let client = Client::new(&base).unwrap();
        let page = client.list(2, Some("neon city"), Some("anime")).unwrap();
        assert_eq!(page.page, 2);
        assert_eq!(page.next, Some(3));
        assert_eq!(page.items[0].id, "neon");
        assert_eq!(page.items[0].tags, vec!["city", "neon"]);
        let request = request.join().unwrap();
        assert!(request.contains("page=2"), "{request}");
        assert!(request.contains("q=neon%20city"), "{request}");
        assert!(request.contains("category=anime"), "{request}");
    }

    #[test]
    fn rejects_non_http_downloads_and_invalid_base_urls() {
        assert!(Client::new("file:///tmp/catalog").is_err());
        assert!(Client::new("https://example.org/v1").is_ok());
        let body = r#"{"items":[{"id":"x","name":"X","download":"file:///etc/passwd"}]}"#;
        let (base, request) = serve_once(body);
        let client = Client::new(&base).unwrap();
        assert!(client.list(1, None, None).is_err());
        request.join().unwrap();
    }

    #[test]
    fn percent_encoding_is_stable() {
        assert_eq!(percent_encode("neon city"), "neon%20city");
        assert_eq!(percent_encode("a/b?c"), "a%2Fb%3Fc");
    }
}
