# Windows packaging

Ferramari ships two executables that must stay side by side:

```
Ferramari\
  ferramari.exe
  ferramari-ui.exe
  locale\ru\LC_MESSAGES\ferramari.mo
  locale\ja\LC_MESSAGES\ferramari.mo
  README.md
  LICENSE
```

`paths::locale_dir()` checks `<exe>\locale` first, so the portable ZIP and the
installed layout both find their catalogs without an environment variable.

## Build

Requirements:

- Rust with the MSVC toolchain and Visual Studio Build Tools
- [Inno Setup 6](https://jrsoftware.org/isinfo.php) (for the installer)
- GNU gettext `msgfmt` (optional, for Russian/Japanese catalogs)

From the repository root:

```powershell
cargo build --workspace --release --locked
.\installer\build.ps1 -NoBuild
```

Or let the script build:

```powershell
.\installer\build.ps1
```

Outputs:

| File | Contents |
| --- | --- |
| `installer\Output\Ferramari-<version>-setup.exe` | Inno Setup installer |
| `installer\Output\Ferramari-<version>-portable.zip` | the layout above |

The CI job `.github/workflows/ci.yml` runs the same script with `-NoBuild`
after the release build and uploads both files as the `ferramari-windows`
artifact. Tagged releases attach them to the GitHub release.

## Installer behaviour

- `AppId` is stable, so a newer installer upgrades the existing installation
  instead of adding a second copy.
- `PrivilegesRequired=lowest` installs under
  `%LOCALAPPDATA%\Programs\Ferramari` by default. Choosing elevation installs
  for all users under `Program Files`.
- Start-menu shortcut always; desktop shortcut is an unchecked task.
- The installer includes English, Russian and Japanese wizard languages.
- Uninstall removes only files the installer wrote. It deliberately does not
  delete `%APPDATA%\ferramari` (config, library, signing key, original
  wallpaper record) or `%LOCALAPPDATA%\ferramari` (cache, generated frames).
  Users who want a clean removal delete those folders themselves.

## Runtime tools

The installer does not bundle `mpv`, `ffmpeg`/`ffprobe` or `yt-dlp`. They must
be on `PATH`, in a standard Windows install location (`Program Files\MPV
Player`, the winget `Links` folder or `%LOCALAPPDATA%\Programs`), or copied
next to `ferramari.exe`; `paths::tool` prefers the sibling copy and then the
standard locations. Streaming additionally needs a current `yt-dlp`.

## Unverified

The script and `.iss` are exercised by the Windows CI job, but this checkout
has no Inno Setup installation and no clean Windows account, so a real
install/upgrade/uninstall run is still on the smoke checklist in
`docs/TESTING.md`.
