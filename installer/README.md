# Windows packaging

Ferramari ships two executables that must stay side by side:

```
Ferramari\
  ferramari.exe
  ferramari-ui.exe
  ferramari-launcher.exe
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

`build.ps1` looks for `ISCC.exe` and `msgfmt.exe` on `PATH`, in the usual
`Program Files` locations and under `%LOCALAPPDATA%\Programs`, which is where
winget installs them for the current user without elevation.

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

## Verified packaging

The 2.1.0 Windows artifacts were built and exercised on Windows 11 23H2 with
Inno Setup 6.7.3 and gettext 1.0+iconv 1.19 installed per user:

- `installer\build.ps1 -NoBuild` compiled both catalogs with
  `msgfmt --check`, produced `Ferramari-2.1.0-setup.exe` (9.1 MB) and
  `Ferramari-2.1.0-portable.zip` (10.5 MB).
- A silent per-user install placed both executables, README, LICENSE, ru/ja
  catalogs and Start-menu shortcuts in the target directory; the installed
  `ferramari --doctor` reported the Win32 backend with every capability
  supported.
- The Start-menu shortcut started `ferramari`, `ferramari-ui` and mpv; a
  control `quit` stopped the server and mpv.
- The portable ZIP extracted to a path containing a space; its
  `ferramari.exe --doctor` ran.
- Silent uninstall removed the installation directory and shortcuts and left
  `%APPDATA%\ferramari\config.json` byte-identical.
- The control surface showed Russian and Japanese catalogs without clipped
  labels.

The detailed smoke checklist and the remaining hardware-dependent items live in
`docs/TESTING.md`.
