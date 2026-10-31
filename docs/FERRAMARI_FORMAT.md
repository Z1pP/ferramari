# The `.ferramari` package format

A package is a directory named `MyWallpaper.ferramari/`:

```text
MyWallpaper.ferramari/
├── manifest.json
├── preview.jpg          (optional)
├── wallpaper.mp4        (video)
│   or index.html        (web)
├── assets/              (optional)
├── scripts/             (optional)
└── resources/           (optional)
```

The directory name is a convention; import accepts any directory that passes
validation. Export always writes `<slug>.ferramari`.

## manifest.json

```json
{
  "format": 1,
  "name": "My Wallpaper",
  "author": "Creator",
  "version": "1.0",
  "type": "video",
  "description": "Example wallpaper",
  "resolution": "3840x2160",
  "entry": "wallpaper.mp4",
  "preview": "preview.jpg",
  "requirements": {
    "audio": false,
    "javascript": false
  },
  "engine_min": "1.6.0"
}
```

| Field | Required | Rules |
| --- | --- | --- |
| `format` | no (default 1) | only 1 is supported; a newer value is rejected |
| `name` | yes | 1–100 characters |
| `author` | no | up to 120 characters |
| `version` | no (default `1.0`) | up to 32 characters |
| `type` | yes | `video` or `web` |
| `description` | no | free text |
| `resolution` | no | `WIDTHxHEIGHT`, both positive |
| `entry` | no | relative path; default `wallpaper.mp4` or `index.html` |
| `preview` | no | relative image path (`jpg`, `jpeg`, `png`, `webp`); `preview.jpg` is used if present |
| `requirements` | no | `audio`/`javascript` booleans, informational |
| `engine_min` | no | version of Ferramari required |

Video entries: `mp4`, `webm`, `gif`, `mkv`, `mov`, `m4v`. Web entries:
`html`, `htm`.

## Validation and limits

Import rejects a package if any of these is true:

- `manifest.json` is missing, larger than 1 MiB, or not valid JSON;
- the format is newer than 1, or `engine_min` is newer than the running app;
- an entry/preview path is absolute, contains `..`, or resolves outside the
  package after canonicalisation;
- the entry extension does not match its type;
- the package contains a symlink (including a symlink to a file inside it);
- the tree is deeper than 32 directories or has more than 10,000 files;
- the total size exceeds 2 GiB.

Validation runs again after copying. A failed import removes the partial
destination. Filenames are never executed; a web package is loaded by
WebKitGTK with the same policy as any other web wallpaper, including the
optional network switch and denied permissions.

## Import and export

**Import** (Local Video → Import…): choose the package directory. A video
package is copied to `$XDG_DATA_HOME/ferramari/packages/<slug>/`, its entry is
assigned to all monitors, and it appears in the local video grid. A web package
is installed and applied as the current web wallpaper.

**Export** (select a video → Export .ferramari): choose a destination folder.
The app writes `<slug>.ferramari/` with `wallpaper.<ext>` and a generated
preview when ffmpeg can produce one. The result is re-validated before the
success message.

Packages are plain files. Removing a package directory and reloading the
library removes it from the grid; the current monitor assignment then points
to a missing file and is reported by the normal playback error path.

## Version compatibility

`format` protects the package reader. `engine_min` protects features that an
older build cannot render. The current app writes `engine_min` equal to its own
version so a package exported by a newer build is not silently opened by an
older one.