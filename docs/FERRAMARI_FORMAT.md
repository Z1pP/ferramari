# The `.ferramari` package format

A package is a directory named `MyWallpaper.ferramari/`:

```text
MyWallpaper.ferramari/
├── manifest.json
├── checksums.json       (optional, added by 1.9+)
├── signature.json       (optional, added by 1.9+)
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

## Integrity and authorship

`checksums.json` maps every other file in the package to its SHA-256:

```json
{
  "manifest.json": "3f2b…",
  "wallpaper.mp4": "9ac1…"
}
```

The two integrity files never hash themselves. Loading a package that carries
`checksums.json` verifies every entry, so a modified or an extra file is
rejected with a clear message. A package without checksums still loads — the
format predates them and creators must not be locked out of their own library.

`signature.json` is an Ed25519 signature over the raw bytes of
`checksums.json`, so one signature covers every hash and every hash covers every
file:

```json
{
  "format": 1,
  "algorithm": "ed25519",
  "key_id": "bd65de61cc23adc9",
  "signature": "…base64…",
  "name": "Alice",
  "public_key": "…base64, 32 bytes…"
}
```

`key_id` is the first 16 hex characters of SHA-256 over the raw public key.
Verification has four outcomes: **Trusted** (the key is in the local trust
store), **Unknown** (the signature verifies against the embedded key, but that
key is not trusted yet), **Unsigned** and **Invalid**. An invalid signature or a
checksum mismatch rejects the package; an unknown author is reported and the
user decides whether to trust the key.

There is no web of trust and no network validation. Trusting is always an
explicit user action, in Local Video → Verify, in Settings → Package signing, or
with `ferramari package trust`.

### Keys

- `~/.config/ferramari/signing-key.json` — the local key pair, mode `0600`,
  created on first use.
- `~/.config/ferramari/trusted-keys.json` — public keys the user chose to trust.

Exporting a package signs it when a key exists, so a creator's own packages
verify as trusted on their machine.

### Command line

```sh
ferramari package key --name "Alice" --export alice.json
ferramari package sign MyWallpaper.ferramari
ferramari package verify MyWallpaper.ferramari   # 0 trusted, 1 untrusted, 2 invalid
ferramari package lint MyWallpaper.ferramari     # 0 publishable, 2 blocked, 3 bad input
ferramari package trust alice.json               # or a signed package directory
ferramari package keys
```

These commands run before the server starts, so they work over SSH and headless.

## Creator review: `package lint`

`verify` answers "is this package intact and signed". `lint` answers a
different question — **"what will a recipient see"** — because import accepts
many packages that look broken once they are installed: no description, no
preview, no resolution and no signature all pass validation and then render as
an empty card in the library and the catalogue.

Findings have two severities:

| Severity | Meaning | Exit code |
| --- | --- | --- |
| `error` | do not publish: import refuses it, or the signature does not verify | `2` |
| `warning` | publishable, but the result is degraded | `0` (or `2` with `--strict`) |

An invalid signature is an error even though `package::load` only reports it
and still copies the package: a tampered package must never reach a catalogue.

Each finding carries a stable code, so a CI job can assert on machine-readable
output instead of grepping sentences:

```sh
ferramari package lint --json MyWallpaper.ferramari
ferramari package lint --ignore preview-missing MyWallpaper.ferramari
ferramari package lint --strict MyWallpaper.ferramari
```

| Code | Severity | Raised when |
| --- | --- | --- |
| `import-rejected` | error | `package::load` refused the package; the message is import's own |
| `name-missing` | error | the manifest declares an empty name and import did not already report it |
| `signature-invalid` | error | a `signature.json` exists and does not verify |
| `unsigned` | warning | no `signature.json` |
| `checksums-missing` | warning | no complete `checksums.json` |
| `author-missing` | warning | `author` is empty: nothing to credit or match a key against |
| `description-missing` | warning | `description` is empty |
| `preview-missing` | warning | no preview image in the package |
| `preview-unreadable` | warning | a preview is present but does not decode as an image |
| `resolution-mismatch` | warning | declared `resolution` contradicts the measured entry |
| `resolution-missing` | warning | no resolution declared |

Warnings exit 0 by default on purpose. A creator iterating on a package would
otherwise get a red build for shipping something merely incomplete, which is
how lint output ends up ignored; `--strict` is the opt-in gate.

`--ignore` refuses unknown codes with exit `3`: a filter that silently matches
nothing turns a real finding into a green build.

Rules absent from the table are absent deliberately. lint does not re-implement
import's structural checks (name length, path traversal, symlinks, size caps,
`format`, `engine_min`); those arrive inside `import-rejected` as import's own
words, so there is one place to keep them in sync and a creator is never told
the same violation twice in different phrasing.

Measurement follows the same honesty rule. lint probes the entry and preview
with `ffprobe` when it can and distinguishes three kinds of "no number": no
tool available, a web page that has no intrinsic size, and a file that refuses
to decode. Only the last is the creator's problem; blaming someone for a
missing ffmpeg is how output gets ignored everywhere else.

### When integrity is checked

- **On import** — always, before the package is copied into the library;
- **on export** — the freshly written package is re-read and re-verified;
- **on demand** — Local Video → Verify, or `ferramari package verify`.

Playback deliberately does not re-hash the video: a 2 GiB package would cost
seconds on every monitor assignment. A file changed after import is therefore
detected by verification, not silently at play time; the library shows the
package as it was imported, and Verify is the check to run after downloading or
copying a package by hand.

## Version compatibility

`format` protects the package reader. `engine_min` protects features that an
older build cannot render. The current app writes `engine_min` equal to its own
version so a package exported by a newer build is not silently opened by an
older one.