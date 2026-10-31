# Plugin API (catalogue sources)

The first plugin API is intentionally narrow: a plugin is a **declarative
catalogue source**, not executable code. It cannot read files, spawn processes
or run JavaScript in the UI. This keeps the boundary small while the local
engine and package format settle.

## Location

```text
$XDG_DATA_HOME/ferramari/plugins/
└── my-source/
    └── plugin.json
```

`plugins` is created on start. A plugin directory is ignored if it is a
symlink, its manifest is larger than 64 KiB, the JSON is invalid, or validation
fails.

## Manifest

```json
{
  "id": "example.catalog",
  "name": "Example Catalogue",
  "version": "1.0",
  "description": "Optional text",
  "kind": "catalog",
  "api": "https://catalog.example.org/v1",
  "engine_min": "1.7.0"
}
```

| Field | Required | Rules |
| --- | --- | --- |
| `id` | yes | 1–64 ASCII letters, digits, `.`, `_`, `-`; unique by convention |
| `name` | yes | non-empty display name |
| `version` | no | up to 32 characters |
| `description` | no | free text |
| `kind` | yes | currently only `catalog` |
| `api` | yes | `http://` or `https://` base URL, max 512 characters |
| `engine_min` | no | Ferramari version required; newer versions are ignored |

`engine_min` prevents a newer plugin from silently using an API the running
version does not understand. Invalid plugins are logged and skipped; they never
prevent the app from starting.

## Discovery and UI

`ferramari --doctor` lists every accepted source:

```text
plugin sources: example.catalog (Example Catalogue) -> https://catalog.example.org/v1
```

Settings → System → Community sources shows the same list. The source API is
documented in [CATALOG_API.md](CATALOG_API.md); the client can be exercised
with:

```sh
ferramari --catalog https://catalog.example.org/v1
```

## Not implemented yet

- executable renderer plugins (new wallpaper types / effects);
- installing plugins from inside the UI;
- signatures or a trust store beyond manifest validation;
- first-party hosted catalogue.

The package format (`.ferramari`) is the supported way to ship a new wallpaper
type today. A future renderer API should be process-isolated with an explicit
capability list; it must not be a thin `dlopen` of arbitrary code.