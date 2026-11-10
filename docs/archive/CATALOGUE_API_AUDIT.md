# Ferramari Catalogue API Audit

> **Date:** 2026-11-07  
> **Version:** 2.0.0  
> **TODO Reference:** §9 — Community, catalogue and creator workflows (items 129–131)  
> **Source files:** `crates/ferramari-core/src/catalog_api.rs`, `catalogue.rs`, `plugin.rs`

## Architecture: two catalogue backends

Ferramari ships **two independent catalogue clients**:

| Client | Module | Transport | Purpose |
|--------|--------|-----------|---------|
| Catalog API v1 | `catalog_api.rs` | JSON over HTTP | Declarative, stable contract |
| Legacy scraper | `catalogue.rs` | HTML scraping + regex | The original hidamari.ru community site |

Both are exercised by the Iced UI (`Online` page): `catalogue::search()` for text search, `CatalogApi::list()` for category/pagination browsing.

## 1. Catalogue API v1 (§9.1)

### Request contract

```
GET {base}/wallpapers?page={n}&q={query}&category={slug}
User-Agent: Ferramari/catalog-api
```

| Aspect | Detail | Status |
|--------|--------|--------|
| Base URL validation | Must be `http://` or `https://`, max 2048 chars | ✅ |
| Page validation | `page >= 1`, rejects 0 with explicit error | ✅ |
| Query encoding | `percent_encode(query)` after trim + empty filter | ✅ |
| Category encoding | Same percent-encoding path as query | ✅ |
| Request timeout | `Duration::from_secs(15)` on the ureq client | ✅ |
| HTTP error mapping | Any status ≥ 400 → descriptive error string | ✅ |

### Response contract (`Page` struct)

```rust
pub struct Page {
    pub next: Option<u32>,      // next page number, or None at the end
    pub items: Vec<WallpaperItem>,
}
pub struct WallpaperItem {
    id, name, author, description, preview, download
}
```

| Aspect | Detail | Status |
|--------|--------|--------|
| Body size cap | 2 MiB (`MAX_BODY_BYTES`), enforced with `.take(MAX_BODY_BYTES + 1)` | ✅ |
| Oversized body | Returns error, does not buffer beyond the cap | ✅ |
| Item count cap | `MAX_ITEMS = 200`, `page.items.truncate(MAX_ITEMS)` even if server sends more | ✅ |
| `id` field validation | non-empty, ≤ 200 chars | ✅ |
| `name` field validation | non-empty, ≤ 300 chars | ✅ |
| `download` field validation | must be `http://` or `https://` | ✅ |
| `preview` field validation | if non-empty, must be `http(s)://` | ✅ |
| Pagination loop safety | `next: Option<u32>` (page numbers, no arbitrary URLs) | ✅ |

**Findings (§9.1):** The JSON API v1 is strictly validated: bounded request, bounded response, bounded items, bounded strings, and `http(s)`-only download/preview URLs. Pagination uses page-numbers so a hostile server cannot inject a redirect URL into the `next` cursor. This matches the design intent of a strict v1 contract.

### Gaps for §9.1

1. **Download URL trust boundary.** Validation accepts any `http(s)` URL. A malicious catalogue can point `download` at an arbitrary host — extraction limits (§9.2) are the remaining defence.
   **Recommendation:** Add a per-item download size cap and require the download URL host to equal the catalogue base host.
2. **No `Content-Type` check.** The client assumes JSON even if the server returns `text/html`. The JSON parse fails with a descriptive error, so this is a soft issue, not a security one.
3. **No cache or ETag.** Every page fetch hits the network (unlike `catalogue.rs` which caches for 5 minutes). Acceptable for a v1 contract.

## 2. Legacy scraper (§9.1 continued)

| Aspect | Detail | Status |
|--------|--------|--------|
| HTML limit | 5 MiB (`HTML_LIMIT`) via `read_text(response, HTML_LIMIT)` | ✅ |
| Page cache | 5 minutes (`PAGE_CACHE_AGE`) on disk, keyed by URL hash | ✅ |
| Search result filter | Only items whose category starts with "Живые обои" are kept | ⚠️ locale-coupled |
| Pagination cursor | Parsed from HTML `<a>` "next" link — server-controlled URL string | ⚠️ |
| Signed link extraction | Regex: `window.location\s*=\s*'([^']+)'` — fragile but intentional | ⚠️ |

**Findings:** The scraper is legacy HTML coupling with a hard-coded Russian filter prefix. A site redesign or a locale change silently returns empty listings.

### Gaps for §9.1
1. **Search filter is hard-coded to Russian.** `item.category.starts_with("Живые обои")` will match no items on a non-Russian locale site.
2. **Next-page URL comes from HTML.** `parse_next()` returns a `String` and `OnlineMore` calls `fetch_page_cached(url)` directly — no validation that the URL host matches the original site. Mitigation: the cache is keyed by URL, but the request still goes wherever the cursor says.

## 3. Archive extraction limits (§9.2)

Package archives (`.ferramari` = ZIP) are handled through `catalogue.rs` and `package.rs`:

| Protection | Mechanism | Status |
|------------|-----------|--------|
| Signed URL validation | Regex capture of `window.location = '...'` | ✅ |
| Path traversal in ZIP entries | Rejected by archive extractor (rejects absolute paths, `..`) | ✅ |
| Manifest size cap | `MAX_MANIFEST_BYTES = 64 KiB` in `plugin.rs` | ✅ |
| Plugin id cap | `MAX_ID_CHARS = 64` | ✅ |
| Code execution | Plugins are declarative metadata only; no code runs in-process | ✅ |

### Gaps for §9.2
1. **ZIP-bomb protection incomplete.** Archive extraction rejects `..`/absolute paths, but there is no uncompressed-size ratio limit. A compressed-but-huge archive would hit disk quota rather than parser checks.
   **Recommendation:** Cap total decompressed size of a `.ferramari` package (e.g., 2 GiB).
2. **Manifest schema is loose.** Plugin validation is only length-based; should reject unexpected top-level fields to catch format drift.

## 4. Download integrity (§9.1)

Downloads flow through `catalogue::download()`: resolves signed URL → streams to temp path → extracts → registers library.

Missing: checksum verification, TLS pinning, resume support.

## 5. Summary scores

| TODO item | Section | Status | Notes |
|-----------|---------|--------|-------|
| Revalidate catalogue API v1 | §9.1 | ✅ Verified | Strict caps and validation confirmed |
| Pagination | §9.1 | ✅ Verified | JSON client is page-number based (safe); HTML uses server-provided cursor |
| Filtering | §9.1 | ⚠️ Locale coupling | Search filter hard-coded to Russian prefix |
| Download integrity | §9.1 | ⚠️ Partial | Signed URL OK; no checksum or TLS pinning |
| Archive extraction limits | §9.2 | ⚠️ Partial | Path traversal + manifest cap present; no uncompressed-size ratio cap |
| Declarative plugins | §9.2 | ✅ Verified | No code execution, bounded manifest |
