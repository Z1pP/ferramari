# Ferramari Catalog API v1

This is the first-party catalogue contract. It is independent of the existing
winzoro.net scraper and can be hosted separately. Ferramari ships a client, not
a server; `ferramari --catalog <base>` is a quick way to inspect a deployment.

## Endpoint

```http
GET /wallpapers?page=1&q=optional+search&category=optional
```

A deployment's base URL includes its API prefix, for example
`https://catalog.example.org/v1`. The client appends `/wallpapers`.

- `page` is 1-based.
- `q` and `category` are optional and percent-encoded.
- Responses must be `application/json`, at most 2 MiB, with at most 200 items.

## Response

```json
{
  "page": 1,
  "next": 2,
  "items": [
    {
      "id": "neon-city",
      "name": "Neon City",
      "author": "Example Author",
      "description": "Optional text",
      "preview": "https://cdn.example.org/neon.jpg",
      "download": "https://cdn.example.org/neon.mp4",
      "category": "anime",
      "tags": ["city", "neon"],
      "resolution": "1920x1080",
      "size": 12345678,
      "sha256": "optional hex digest"
    }
  ]
}
```

| Field | Required | Notes |
| --- | --- | --- |
| `id` | yes | stable, unique, up to 200 chars |
| `name` | yes | up to 300 chars |
| `author` | no | display name |
| `description` | no | plain text |
| `preview` | no | `http(s)` image URL |
| `download` | yes | `http(s)` video/package URL |
| `category` | no | display/filter key |
| `tags` | no | string array |
| `resolution` | no | `WIDTHxHEIGHT` |
| `size` | no | bytes |
| `sha256` | no | integrity hint for future verification |

`next` is `null` when there is no next page. The client rejects non-HTTP
download/preview URLs, oversized bodies, and invalid ids/names.

## Client behaviour

- 15 s timeout, `User-Agent: Ferramari/catalog-api`.
- Does not follow links automatically beyond the HTTP client's normal
  redirect handling.
- Never writes the response outside the catalogue cache.
- Does not execute code from the response.

## Server implementation notes

A server can be static JSON, a small Rust/Go/Python service, or a CDN-backed
endpoint. It should support ETag/Last-Modified and rate limiting. No
authentication is defined for v1; publishing/author profiles would need a
separate authenticated API and are not part of this contract.

The existing winzoro catalogue remains a separate legacy provider; it is not
the first-party API.

## Trying it

```sh
ferramari --catalog https://catalog.example.org/v1
```

prints:

```text
page 1 next Some(2)
neon-city	Neon City
...
```