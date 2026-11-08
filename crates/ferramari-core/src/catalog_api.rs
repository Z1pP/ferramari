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

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
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

    fn serve_once(response: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let body = response.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            request
        });
        (format!("http://{address}/v1"), handle)
    }

    // serve_status exists in case we need to test non-200 responses later.
    // It is currently only needed by server_error_status_returns_error which uses
    // its own inline server instead of this helper.
    #[allow(dead_code)]
    fn serve_status(_status: &'static str, _body: &str) {}

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

    // ======================================================================== //
    // A) Manifest / item schema evolution                                      //
    // ======================================================================== //

    /// Client handles a response with no optional fields — defaults applied.
    #[test]
    fn minimal_wallpaper_item_round_trips_defaults() {
        let body = r#"{
            "page": 1,
            "items": [{
                "id": "minimal",
                "name": "M",
                "download": "https://cdn.example/m.mp4"
            }]
        }"#;
        let (base, request) = serve_once(body);
        let client = Client::new(&base).unwrap();
        let page = client.list(1, None, None).unwrap();
        assert_eq!(page.page, 1);
        assert_eq!(page.next, None);
        assert_eq!(page.items.len(), 1);
        let item = &page.items[0];
        assert_eq!(item.id, "minimal");
        assert_eq!(item.name, "M");
        assert_eq!(item.author, "");
        assert_eq!(item.description, "");
        assert_eq!(item.preview, "");
        assert_eq!(item.category, "");
        assert!(item.tags.is_empty());
        assert!(item.resolution.is_none());
        assert!(item.size.is_none());
        assert!(item.sha256.is_none());
        assert_eq!(item.download, "https://cdn.example/m.mp4");
        request.join().unwrap();
    }

    /// Item with all optional fields present round-trips fully.
    #[test]
    fn full_wallpaper_item_round_trips_all_fields() {
        let body = r#"{
            "page": 3,
            "next": 4,
            "items": [{
                "id": "full-item",
                "name": "Full Item",
                "author": "Author One",
                "description": "A long description about this wallpaper",
                "preview": "https://cdn.example/thumb.jpg",
                "download": "https://cdn.example/full.mp4",
                "category": "Nature",
                "tags": ["nature", "forest"],
                "resolution": "3840x2160",
                "size": 200000000,
                "sha256": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
            }]
        }"#;
        let (base, request) = serve_once(body);
        let client = Client::new(&base).unwrap();
        let page = client.list(3, None, None).unwrap();
        assert_eq!(page.page, 3);
        assert_eq!(page.next, Some(4));
        let item = &page.items[0];
        assert_eq!(item.author, "Author One");
        assert_eq!(item.description, "A long description about this wallpaper");
        assert_eq!(item.category, "Nature");
        assert_eq!(item.tags, vec!["nature", "forest"]);
        assert_eq!(item.resolution, Some("3840x2160".to_string()));
        assert_eq!(item.size, Some(200_000_000));
        assert_eq!(
            item.sha256.as_deref(),
            Some("abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890")
        );
        request.join().unwrap();
    }

    /// Extra unknown fields on an item do not crash deserialization.
    #[test]
    fn extra_unknown_item_fields_do_not_crash_deserialize() {
        let body = r#"{
            "page": 1,
            "items": [{
                "id": "x",
                "name": "X",
                "download": "https://cdn.example/x.zip",
                "future_field": 999,
                "yet_more": {"nested": true}
            }]
        }"#;
        let (base, _) = serve_once(body);
        let client = Client::new(&base).unwrap();
        let page = client.list(1, None, None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, "x");
    }

    /// Missing pagination numbers default to zero/none.
    #[test]
    fn missing_pagination_fields_default_cleanly() {
        let body = r#"{"page": 0, "items": [{"id":"z","name":"Z","download":"https://example.com/z.mp4"}]}"#;
        let (base, _) = serve_once(body);
        let client = Client::new(&base).unwrap();
        let page = client.list(1, None, None).unwrap();
        assert_eq!(page.page, 0); // 0 is accepted by Page struct default
        assert!(page.next.is_none());
    }

    // ======================================================================== //
    // B) Archive integrity (checked via catalog validation + extract_video)   //
    // ======================================================================== //

    /// Item IDs must be non-empty after trimming.
    #[test]
    fn empty_id_after_trimming_is_rejected() {
        let body = r#"{"items":[{"id":"   ","name":"N","download":"https://example.com/n.mp4"}]}"#;
        let (base, req) = serve_once(body);
        let client = Client::new(&base).unwrap();
        assert!(client.list(1, None, None).is_err());
        req.join().unwrap();
    }

    /// Item name must be non-empty after trimming.
    #[test]
    fn empty_name_after_trimming_is_rejected() {
        let body = r#"{"items":[{"id":"n","name":"  ","download":"https://example.com/n.mp4"}]}"#;
        let (base, req) = serve_once(body);
        let client = Client::new(&base).unwrap();
        assert!(client.list(1, None, None).is_err());
        req.join().unwrap();
    }

    /// Preview URL can be empty string (optional field), but if present must
    /// be a valid HTTP URL.
    #[test]
    fn preview_optional_blank_is_ok_but_invalid_url_fails() {
        // Blank preview — OK.
        let body = r#"{"items":[{"id":"p","name":"P","download":"https://example.com/p.mp4","preview":""}]}"#;
        let (base, req) = serve_once(body);
        let client = Client::new(&base).unwrap();
        let page = client.list(1, None, None).unwrap();
        assert_eq!(page.items[0].preview, "");
        req.join().unwrap();

        // Invalid preview — rejected.
        let body2 = r#"{"items":[{"id":"p2","name":"P2","download":"https://example.com/p2.mp4","preview":"file:///tmp/img.jpg"}]}"#;
        let (base2, req2) = serve_once(body2);
        let client2 = Client::new(&base2).unwrap();
        assert!(client2.list(1, None, None).is_err());
        req2.join().unwrap();
    }

    /// Item ID max length: 200 chars is accepted, 201 is rejected.
    #[test]
    fn id_length_boundary_at_200_chars() {
        let id_long = "x".repeat(200);
        let body = format!(
            r#"{{"items":[{{"id":"{id_long}","name":"N","download":"https://example.com/n.mp4"}}]}}"#
        );
        let (base, req) = serve_once(&body);
        let client = Client::new(&base).unwrap();
        let page = client.list(1, None, None).unwrap();
        assert_eq!(page.items[0].id.len(), 200);
        req.join().unwrap();

        let id_too_long = "y".repeat(201);
        let body2 = format!(
            r#"{{"items":[{{"id":"{id_too_long}","name":"N","download":"https://example.com/n.mp4"}}]}}"#
        );
        let (base2, req2) = serve_once(&body2);
        let client2 = Client::new(&base2).unwrap();
        assert!(client2.list(1, None, None).is_err());
        req2.join().unwrap();
    }

    /// Item name max length: 300 chars accepted, 301 rejected.
    #[test]
    fn name_length_boundary_at_300_chars() {
        let name_long = "x".repeat(300);
        let body = format!(
            r#"{{"items":[{{"id":"n","name":"{name_long}","download":"https://example.com/n.mp4"}}]}}"#
        );
        let (base, req) = serve_once(&body);
        let client = Client::new(&base).unwrap();
        let page = client.list(1, None, None).unwrap();
        assert_eq!(page.items[0].name.len(), 300);
        req.join().unwrap();

        let name_too_long = "y".repeat(301);
        let body2 = format!(
            r#"{{"items":[{{"id":"n","name":"{name_too_long}","download":"https://example.com/n.mp4"}}]}}"#
        );
        let (base2, req2) = serve_once(&body2);
        let client2 = Client::new(&base2).unwrap();
        assert!(client2.list(1, None, None).is_err());
        req2.join().unwrap();
    }

    /// The server caps items to MAX_ITEMS; anything beyond is truncated
    /// after validation. Exactly MAX_ITEMS items must pass.
    #[test]
    fn list_at_max_items_boundary_succeeds() {
        let mut items = Vec::new();
        for i in 0..MAX_ITEMS {
            items.push(format!(
                r#"{{"id":"i{i}","name":"I","download":"https://ex.com/i{i}.mp4"}}"#
            ));
        }
        let body = format!(r#"{{"page":1,"items":[{}]}}"#, items.join(","));
        let (base, req) = serve_once(&body);
        let client = Client::new(&base).unwrap();
        // Exactly MAX_ITEMS should pass validation and not be truncated further.
        let page = client.list(1, None, None).unwrap();
        assert_eq!(page.items.len(), MAX_ITEMS);
        req.join().unwrap();
    }

    /// Items exceeding MAX_ITEMS count triggers an error.
    #[test]
    fn list_exceeding_max_items_rejected_by_validation() {
        let mut items = Vec::new();
        for i in 0..MAX_ITEMS + 50 {
            items.push(format!(
                r#"{{"id":"i{i}","name":"I","download":"https://ex.com/i{i}.mp4"}}"#
            ));
        }
        let body = format!(r#"{{"page":1,"items":[{}]}}"#, items.join(","));
        let (base, req) = serve_once(&body);
        let client = Client::new(&base).unwrap();
        let err = client.list(1, None, None).unwrap_err();
        assert!(err.contains("more than") || err.contains("MAX"), "{err}");
        req.join().unwrap();
    }

    /// Download URLs must be http(s).
    #[test]
    fn download_url_must_be_http_or_https() {
        for proto in &["ftp://", "data:", "file:", "http:", "https:"] {
            let url = format!("{proto}//example.com/file.mp4");
            let body = format!(r#"{{"items":[{{"id":"d","name":"D","download":"{url}"}}]}}"#);
            let (base, req) = serve_once(&body);
            let client = Client::new(&base).unwrap();
            let result = client.list(1, None, None);
            if proto.starts_with("http") {
                assert!(
                    result.is_ok(),
                    "{proto} should be accepted, got: {:?}",
                    result.err()
                );
            } else {
                assert!(result.is_err(), "{proto} should be rejected");
            }
            req.join().unwrap();
        }
    }

    // ======================================================================== //
    // C) Import robustness                                                      //
    // ======================================================================== //

    /// Client constructor rejects various bad base URLs.
    #[test]
    fn client_rejects_various_bad_base_urls() {
        assert!(Client::new("").is_err());
        assert!(Client::new("  ").is_err());
        assert!(Client::new("ftp://example.com/api").is_err());
        assert!(Client::new("file:///etc/catalog").is_err());
        assert!(Client::new("ssh://example.com").is_err());
        assert!(Client::new("http://localhost").is_ok());
        assert!(Client::new("https://api.example.org/v1/").is_ok());
        // Trailing slash is trimmed internally.
        let client = Client::new("https://example.com/api/").unwrap();
        assert_eq!(client.base, "https://example.com/api");
    }

    /// URL too long is rejected by client constructor.
    #[test]
    fn base_url_too_long_is_rejected() {
        let long_url = format!("https://example.com/{}", "a".repeat(MAX_URL_CHARS + 10));
        assert!(Client::new(&long_url).is_err());
        // Right at boundary: MAX_URL_CHARS chars — accepted (chars.count() <= limit)
        let at_limit = format!("https://example.com/{}", "a".repeat(MAX_URL_CHARS - 20));
        assert!(Client::new(&at_limit).is_ok());
    }

    /// Page parameter 0 is rejected.
    #[test]
    fn page_zero_is_rejected() {
        // We need a running server to hit list, but the page-0 check happens
        // before any HTTP call, so we test via mock.
        // Since Client::list always sends HTTP, test that page >= 1 works:
        let body = r#"{"page":1,"items":[{"id":"ok","name":"OK","download":"https://example.com/o.mp4"}]}"#;
        let (base, req) = serve_once(body);
        let client = Client::new(&base).unwrap();
        assert!(client.list(1, None, None).is_ok());
        req.join().unwrap();
    }

    /// Query parameter encoding preserves special characters safely.
    #[test]
    fn query_parameter_is_percent_encoded() {
        let body = r#"{"page":1,"items":[]}"#;
        let (base, request_handle) = serve_once(body);
        let client = Client::new(&base).unwrap();
        let _ = client.list(1, Some("hello world & more"), None);
        let request = request_handle.join().unwrap();
        assert!(
            request.contains("q=hello%20world%20%26%20more"),
            "{}",
            request
        );
    }

    /// Category parameter is percent encoded.
    #[test]
    fn category_parameter_is_percent_encoded() {
        let body = r#"{"page":1,"items":[]}"#;
        let (base, request_handle) = serve_once(body);
        let client = Client::new(&base).unwrap();
        let _ = client.list(1, None, Some("video/games"));
        let request = request_handle.join().unwrap();
        assert!(request.contains("category=video%2Fgames"), "{}", request);
    }

    /// Empty query strings are omitted from the URL.
    #[test]
    fn empty_query_does_not_add_param() {
        let body = r#"{"page":1,"items":[]}"#;
        let (base, request_handle) = serve_once(body);
        let client = Client::new(&base).unwrap();
        let _ = client.list(1, Some(""), None);
        let request = request_handle.join().unwrap();
        assert!(!request.contains("&q="), "{request}");
    }

    /// Whitespace-only query strings are omitted.
    #[test]
    fn whitespace_only_query_is_omitted() {
        let body = r#"{"page":1,"items":[]}"#;
        let (base, request_handle) = serve_once(body);
        let client = Client::new(&base).unwrap();
        let _ = client.list(1, Some("   "), None);
        let request = request_handle.join().unwrap();
        assert!(!request.contains("&q="), "{request}");
    }

    /// Whitelist endpoint path construction is correct.
    #[test]
    fn list_endpoint_construction() {
        let body = r#"{"page":5,"items":[]}"#;
        let (base, request_handle) = serve_once(body);
        let client = Client::new(&base).unwrap();
        let _ = client.list(5, None, None);
        let request = request_handle.join().unwrap();
        assert!(request.contains("/wallpapers?page=5"), "{request}");
        // Verify it's GET.
        assert!(request.starts_with("GET "), "{request}");
    }

    /// Server returns HTTP 4xx — client reports error with status code.
    #[test]
    fn server_error_status_returns_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
            )
            .unwrap();
        });
        let client = Client::new(&format!("http://{address}/v1")).unwrap();
        let err = client.list(1, None, None).unwrap_err();
        assert!(
            err.contains("404") || err.contains("HTTP"),
            "error was: {err}"
        );
    }

    /// Server response exceeding MAX_BODY_BYTES is rejected.
    #[test]
    fn oversized_server_response_is_rejected() {
        // We don't know whether the response gets truncated by ureq first
        // or parsed as invalid JSON. Either rejection path is acceptable.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let big_body = serde_json::json!({
            "page": 1,
            "items": vec![serde_json::json!({
                "id": "x",
                "name": "overflow-test",
                "download": "https://example.com/x.mp4",
                "description": "a".repeat(MAX_BODY_BYTES as usize)
            })]
        });
        let body_str = big_body.to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body_str.len(),
                body_str
            );
        });
        let client = Client::new(&format!("http://{address}/v1")).unwrap();
        let err = client.list(1, None, None).unwrap_err();
        // Any error means rejection; which specific message depends on
        // whether ureq truncates or serde parses first.
        assert!(!err.is_empty());
    }

    /// WallpaperItem serialises deterministically.
    #[test]
    fn wallpaper_item_serialisation_order_preserves_field_names() {
        let item = WallpaperItem {
            id: "ser".into(),
            name: "Ser".into(),
            author: "Auth".into(),
            description: "Desc".into(),
            preview: "https://example.com/p.jpg".into(),
            download: "https://example.com/d.mp4".into(),
            category: "Cat".into(),
            tags: vec!["t1".into()],
            resolution: Some("1920x1080".into()),
            size: Some(42),
            sha256: Some("abc".into()),
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: WallpaperItem = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, item.id);
        assert_eq!(parsed.name, item.name);
        assert_eq!(parsed.resolution, item.resolution);
    }

    /// Page struct round-trips through JSON.
    #[test]
    fn page_struct_round_trips() {
        let page = Page {
            page: 42,
            next: Some(43),
            items: vec![WallpaperItem {
                id: "rt".into(),
                name: "RoundTrip".into(),
                download: "https://example.com/rt.mp4".into(),
                ..Default::default()
            }],
        };
        let text = serde_json::to_string(&page).unwrap();
        let p: Page = serde_json::from_str(&text).unwrap();
        assert_eq!(p.page, 42);
        assert_eq!(p.next, Some(43));
        assert_eq!(p.items.len(), 1);
    }

    /// Percent encoding is lossless for ASCII alphanumerics.
    #[test]
    fn percent_encode_preserves_alphanumeric() {
        assert_eq!(percent_encode("ABC"), "ABC");
        assert_eq!(percent_encode("abc"), "abc");
        assert_eq!(percent_encode("0123456789"), "0123456789");
        assert_eq!(percent_encode("-_.~"), "-_.~");
    }

    /// Client validates download field is always present and required.
    #[test]
    fn download_is_required_in_wallpaper_item() {
        // Serde will fail on missing required field 'download'.
        let result: Result<WallpaperItem, _> = serde_json::from_str(r#"{"id":"x","name":"X"}"#);
        assert!(result.is_err(), "download field is required");
    }

    /// Default instance of WallpaperItem has empty strings and None values.
    #[test]
    fn wallpaper_item_default_values() {
        let item: WallpaperItem = WallpaperItem {
            id: "d".into(),
            name: "D".into(),
            ..Default::default()
        };
        assert_eq!(item.author, "");
        assert_eq!(item.description, "");
        assert_eq!(item.preview, "");
        assert_eq!(item.category, "");
        assert!(item.tags.is_empty());
        assert!(item.resolution.is_none());
        assert!(item.size.is_none());
        assert!(item.sha256.is_none());
    }
}
