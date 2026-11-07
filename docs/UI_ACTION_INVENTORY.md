# Ferramari UI Action Inventory — Iced vs GTK

> **Date:** 2026-11-07  
> **Version:** 2.0.0 (Iced)  
> **Reference:** v1.9.4 (GTK/WebKitGTK)  
> **Source:** `crates/ferramari-ui/src/main.rs` — all `Message::*` variants used in page views and handlers

## Page-by-page action mapping

### Home (`fn home`)
| Action | Message | D-Bus Method | Status |
|--------|---------|--------------|--------|
| Toggle play/pause | `TogglePlay` | `toggle_play` | ✅ Complete |
| Toggle mute | `ToggleMute` | `set_mute(bool)` | ✅ Complete |
| Reload wallpaper | `ReloadWallpaper` | `reload()` | ✅ Complete |
| Feeling lucky (random) | `FeelingLucky` | `feeling_lucky()` | ✅ Complete |
| Set volume slider | `SetVolume(i32)` | `set_volume(i32)` | ✅ Complete |
| Static wallpaper toggle | `ToggleStatic(bool)` | `is_static_wallpaper` prop | ✅ Complete |
| Blur radius change | `BlurChanged(i32)` | `blur_radius` prop | ✅ Complete |
| Apply blur draft | `ApplyBlur` | `set_property("blur_radius")` | ✅ Complete |
| Monitor rotation assignment | `SetMonitorRotation(m, coll)` | `set_monitor_rotation` | ✅ Complete |
| Pause on maximize | N/A | `is_pause_when_maximized` | ⚠️ Only in Settings |
| Mute on maximize | N/A | `is_mute_when_maximized` | ⚠️ Only in Settings |

**Missing from Home:** No direct access to theme/language/playback settings (correctly deferred to Settings page).

### Library (`fn library`)
| Action | Message | D-Bus/Local | Status |
|--------|---------|-------------|--------|
| Add video via dialog | `PickVideo` → `VideoPicked` | rfd file picker + `Library::import_video` | ✅ Complete |
| Import video into library | `VideoImported` | Server stores path in data_source | ✅ Complete |
| Refresh library list | `RefreshLibrary` | Rescans video directory locally | ⚠️ Needs polish |
| Play video on monitor | `PlayOnMonitor(path, monitor)` | `video(path, monitor)` D-Bus call | ✅ Complete |
| Add to collection | `AddToCollection(id, path)` | Collection item append | ✅ Complete |
| Remove from collection | `RemoveFromCollection(id, path)` | Collection item remove | ✅ Complete |
| Play video (library view) | `Play(path)` | Uses default monitor or first available | ✅ Complete |
| Toggle favorite | `ToggleFavorite(path)` | Favorites list add/remove | ✅ Complete |
| Remove media from library | `RemoveMedia(path)` | Removes local copy from library dir | ⚠️ Partial — no thumbnail refresh |
| Open folder (file manager) | `OpenFolder` | OS native file manager launch | ✅ Complete |
| Rename collection | `RenameCollection(id, name)` | Collection rename | ✅ Complete |
| Create collection | `CreateCollection` | New empty collection | ✅ Complete |
| Delete collection | `DeleteCollection(id)` | Remove collection entirely | ✅ Complete |

**Missing from Library:** Thumbnails generation per video entry (TODO: ffmpeg-based thumb extraction), drag-and-drop reorder support.

### Online Catalogue (`fn online`)
| Action | Message | D-Bus/API | Status |
|--------|---------|-----------|--------|
| Search wallpapers | `OnlineSearch` → `CatalogueLoaded` | Catalogue API search endpoint | ✅ Complete |
| Category filter | `OnlineCategory(name)` | Filter by category ID | ✅ Complete |
| Load more results | `OnlineMore` | Paginated next-page fetch | ✅ Complete |
| Refresh catalogue | `OnlineRefresh` | Force reload without pagination state | ✅ Complete |
| Download wallpaper | `Download(wallpaper)` → `Downloaded` | Fetch + extract to packages dir | ✅ Complete |
| Enter search query | `OnlineSearchChanged(str)` | Local buffer update | ✅ Complete |

**Known issues:** Preview images use remote URLs only (no cached thumbnails yet), single-click download without user confirmation for large archives.

### Performance (`fn performance`)
| Action | Message | Status |
|--------|---------|--------|
| Set language | `SetLanguage(lang)` | ✅ Complete |
| Set theme | `SetTheme(theme)` | ✅ Complete |
| Toggle autostart | `ToggleAutostart(enabled)` | ✅ Complete |
| Set playback profile | `SetProfile(profile)` | ✅ Complete |

### Settings (`fn settings`)
| Action | Message | Status |
|--------|---------|--------|
| Set theme | `SetTheme(theme)` | ✅ Complete |
| Set language | `SetLanguage(lang)` | ✅ Complete |
| Toggle autostart | `ToggleAutostart(bool)` | ✅ Complete |
| Set playback profile | `SetProfile(mode)` | ✅ Complete |
| Set hardware decoding | `SetHwdec(mode)` | ✅ Complete |
| Toggle pause on battery | `TogglePauseOnBattery(bool)` | ✅ Complete |
| Set idle timeout | `SetIdleTimeout(sec)` | ✅ Complete |
| Set rotation mode | `SetRotationMode(mode)` | ✅ Complete |
| Set rotation interval | `SetRotationInterval(sec)` | ✅ Complete |
| Toggle web network | `ToggleWebNetwork(bool)` | ✅ Complete |
| Toggle pause when maximized | `TogglePauseMaximized(bool)` | ✅ Complete |
| Toggle mute when maximized | `ToggleMuteMaximized(bool)` | ✅ Complete |

### Package Management (cross-cutting, invoked from Library page)
| Action | Message | D-Bus/Local | Status |
|--------|---------|-------------|--------|
| Pick package source | `PickPackageSource` → `PackageSourcePicked` | rfd folder dialog | ✅ Complete |
| Import package | `ImportPackage` | Server-side unpack + merge | ✅ Complete |
| Export video as package | `ExportPackage` | Creates `.ferramari` archive | ✅ Complete |
| Sign package with key | `SignPackage` | Ed25519 signature over metadata.json | ✅ Complete |
| Verify package signature | `VerifyPackage` | Verifies against trusted keys | ✅ Complete |
| Trust package signing key | `TrustPackageKey` | Adds key to ~/.config/ferramari/keys | ✅ Complete |
| Pick export destination | `PickPackageParent` → `PackageParentPicked` | rfd folder dialog | ✅ Complete |

## Summary gap analysis

### Fully covered (✅)
- Playback controls: play/pause, mute, volume, reload, random
- Wallpaper assignment: per-monitor play, collections, rotation modes
- Library management: add video, import/export packages, collections CRUD
- Catalogue browsing: search, categories, pagination, download
- Player settings: hwdec, profile, playback quality
- App settings: theme, language, autostart, idle timeout
- Security: sign, verify, trust package keys
- System toggles: static wallpaper, blur, pause/mute on maximize, pause on battery, web network
- Native dialogs: video picker (rfd), package folders

### Partial coverage (⚠️)
- `RefreshLibrary`: rescans filesystem but doesn't trigger thumbnail regeneration
- `RemoveMedia`: removes file from disk but leaves stale entries in catalog thumbnails cache
- Package operations: progress reporting works but lacks granular ETA/status updates

### Missing features (❌)
- Video thumbnail preview generation (ffmpeg-based)
- Drag-and-drop reordering of library items
- Catalogue preview image caching
- Webpage wallpaper renderer (marked Unsupported in TODO §7)

### Not applicable to Iced migration
- GTK-specific rendering events (GDK draw callbacks)
- GTK ApplicationHoldGuard replaced by zbus name ownership

## Notes for future work

1. Thumbnails require integration with ffmpeg `ss=0` frame extraction — see existing `ffprobe`/`ffmpeg` calls in `catalogue.rs`.
2. Package operation status bars need async stream updates for real-time progress.
3. Webpage wallpaper depends on selecting a replacement for WebKitGTK (e.g., headless WebView or isolated browser process).
