//! Ferramari — Iced control surface.
//!
//! The window is a plain D-Bus client of the running wallpaper server
//! (`io.github.ferramari.Ferramari.server`). It never touches X11 or mpv
//! directly, so the same binary works whether the server was started by the
//! desktop session, the tray, or a terminal.
//!
//! Reading is a `GetAll` poll on both interfaces once a second; writing is a
//! property `Set` or a method call. That is deliberately the same contract
//! existing scripts use: the UI is an ordinary bus client, not a privileged
//! one.
//!
//! Run: `cargo run -p ferramari-ui` (the server must be running).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use iced::border::Radius as BorderRadius;
use iced::widget::{
    button, container, image, pick_list, rule, scrollable, slider, text, text_input, toggler,
    Column, Container, Row, Space,
};
use iced::{
    Alignment, Background, Border, Color, ContentFit, Element, Length, Padding, Shadow, Size,
    Subscription, Task, Theme, Vector,
};

use ferramari_core::catalogue::{self, Category, Listing, Wallpaper};
use ferramari_core::config::Config;
use ferramari_core::library::Library;
use ferramari_core::metrics::{Metric, Monitor, Sample};
use ferramari_core::paths;
use ferramari_core::thumbnails;
use ferramari_core::util::tr;
use ferramari_ui::client::{Server, ServerState};
use ferramari_ui::theme::{self, black_a, ferramari_theme, white_a, Semantics};

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 780.0;

// ============================================================================ //
// Pages                                                                        //
// ============================================================================ //

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Library,
    Online,
    Performance,
    Settings,
}

impl Page {
    const ALL: [Page; 5] = [
        Page::Home,
        Page::Library,
        Page::Online,
        Page::Performance,
        Page::Settings,
    ];

    /// Sidebar label. Short on purpose: the reference design keeps the rail
    /// compact and puts the full name in the page header.
    fn title(self) -> String {
        match self {
            Page::Home => tr("Home"),
            Page::Library => tr("Library"),
            Page::Online => tr("Online"),
            Page::Performance => tr("Performance"),
            Page::Settings => tr("Settings"),
        }
    }

    /// Sidebar glyph. Plain Unicode symbols: every Linux system ships a font
    /// covering them (DejaVu Sans), and cosmic-text reaches it through font
    /// fallback. The earlier codepoints lived in the Private Use Area and
    /// assumed a Material Symbols font that was never bundled — they rendered
    /// as tofu boxes.
    fn icon(self) -> char {
        match self {
            Page::Home => '\u{F015}',      // ⌂ house
            Page::Library => '\u{F008}',   // filmstrip
            Page::Online => '\u{F0AC}',    // globe
            Page::Performance => '\u{F201}', // activity trace
            Page::Settings => '\u{F013}',  // cog
        }
    }

    fn subtitle(self) -> String {
        match self {
            Page::Home => tr("Your animated wallpaper engine"),
            Page::Library => tr("Your local wallpapers"),
            Page::Online => tr("Browse and download wallpapers"),
            Page::Performance => tr("System monitoring and engine statistics"),
            Page::Settings => tr("Configure Ferramari to your needs"),
        }
    }
}

// ============================================================================ //
// Messages                                                                     //
// ============================================================================ //

#[derive(Clone)]
enum Message {
    Navigate(Page),
    Tick,

    Connected(Result<Server, String>),
    StateUpdated(Box<ServerState>),
    CommandFinished(Result<(), String>),
    PackageRequestFinished(Result<(), String>),

    TogglePlay,
    ToggleMute,
    ReloadWallpaper,
    FeelingLucky,

    SetVolume(i32),
    BlurChanged(i32),
    ApplyBlur,
    ToggleStatic(bool),
    TogglePauseMaximized(bool),
    ToggleMuteMaximized(bool),

    SetTheme(String),
    SetLanguage(String),
    SetProfile(String),
    SetHwdec(String),
    SetRotationMode(String),
    SetRotationInterval(i32),
    TogglePauseOnBattery(bool),
    SetIdleTimeout(i32),
    ToggleWebNetwork(bool),
    ToggleAutostart(bool),

    RefreshLibrary,
    ToggleFavorite(String),
    RemoveMedia(String),
    CollectionNameChanged(String),
    CreateCollection,
    RenameCollection(String, String),
    DeleteCollection(String),
    AddToCollection(String, String),
    RemoveFromCollection(String, String),
    Play(String),
    PlayOnMonitor(String, String),
    SetMonitorRotation(String, String),
    OpenFolder,
    PickVideo,
    VideoPicked(Option<PathBuf>),
    VideoImported(Result<PathBuf, String>),
    PickPackageSource,
    PackageSourcePicked(Option<PathBuf>),
    PickExportVideo,
    ExportVideoPicked(Option<PathBuf>),
    PickPackageParent,
    PackageParentPicked(Option<PathBuf>),
    PackageSourceChanged(String),
    PackageParentChanged(String),
    ImportPackage,
    ExportPackage,
    SignPackage,
    VerifyPackage,
    TrustPackageKey,

    OnlineSearchChanged(String),
    OnlineSearch,
    OnlineCategory(String),
    OnlineRefresh,
    OnlineMore,
    CatalogueLoaded(Result<Listing, String>, bool),
    Download(Wallpaper),
    Downloaded(String, Result<PathBuf, String>),
    PreviewLoaded(String, Option<PathBuf>),
}

// ============================================================================ //
// State                                                                        //
// ============================================================================ //

#[derive(Default)]
struct Online {
    items: Vec<Wallpaper>,
    categories: Vec<Category>,
    next: Option<String>,
    selected: Option<Category>,
    loading: bool,
    error: Option<String>,
    search: String,
    /// Localized placeholder, owned so `text_input` can borrow it from `self`.
    search_placeholder: String,
    downloading: Option<String>,
    /// Wallpaper id → local preview file. `None` means the download failed or
    /// the entry has no thumbnail; the card keeps its placeholder in that case
    /// and the loader will not retry it during this session.
    previews: HashMap<String, Option<PathBuf>>,
}

struct StatusLine {
    text: String,
    error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CollectionChoice {
    id: String,
    name: String,
}

impl std::fmt::Display for CollectionChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.name)
    }
}

struct App {
    page: Page,
    server: Option<Server>,
    state: ServerState,
    system_dark: bool,
    status: Option<StatusLine>,

    /// Last language code handed to gettext, to avoid re-applying it on every
    /// poll.
    applied_language: String,
    metrics: Monitor,
    sample: Option<Sample>,

    library: Library,
    collection_name: String,
    collection_placeholder: String,
    package_source: String,
    package_parent: String,
    /// Slider position for the static wallpaper blur. Committed with the
    /// explicit Apply button, mirroring the GTK control: regenerating the
    /// blurred frame on every pixel of drag would be wasteful.
    blur_draft: i32,
    blur_dirty: bool,

    online: Online,
    ticks: u64,

    /// Tracks the last user-initiated play/pause action. Used to ignore stale
    /// snapshots that contradict the user's intent (e.g. user clicked Resume
    /// but server hasn't started playback yet, so snapshot still says paused).
    pending_play: Option<bool>,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let library = Library::load();
        (
            Self {
                page: Page::Home,
                server: None,
                state: ServerState::default(),
                system_dark: theme::system_prefers_dark(),
                status: None,
                applied_language: String::new(),
                metrics: Monitor::default(),
                sample: None,
                library,
                collection_name: String::new(),
                collection_placeholder: tr("New collection"),
                package_source: String::new(),
                package_parent: String::new(),
                blur_draft: 5,
                blur_dirty: false,
                online: Online {
                    search_placeholder: tr("Search wallpapers…"),
                    ..Online::default()
                },
                ticks: 0,
                pending_play: None,
            },
            Task::perform(Server::connect(), Message::Connected),
        )
    }

    fn sem(&self) -> &'static Semantics {
        Semantics::resolve(&self.state.theme, self.system_dark)
    }

    fn send_package_request<F, Fut>(&mut self, request: F, needs_parent: bool) -> Task<Message>
    where
        F: FnOnce(Server, String, String) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let Some(server) = self.server.clone() else {
            self.status = Some(StatusLine {
                text: tr("Server is unavailable"),
                error: true,
            });
            return Task::none();
        };
        let source = self.package_source.trim().to_string();
        let parent = self.package_parent.trim().to_string();
        if source.is_empty() || (needs_parent && parent.is_empty()) {
            self.status = Some(StatusLine {
                text: tr("Enter the required package paths"),
                error: true,
            });
            return Task::none();
        }
        Task::perform(
            request(server, source, parent),
            Message::PackageRequestFinished,
        )
    }

    fn persist_library(&mut self) -> Task<Message> {
        if let Err(error) = self.library.save() {
            self.status = Some(StatusLine {
                text: error,
                error: true,
            });
            return Task::none();
        }
        let Some(server) = self.server.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { server.command("library_changed").await },
            Message::CommandFinished,
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Navigate(page) => {
                self.page = page;
                if page == Page::Online && self.online.items.is_empty() && !self.online.loading {
                    return self.load_catalogue(None, false);
                }
                Task::none()
            }

            Message::Tick => {
                self.ticks = self.ticks.wrapping_add(1);
                let Some(server) = self.server.clone() else {
                    // Reconnect every few seconds while the server is down.
                    if self.ticks.is_multiple_of(5) {
                        return Task::perform(Server::connect(), Message::Connected);
                    }
                    return Task::none();
                };
                if self.page == Page::Performance {
                    let pid = self.state.server_pid.unwrap_or_else(std::process::id);
                    self.sample = Some(self.metrics.sample_of(pid));
                }
                Task::perform(async move { server.snapshot().await }, |s| {
                    Message::StateUpdated(s.into())
                })
            }

            Message::Connected(Ok(server)) => {
                self.server = Some(server.clone());
                self.status = None;
                Task::perform(async move { server.snapshot().await }, |s| {
                    Message::StateUpdated(s.into())
                })
            }
            Message::Connected(Err(error)) => {
                self.status = Some(StatusLine {
                    text: error,
                    error: true,
                });
                Task::none()
            }

            Message::StateUpdated(state) => {
                let state_ref = state.as_ref();
                let reachable = state_ref.reachable;
                // Unwrap the Box into a clone to avoid keeping a dangling reference.
                let mut new_state = (*state).clone();
                if !new_state.operation_status.is_empty() {
                    self.status = Some(StatusLine {
                        text: new_state.operation_status.clone(),
                        error: new_state.operation_error,
                    });
                }
                if !reachable {
                    self.server = None;
                    self.applied_language.clear();
                    self.pending_play = None;
                }
                if !self.blur_dirty {
                    self.blur_draft = new_state.blur_radius;
                }
                // Apply the server's language whenever it differs from what
                // this window last applied. Both `""` and `"system"` mean
                // "follow the desktop locale", so they are normalised before
                // comparing — otherwise a value the server persists would
                // never match and every poll would re-apply (undo-loop),
                // while an external change would be ignored on first sight.
                let want = match new_state.language.as_str() {
                    "" => "system".to_string(),
                    other => other.to_string(),
                };
                if want != self.applied_language {
                    apply_language(&want);
                    self.applied_language = want;
                    self.online.search_placeholder = tr("Search wallpapers…");
                    self.collection_placeholder = tr("New collection");
                }
                // Ignore stale snapshots that contradict the user's pending
                // play/pause action. When the user clicks "Resume", we set
                // pending_play = Some(true). If the server responds with
                // is_playing = false (because mpv hasn't started yet), we
                // keep our optimistic true value.
                if let Some(pending) = self.pending_play.take() {
                    if new_state.is_playing != pending {
                        // Server's snapshot contradicts user's action — keep
                        // our optimistic value.
                        new_state.is_playing = pending;
                    }
                }
                self.state = new_state;
                Task::none()
            }

            Message::CommandFinished(result) => {
                if let Err(error) = result {
                    self.status = Some(StatusLine {
                        text: error,
                        error: true,
                    });
                }
                Task::none()
            }
            Message::PackageRequestFinished(result) => {
                self.status = Some(match result {
                    Ok(()) => StatusLine {
                        text: tr("Package request queued; processing continues in the server"),
                        error: false,
                    },
                    Err(error) => StatusLine {
                        text: error,
                        error: true,
                    },
                });
                Task::none()
            }

            // --- Playback --------------------------------------------------- //
            Message::TogglePlay => {
                // `pause_playback`/`start_playback` only touch the players; the
                // Eco plan recomputes them as `Active` on the next signal, so
                // the button would flip itself back. `is_paused_by_user` is the
                // state the policy honours, so the button writes that. The
                // intent comes from what is on screen, not from the flag: the
                // wallpaper can be paused by the Eco Engine while the user has
                // not paused anything, and then "Play" must clear the flag.
                let resume = !self.state.is_playing;
                self.state.is_playing = resume;
                self.state.is_paused_by_user = !resume;
                // Hold the expected value so the chip does not contradict the
                // button while the server catches up.
                self.pending_play = Some(resume);
                set_bool(self.server.as_ref(), "is_paused_by_user", !resume)
            }
            Message::ToggleMute => {
                let muted = !self.state.is_mute;
                self.state.is_mute = muted;
                set_bool(self.server.as_ref(), "is_mute", muted)
            }
            Message::ReloadWallpaper => call(self.server.as_ref(), "reload"),
            Message::FeelingLucky => call(self.server.as_ref(), "feeling_lucky"),

            Message::SetVolume(volume) => {
                self.state.volume = volume;
                set_i32(self.server.as_ref(), "volume", volume)
            }
            Message::BlurChanged(radius) => {
                self.blur_draft = radius;
                self.blur_dirty = true;
                Task::none()
            }
            Message::ApplyBlur => {
                self.blur_dirty = false;
                set_i32(self.server.as_ref(), "blur_radius", self.blur_draft)
            }
            Message::ToggleStatic(enabled) => {
                self.state.is_static_wallpaper = enabled;
                set_bool(self.server.as_ref(), "is_static_wallpaper", enabled)
            }
            Message::TogglePauseMaximized(enabled) => {
                self.state.is_pause_when_maximized = enabled;
                set_bool(self.server.as_ref(), "is_pause_when_maximized", enabled)
            }
            Message::ToggleMuteMaximized(enabled) => {
                self.state.is_mute_when_maximized = enabled;
                set_bool(self.server.as_ref(), "is_mute_when_maximized", enabled)
            }

            // --- Settings --------------------------------------------------- //
            Message::SetTheme(theme) => {
                self.state.theme = theme.clone();
                set_string(self.server.as_ref(), "theme", theme)
            }
            Message::SetLanguage(language) => {
                // Store the same normalised form the poll compares against, so
                // the user's choice is not immediately re-applied by `Tick`.
                let normalised = if language.is_empty() {
                    "system".to_string()
                } else {
                    language.clone()
                };
                apply_language(&normalised);
                self.applied_language = normalised;
                self.state.language = language.clone();
                self.online.search_placeholder = tr("Search wallpapers…");
                self.collection_placeholder = tr("New collection");
                set_string(self.server.as_ref(), "language", language)
            }
            Message::SetProfile(profile) => {
                self.state.playback_profile = profile.clone();
                set_string(self.server.as_ref(), "playback_profile", profile)
            }
            Message::SetHwdec(mode) => {
                self.state.hwdec = mode.clone();
                set_string(self.server.as_ref(), "hwdec", mode)
            }
            Message::SetRotationMode(mode) => {
                self.state.rotation_mode = mode.clone();
                set_string(self.server.as_ref(), "rotation_mode", mode)
            }
            Message::SetRotationInterval(minutes) => {
                let seconds = minutes.saturating_mul(60);
                self.state.rotation_interval_sec = seconds;
                set_i32(self.server.as_ref(), "rotation_interval_sec", seconds)
            }
            Message::TogglePauseOnBattery(enabled) => {
                self.state.pause_on_battery = enabled;
                set_bool(self.server.as_ref(), "pause_on_battery", enabled)
            }
            Message::SetIdleTimeout(minutes) => {
                let seconds = minutes.saturating_mul(60);
                self.state.idle_timeout_sec = seconds;
                set_i32(self.server.as_ref(), "idle_timeout_sec", seconds)
            }
            Message::ToggleWebNetwork(enabled) => {
                self.state.web_allow_network = enabled;
                set_bool(self.server.as_ref(), "web_allow_network", enabled)
            }
            Message::ToggleAutostart(enabled) => {
                self.state.autostart_enabled = enabled;
                set_bool(self.server.as_ref(), "autostart_enabled", enabled)
            }

            // --- Library ---------------------------------------------------- //
            Message::RefreshLibrary => {
                self.library = Library::load();
                Task::none()
            }
            Message::ToggleFavorite(path) => {
                self.library.toggle_favorite(&path);
                self.persist_library()
            }
            Message::RemoveMedia(path) => {
                if self.library.remove_media(&path) {
                    self.persist_library()
                } else {
                    Task::none()
                }
            }
            Message::CollectionNameChanged(name) => {
                self.collection_name = name;
                Task::none()
            }
            Message::CreateCollection => {
                if self
                    .library
                    .create_collection(&self.collection_name)
                    .is_some()
                {
                    self.collection_name.clear();
                    return self.persist_library();
                }
                Task::none()
            }
            Message::RenameCollection(id, name) => {
                if self.library.rename_collection(&id, &name) {
                    return self.persist_library();
                }
                Task::none()
            }
            Message::DeleteCollection(id) => {
                if self.library.delete_collection(&id) {
                    return self.persist_library();
                }
                Task::none()
            }
            Message::AddToCollection(id, path) => {
                if self.library.add_to_collection(&id, &path) {
                    return self.persist_library();
                }
                Task::none()
            }
            Message::RemoveFromCollection(id, path) => {
                if self.library.remove_from_collection(&id, &path) {
                    return self.persist_library();
                }
                Task::none()
            }
            Message::Play(path) => play_video(self.server.as_ref(), path),
            Message::PlayOnMonitor(path, monitor) => {
                play_video_on_monitor(self.server.as_ref(), path, monitor)
            }
            Message::SetMonitorRotation(monitor, collection) => {
                let Some(server) = self.server.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move { server.set_monitor_rotation(monitor, collection).await },
                    Message::CommandFinished,
                )
            }
            Message::OpenFolder => {
                open_folder(paths::video_wallpaper_dir());
                Task::none()
            }
            Message::PickVideo => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .add_filter("Video", &["mp4", "mkv", "webm", "mov", "avi", "gif"])
                        .pick_file()
                        .await
                        .map(|file| file.path().to_path_buf())
                },
                Message::VideoPicked,
            ),
            Message::VideoPicked(Some(source)) => Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || Library::import_video(&source))
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                },
                Message::VideoImported,
            ),
            Message::VideoPicked(None) => Task::none(),
            Message::VideoImported(Ok(path)) => {
                self.status = Some(StatusLine {
                    text: format!("{}: {}", tr("Added to library"), path.display()),
                    error: false,
                });
                play_video(self.server.as_ref(), path.to_string_lossy().into_owned())
            }
            Message::VideoImported(Err(error)) => {
                self.status = Some(StatusLine {
                    text: error,
                    error: true,
                });
                Task::none()
            }
            Message::PickPackageSource => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title(tr("Choose package directory"))
                        .pick_folder()
                        .await
                        .map(|folder| folder.path().to_path_buf())
                },
                Message::PackageSourcePicked,
            ),
            Message::PackageSourcePicked(Some(path)) => {
                self.package_source = path.to_string_lossy().into_owned();
                Task::none()
            }
            Message::PackageSourcePicked(None) => Task::none(),
            Message::PickExportVideo => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .add_filter("Video", &["mp4", "mkv", "webm", "mov", "avi", "gif"])
                        .set_title(tr("Choose video to export"))
                        .pick_file()
                        .await
                        .map(|file| file.path().to_path_buf())
                },
                Message::ExportVideoPicked,
            ),
            Message::ExportVideoPicked(Some(path)) => {
                self.package_source = path.to_string_lossy().into_owned();
                Task::none()
            }
            Message::ExportVideoPicked(None) => Task::none(),
            Message::PickPackageParent => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title(tr("Choose export destination"))
                        .pick_folder()
                        .await
                        .map(|folder| folder.path().to_path_buf())
                },
                Message::PackageParentPicked,
            ),
            Message::PackageParentPicked(Some(path)) => {
                self.package_parent = path.to_string_lossy().into_owned();
                Task::none()
            }
            Message::PackageParentPicked(None) => Task::none(),
            Message::PackageSourceChanged(path) => {
                self.package_source = path;
                Task::none()
            }
            Message::PackageParentChanged(path) => {
                self.package_parent = path;
                Task::none()
            }
            Message::ImportPackage => self.send_package_request(
                |server, path, _| async move { server.import_package(path).await },
                false,
            ),
            Message::SignPackage => self.send_package_request(
                |server, path, _| async move { server.sign_package(path).await },
                false,
            ),
            Message::VerifyPackage => self.send_package_request(
                |server, path, _| async move { server.verify_package(path).await },
                false,
            ),
            Message::TrustPackageKey => self.send_package_request(
                |server, path, _| async move { server.trust_key(path).await },
                false,
            ),
            Message::ExportPackage => self.send_package_request(
                |server, video, parent| async move { server.export_package(video, parent).await },
                true,
            ),

            // --- Online catalogue ------------------------------------------ //
            Message::OnlineSearchChanged(query) => {
                self.online.search = query;
                Task::none()
            }
            Message::OnlineSearch => {
                self.online.items.clear();
                self.online.next = None;
                self.online.error = None;
                self.online.selected = None;
                let query = self.online.search.trim().to_string();
                if query.is_empty() {
                    self.load_catalogue(None, false)
                } else {
                    self.online.loading = true;
                    Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                catalogue::search(&query).map_err(|error| error.to_string())
                            })
                            .await
                            .unwrap_or_else(|error| Err(error.to_string()))
                        },
                        |result| Message::CatalogueLoaded(result, false),
                    )
                }
            }
            Message::OnlineCategory(name) => {
                if name == tr("All wallpapers") {
                    self.online.selected = None;
                    self.load_catalogue(None, false)
                } else {
                    let selected = self
                        .online
                        .categories
                        .iter()
                        .find(|category| category.name == name)
                        .cloned();
                    let url = selected.as_ref().map(|category| category.url.clone());
                    self.online.selected = selected;
                    self.load_catalogue(url, false)
                }
            }
            Message::OnlineRefresh => {
                let url = self
                    .online
                    .selected
                    .as_ref()
                    .map(|category| category.url.clone());
                self.load_catalogue(url, false)
            }
            Message::OnlineMore => {
                let url = self.online.next.clone();
                self.load_catalogue(url, true)
            }
            Message::CatalogueLoaded(result, append) => {
                self.online.loading = false;
                match result {
                    Ok(listing) => {
                        if !listing.categories.is_empty() {
                            self.online.categories = listing.categories;
                        }
                        if append {
                            self.online.items.extend(listing.items.clone());
                        } else {
                            self.online.items = listing.items.clone();
                        }
                        self.online.next = listing.next;
                        self.online.error = None;
                        // Fetch the thumbnails of the items that just arrived.
                        self.load_previews()
                    }
                    Err(error) => {
                        self.online.error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::Download(wallpaper) => {
                self.online.downloading = Some(wallpaper.id.clone());
                let id = wallpaper.id.clone();
                let title = wallpaper.title.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let cancel = AtomicBool::new(false);
                            catalogue::download_video(&id, &cancel, &mut |_, _| {})
                                .map(|downloaded| downloaded.video)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                    },
                    move |result| Message::Downloaded(title.clone(), result),
                )
            }
            Message::Downloaded(title, result) => {
                self.online.downloading = None;
                match result {
                    Ok(path) => {
                        self.status = Some(StatusLine {
                            text: format!("{}: {}", tr("Downloaded"), title),
                            error: false,
                        });
                        play_video(self.server.as_ref(), path.to_string_lossy().into_owned())
                    }
                    Err(error) => {
                        self.online.error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::PreviewLoaded(id, path) => {
                self.online.previews.insert(id, path);
                Task::none()
            }
        }
    }

    /// Fetch the card thumbnails that are not cached yet. One task per item:
    /// `ensure_preview` is a blocking HTTP call, so it runs on the blocking
    /// pool and reports back through `PreviewLoaded`. An entry already present
    /// in `previews` is skipped, cached or not, so a refresh does not re-request
    /// images the page already shows.
    fn load_previews(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        for wallpaper in &self.online.items {
            if self.online.previews.contains_key(&wallpaper.id) {
                continue;
            }
            if wallpaper.thumb_url.is_empty() {
                self.online.previews.insert(wallpaper.id.clone(), None);
                continue;
            }
            let id = wallpaper.id.clone();
            let url = wallpaper.thumb_url.clone();
            tasks.push(Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || catalogue::ensure_preview(&url).ok())
                        .await
                        .unwrap_or(None)
                },
                move |path| Message::PreviewLoaded(id.clone(), path),
            ));
        }
        Task::batch(tasks)
    }

    fn load_catalogue(&mut self, url: Option<String>, append: bool) -> Task<Message> {
        self.online.loading = true;
        let url = url.unwrap_or_else(|| catalogue::listing_url(None, 1));
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    catalogue::load_listing(&url).map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Message::CatalogueLoaded(result, append),
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick)
    }

    fn theme(&self) -> Theme {
        ferramari_theme(self.system_dark, &self.state.theme)
    }

    // ---------------------------------------------------------------------- //
    // View                                                                   //
    // ---------------------------------------------------------------------- //

    fn view(&self) -> Element<'_, Message> {
        let sem = self.sem();
        container(
            Row::with_children(vec![self.sidebar(sem), self.content(sem)])
                .spacing(14)
                .padding(14),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| root_style(sem))
        .into()
    }

    fn sidebar(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let mut nav = Column::new().spacing(8);
        for page in Page::ALL {
            let active = page == self.page;
            nav = nav.push(
                button(
                    Row::new()
                        .spacing(12)
                        .align_y(Alignment::Center)
                        .push(
                            text(page.icon())
                                .size(17)
                                .font(iced::Font::with_name("FontAwesome"))
                                .color(if active {
                                    sem.accent_text
                                } else {
                                    sem.text_secondary
                                }),
                        )
                        .push(text(page.title()).size(13).color(if active {
                            sem.accent_text
                        } else {
                            sem.text_secondary
                        })),
                )
                .width(Length::Fill)
                .padding(Padding {
                    top: 11.0,
                    right: 16.0,
                    bottom: 11.0,
                    left: 16.0,
                })
                .on_press(Message::Navigate(page))
                .style(button_style(
                    sem,
                    if active {
                        ButtonKind::NavActive
                    } else {
                        ButtonKind::Nav
                    },
                )),
            );
        }

        // Connection status indicator with animated pulse.
        let (dot, connection) = if self.state.reachable {
            (sem.focus, tr("Ready"))
        } else {
            (sem.danger, tr("Unavailable"))
        };

        let content = Column::new()
            .spacing(8)
            .push(
                Column::new()
                    .spacing(3)
                    .push(text("Ferramari").size(22).color(sem.accent_text))
                    .push(text(tr("Video wallpaper")).size(11).color(sem.text_muted)),
            )
            .push(Space::new().height(16))
            .push(nav)
            .push(Space::new().height(Length::Fill))
            .push(
                container(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(
                            container(Space::new())
                                .width(Length::Fixed(6.0))
                                .height(Length::Fixed(6.0))
                                .style(move |_| container::Style {
                                    background: Some(Background::Color(dot)),
                                    ..Default::default()
                                }),
                        )
                        .push(text(connection).size(11).color(sem.text_muted)),
                )
                .padding(Padding {
                    top: 5.0,
                    right: 10.0,
                    bottom: 5.0,
                    left: 10.0,
                })
                .style(move |_| container::Style {
                    background: Some(Background::Color(sem.surface)),
                    border: Border {
                        color: sem.border,
                        width: 1.0,
                        radius: 10.0.into(),
                    },
                    ..Default::default()
                }),
            );

        container(content)
            .width(Length::Fixed(240.0))
            .height(Length::Fill)
            .padding(20)
            .style(move |_| sidebar_style(sem))
            .into()
    }

    fn content(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let mut header = Row::new().align_y(Alignment::Center).spacing(16).push(
            Column::new()
                .spacing(4)
                .width(Length::Fill)
                .push(text(self.page.title()).size(28).color(sem.text))
                .push(text(self.page.subtitle()).size(13).color(sem.text_muted)),
        );
        if let Some(status) = &self.status {
            header = header.push(
                container(text(status.text.clone()).size(11).color(if status.error {
                    sem.danger
                } else {
                    sem.text_secondary
                }))
                .padding(Padding {
                    top: 7.0,
                    right: 14.0,
                    bottom: 7.0,
                    left: 14.0,
                })
                .style(move |_| status_style(sem, status.error)),
            );
        }

        Column::new()
            .spacing(18)
            .width(Length::Fill)
            .push(header)
            .push(scrollable(self.page_view(sem)).height(Length::Fill))
            .into()
    }

    fn page_view(&self, sem: &'static Semantics) -> Element<'_, Message> {
        match self.page {
            Page::Home => self.home(sem),
            Page::Library => self.library(sem),
            Page::Online => self.online(sem),
            Page::Performance => self.performance(sem),
            Page::Settings => self.settings(sem),
        }
    }

    fn home(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let mode = match self.state.mode.as_str() {
            "MODE_VIDEO" => tr("Video"),
            "MODE_STREAM" => tr("Streaming"),
            "MODE_WEBPAGE" => tr("Web Page"),
            _ => tr("Wallpaper"),
        };
        let playback = if self.state.is_playing {
            tr("Playing")
        } else if self.state.is_paused_by_user {
            tr("Paused")
        } else {
            tr("No wallpaper set")
        };

        let play_label = if self.state.is_playing {
            tr("Pause")
        } else {
            tr("Play")
        };
        let mute_label = if self.state.is_mute {
            tr("Unmute")
        } else {
            tr("Mute")
        };

        // ── Now Playing: large hero card ────────────────────────────────
        let now_playing = container(
            Column::new()
                .spacing(16)
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(section_title(tr("Now playing"), sem))
                        .push(Space::new().width(Length::Fill))
                        .push(chip(mode, sem))
                        .push(chip(playback, sem)),
                )
                .push(
                    // Play/Pause — primary accent button, larger
                    container(primary_button(play_label, Message::TogglePlay, sem))
                        .width(Length::Fill),
                )
                .push(
                    // Mute, Reload, Lucky — secondary row
                    Row::new()
                        .spacing(8)
                        .push(secondary_button(mute_label, Message::ToggleMute, sem))
                        .push(secondary_button(
                            tr("Reload"),
                            Message::ReloadWallpaper,
                            sem,
                        ))
                        .push(secondary_button(
                            tr("I'm Feeling Lucky"),
                            Message::FeelingLucky,
                            sem,
                        )),
                )
                .push(slider_row(
                    tr("Volume"),
                    self.state.volume,
                    0..=100,
                    Message::SetVolume,
                    sem,
                )),
        )
        .padding(24);

        // ── Monitor grid (2 columns) ────────────────────────────────────
        let mut rotation_options = vec![CollectionChoice {
            id: String::new(),
            name: tr("No rotation"),
        }];
        rotation_options.extend(self.library.collections().iter().map(|collection| {
            CollectionChoice {
                id: collection.id.clone(),
                name: collection.name.clone(),
            }
        }));
        let mut names: Vec<&String> = self.state.monitor_states.keys().collect();
        names.sort();

        let monitors_section = if names.is_empty() {
            card(
                text(tr("No displays detected"))
                    .size(13)
                    .color(sem.text_muted),
                sem,
            )
        } else {
            // Build monitor cards into a vector then pair them for 2-column grid
            let mut card_vec = Vec::<Element<Message>>::new();
            for name in names {
                let state = self
                    .state
                    .monitor_states
                    .get(name)
                    .cloned()
                    .unwrap_or_default();
                let source = self
                    .state
                    .data_source
                    .get(name)
                    .or_else(|| self.state.data_source.get("Default"))
                    .cloned()
                    .unwrap_or_default();
                let file = PathBuf::from(source)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| tr("No wallpaper"));
                let selected_rotation = self.state.rotations.get(name).cloned().unwrap_or_default();
                let selected_choice = rotation_options
                    .iter()
                    .find(|choice| choice.id == selected_rotation)
                    .cloned();
                let monitor_name = name.clone();

                let card_content = Column::new()
                    .spacing(8)
                    .align_x(Alignment::Center)
                    .push(text(name.clone()).size(14).color(sem.text))
                    .push(text(file).size(11).color(sem.text_muted))
                    .push(chip(state, sem))
                    .push(
                        pick_list(rotation_options.clone(), selected_choice, move |choice| {
                            Message::SetMonitorRotation(monitor_name.clone(), choice.id)
                        })
                        .width(Length::Fill),
                    );

                card_vec.push(
                    container(card_content)
                        .padding(16)
                        .width(Length::Fill)
                        .style(move |_| floating_card_style(sem))
                        .into(),
                );
            }

            // Build paired rows (2 columns each)
            let mut grid_rows = Vec::<Element<Message>>::new();
            let mut iter = card_vec.into_iter();
            while let Some(first) = iter.next() {
                let mut row = Row::new().spacing(12).push(first);
                if let Some(second) = iter.next() {
                    row = row.push(second);
                }
                grid_rows.push(row.into());
            }

            container(
                Column::new()
                    .spacing(18)
                    .push(section_title(tr("Monitors"), sem))
                    .push(Column::new().spacing(12).extend(grid_rows)),
            )
            .padding(20)
        };

        // ── Quick toggles + Static wallpaper (single combined card) ─────
        let mut control_rows = Vec::new();
        control_rows.push(toggle_row(
            tr("Apply static wallpaper"),
            tr("Video frame applied as system wallpaper"),
            self.state.is_static_wallpaper,
            Message::ToggleStatic,
            sem,
        ));
        if self.blur_dirty {
            control_rows.push(slider_row(
                tr("Blur radius"),
                self.blur_draft,
                0..=40,
                Message::BlurChanged,
                sem,
            ));
            control_rows.push(primary_button(tr("Apply"), Message::ApplyBlur, sem));
        }
        control_rows.push(rule::horizontal(1).into());
        control_rows.push(toggle_row(
            tr("Pause when maximized"),
            tr("Auto-pause on fullscreen windows"),
            self.state.is_pause_when_maximized,
            Message::TogglePauseMaximized,
            sem,
        ));
        control_rows.push(toggle_row(
            tr("Mute when maximized"),
            tr("Auto-mute on fullscreen windows"),
            self.state.is_mute_when_maximized,
            Message::ToggleMuteMaximized,
            sem,
        ));

        let combined_controls = container(
            Column::new()
                .spacing(16)
                .push(section_title(tr("Controls"), sem))
                .extend(control_rows),
        )
        .padding(20);

        // ── Assemble all sections ───────────────────────────────────────
        Column::new()
            .spacing(18)
            .push(now_playing)
            .push(monitors_section)
            .push(combined_controls)
            .into()
    }

    fn library(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let package_source_placeholder = tr("Package directory or video to export");
        let package_parent_placeholder = tr("Export destination folder");

        // Per-entry actions need both target lists. Sorted so the pick-list
        // order is stable across redraws.
        let monitor_options: Vec<String> = {
            let mut names: Vec<String> = self.state.monitor_states.keys().cloned().collect();
            names.sort();
            names
        };
        let collection_options: Vec<CollectionChoice> = self
            .library
            .collections()
            .iter()
            .map(|collection| CollectionChoice {
                id: collection.id.clone(),
                name: collection.name.clone(),
            })
            .collect();

        // ── Recent videos list ──────────────────────────────────────────
        let mut recent_items = Vec::<Element<'_, Message>>::new();
        let history = self.library.history();
        if history.is_empty() {
            recent_items.push(
                text(tr("No recently used videos"))
                    .size(13)
                    .color(sem.text)
                    .into(),
            );
            recent_items.push(
                text(tr("Apply a video and it will appear here."))
                    .size(11)
                    .color(sem.text_muted)
                    .into(),
            );
        } else {
            for entry in history.iter().take(30) {
                let path = PathBuf::from(&entry.path);
                let title = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| entry.path.clone());

                let thumb_handle = thumbnails::generate(&path)
                    .filter(|thumb_path| thumb_path.is_file())
                    .map(image::Handle::from_path);

                let col = Column::new()
                    .spacing(2)
                    .push(text(title).size(13).color(sem.text))
                    .push(text(entry.path.clone()).size(10).color(sem.text_muted));

                let thumb_el: Element<'_, Message> = if let Some(handle) = thumb_handle {
                    image::Image::new(handle).width(Length::Fixed(60.0)).into()
                } else {
                    Space::new().width(Length::Fixed(60.0)).into()
                };

                let mut row = Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .width(Length::Fill)
                    .push(thumb_el)
                    .push(col);

                // Actions row
                row = row.push(secondary_button(
                    tr(if self.library.is_favorite(&entry.path) {
                        "Remove favorite"
                    } else {
                        "Add to favorites"
                    }),
                    Message::ToggleFavorite(entry.path.clone()),
                    sem,
                ));
                row = row.push(secondary_button(
                    tr("Play"),
                    Message::Play(entry.path.clone()),
                    sem,
                ));

                // Second action row: "add to" and "play on" targets. Kept on
                // its own line so the primary row stays scannable, matching
                // the Glassmorphism II density rules.
                let mut targets = Row::new().spacing(8).align_y(Alignment::Center);
                if !collection_options.is_empty() {
                    let options = collection_options.clone();
                    let path = entry.path.clone();
                    targets = targets.push(
                        pick_list(options, None::<CollectionChoice>, move |choice| {
                            Message::AddToCollection(choice.id, path.clone())
                        })
                        .width(Length::Fixed(180.0)),
                    );
                }
                if monitor_options.len() > 1 {
                    let options = monitor_options.clone();
                    let path = entry.path.clone();
                    targets = targets.push(
                        pick_list(options, None::<String>, move |monitor| {
                            Message::PlayOnMonitor(path.clone(), monitor)
                        })
                        .width(Length::Fixed(160.0)),
                    );
                }
                // Forget the entry everywhere (history, favourites, collections).
                // Deliberately last and right-aligned: it is destructive and
                // must not sit next to Play. The file on disk is untouched.
                targets = targets
                    .push(Space::new().width(Length::Fill))
                    .push(secondary_button(
                        tr("Remove from library"),
                        Message::RemoveMedia(entry.path.clone()),
                        sem,
                    ));
                // The second row always has at least "Remove from library",
                // so it is never empty.
                let card_body = Column::new().spacing(8).push(row).push(targets);

                recent_items.push(
                    container(card_body)
                        .padding(12)
                        .style(move |_| floating_card_style(sem))
                        .into(),
                );
            }
        }

        let recent_section = container(
            Column::new()
                .spacing(14)
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(12)
                        .push(
                            Column::new()
                                .spacing(2)
                                .push(section_title(tr("Recent"), sem))
                                .push(
                                    text(tr("Videos located in Ferramari folder"))
                                        .size(11)
                                        .color(sem.text_muted),
                                ),
                        )
                        .push(
                            Row::new()
                                .spacing(8)
                                .push(secondary_button(tr("Add video"), Message::PickVideo, sem))
                                .push(secondary_button(
                                    tr("Refresh"),
                                    Message::RefreshLibrary,
                                    sem,
                                ))
                                .push(secondary_button(
                                    tr("Open folder"),
                                    Message::OpenFolder,
                                    sem,
                                )),
                        ),
                )
                .push(Column::new().extend(recent_items)),
        )
        .padding(20);

        // ── Favorites section ───────────────────────────────────────────
        let mut fav_rows = Vec::<Element<'_, Message>>::new();
        if self.library.favorites().is_empty() {
            fav_rows.push(
                text(tr("No favorite videos"))
                    .size(12)
                    .color(sem.text_muted)
                    .into(),
            );
        } else {
            for path in self.library.favorites() {
                let title = PathBuf::from(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                fav_rows.push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(
                            Column::new()
                                .spacing(1)
                                .push(text(title).size(13).color(sem.text))
                                .push(text(path.clone()).size(10).color(sem.text_muted)),
                        )
                        .push(secondary_button(
                            tr("Play"),
                            Message::Play(path.clone()),
                            sem,
                        ))
                        .push(secondary_button(
                            tr("Unfavorite"),
                            Message::ToggleFavorite(path.clone()),
                            sem,
                        ))
                        .into(),
                );
            }
        }
        let favorites_section = container(
            Column::new()
                .spacing(14)
                .push(section_title(tr("Favorites"), sem))
                .push(Column::new().extend(fav_rows)),
        )
        .padding(20);

        // ── Collections section ─────────────────────────────────────────
        let mut coll_items = Vec::<Element<'_, Message>>::new();
        if self.library.collections().is_empty() {
            coll_items.push(
                text(tr("No collections yet"))
                    .size(12)
                    .color(sem.text_muted)
                    .into(),
            );
        } else {
            for collection in self.library.collections() {
                let collection_id = collection.id.clone();

                let mut item_rows = Vec::<Element<'_, Message>>::new();
                for path in &collection.items {
                    let p = PathBuf::from(path);
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.clone());
                    item_rows.push(
                        Row::new()
                            .spacing(8)
                            .align_y(Alignment::Center)
                            .push(text(name).size(12).width(Length::Fill))
                            .push(secondary_button(
                                tr("Play"),
                                Message::Play(path.clone()),
                                sem,
                            ))
                            .push(secondary_button(
                                tr("Remove"),
                                Message::RemoveFromCollection(collection_id.clone(), path.clone()),
                                sem,
                            ))
                            .into(),
                    );
                }

                let item_col: Vec<Element<Message>> = if item_rows.is_empty() {
                    vec![text(tr("No videos")).size(11).color(sem.text_muted).into()]
                } else {
                    item_rows
                };

                // Header row: the name is editable in place (v1 behaviour,
                // restored) and the collection can be deleted from here.
                let rename_id = collection_id.clone();
                let header = Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(
                        text_input(&tr("Collection name"), &collection.name)
                            .on_input(move |name| {
                                Message::RenameCollection(rename_id.clone(), name)
                            })
                            .width(Length::Fill),
                    )
                    .push(secondary_button(
                        tr("Delete"),
                        Message::DeleteCollection(collection_id.clone()),
                        sem,
                    ));

                let mut body = Column::new()
                    .spacing(12)
                    .push(header)
                    .push(rule::horizontal(1));
                body = body.extend(item_col);
                coll_items.push(card(body, sem).into());
            }
        }

        let collections_section = container(
            Column::new()
                .spacing(14)
                .push(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(section_title(tr("Collections"), sem))
                        .push(Space::new().width(Length::Fill))
                        .push(
                            text_input(self.collection_placeholder.as_str(), &self.collection_name)
                                .on_input(Message::CollectionNameChanged)
                                .width(Length::Fill),
                        )
                        .push(secondary_button(
                            tr("Create"),
                            Message::CreateCollection,
                            sem,
                        )),
                )
                .push(Column::new().extend(coll_items)),
        )
        .padding(20);

        // ── Packages section (minimal form) ─────────────────────────────
        let packages_section = container(Column::new()
            .spacing(12)
            .push(section_title(tr("Packages"), sem))
            .push(text(tr("Import, export, sign and verify .ferramari packages. Choose paths with native dialogs; operations run in the server."))
                .size(11).color(sem.text_muted))
            .push(Row::new().spacing(8).align_y(Alignment::Center)
                .push(text_input(package_source_placeholder.as_str(), &self.package_source)
                    .on_input(Message::PackageSourceChanged)
                    .width(Length::Fill))
                .push(secondary_button(tr("Select…"), Message::PickPackageSource, sem))
                // Exporting a package starts from a loose video file, so the
                // picker here accepts media, not just package folders.
                .push(secondary_button(tr("Video…"), Message::PickExportVideo, sem)))
            .push(Row::new().spacing(8).align_y(Alignment::Center)
                .push(text_input(package_parent_placeholder.as_str(), &self.package_parent)
                    .on_input(Message::PackageParentChanged)
                    .width(Length::Fill))
                .push(secondary_button(tr("Destination…"), Message::PickPackageParent, sem)))
            .push(Row::new().spacing(8)
                .push(secondary_button(tr("Import"), Message::ImportPackage, sem))
                .push(secondary_button(tr("Export"), Message::ExportPackage, sem))
                .push(secondary_button(tr("Sign"), Message::SignPackage, sem))
                .push(secondary_button(tr("Verify"), Message::VerifyPackage, sem))
                .push(secondary_button(tr("Trust key"), Message::TrustPackageKey, sem)))
        ).padding(20);

        // ── Assemble ────────────────────────────────────────────────────
        Column::new()
            .spacing(18)
            .push(recent_section)
            .push(favorites_section)
            .push(collections_section)
            .push(packages_section)
            .into()
    }

    fn online(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let mut category_options = vec![tr("All wallpapers")];
        category_options.extend(
            self.online
                .categories
                .iter()
                .map(|category| category.name.clone()),
        );
        let selected = self
            .online
            .selected
            .as_ref()
            .map(|category| category.name.clone())
            .unwrap_or_else(|| tr("All wallpapers"));

        // ── Search bar with glass card ──────────────────────────────────
        let search_card = container(
            Column::new()
                .spacing(10)
                .push(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(
                            text_input(
                                self.online.search_placeholder.as_str(),
                                &self.online.search,
                            )
                            .on_input(Message::OnlineSearchChanged)
                            .on_submit(Message::OnlineSearch)
                            .width(Length::Fill),
                        )
                        .push(secondary_button(tr("Search"), Message::OnlineSearch, sem)),
                )
                .push(
                    Row::new()
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .push(pick_list(
                            category_options,
                            Some(selected),
                            Message::OnlineCategory,
                        ))
                        .push(Space::new().width(Length::Fill))
                        .push(secondary_button(tr("Refresh"), Message::OnlineRefresh, sem)),
                ),
        )
        .padding(16);

        // ── Wallpaper items grid ────────────────────────────────────────
        let mut items = Vec::<Element<'_, Message>>::new();

        if let Some(error) = &self.online.error {
            items.push(text(error.clone()).size(12).color(sem.danger).into());
        } else if self.online.loading {
            items.push(
                text(tr("Loading wallpapers…"))
                    .size(12)
                    .color(sem.text_muted)
                    .into(),
            );
        } else if self.online.items.is_empty() {
            items.push(
                text(tr("No wallpapers found"))
                    .size(13)
                    .color(sem.text_muted)
                    .into(),
            );
        } else {
            for wallpaper in &self.online.items {
                let busy = self
                    .online
                    .downloading
                    .as_deref()
                    .is_some_and(|id| id == wallpaper.id);

                let download_btn: Element<'_, Message> = if busy {
                    // Not pressable during download: a second click would race
                    // on the same `.part` file.  We use `on_press_maybe(None)` so
                    // the button renders disabled and no message fires.
                    button(text(tr("Downloading…")).size(12))
                        .padding(Padding::new(8.0))
                        .on_press_maybe(None::<Message>)
                        .style(button_style(sem, ButtonKind::Secondary))
                        .into()
                } else {
                    button(text(tr("Install")).size(12))
                        .padding(Padding::new(8.0))
                        .on_press(Message::Download(wallpaper.clone()))
                        .style(button_style(sem, ButtonKind::Primary))
                        .into()
                };

                // Build the preview image if we have a cached file.
                let preview_el: Option<Element<'_, Message>> = self
                    .online
                    .previews
                    .get(&wallpaper.id)
                    .and_then(|path| path.as_ref())
                    .map(image::Handle::from_path)
                    .map(|handle| {
                        image::Image::new(handle)
                            .width(Length::Fixed(120.0))
                            .height(Length::Fixed(80.0))
                            .content_fit(ContentFit::Cover)
                            .border_radius(BorderRadius::from(8.0_f32))
                            .into()
                    });

                let item_row = Row::new()
                    .spacing(14)
                    .align_y(Alignment::Center)
                    .width(Length::Fill)
                    .push(
                        Column::new()
                            .spacing(2)
                            .push(text(&wallpaper.title).size(14).color(sem.text))
                            .push(
                                Row::new()
                                    .spacing(8)
                                    .push(chip(wallpaper.category.clone(), sem)),
                            ),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(
                        preview_el
                            .unwrap_or_else(|| Space::new().width(Length::Fixed(120.0)).into()),
                    )
                    .push(download_btn);

                items.push(
                    container(item_row)
                        .padding(14)
                        .style(move |_| floating_card_style(sem))
                        .into(),
                );
            }
            if self.online.next.is_some() {
                items.push(
                    Row::new()
                        .push(secondary_button(tr("Load more"), Message::OnlineMore, sem))
                        .into(),
                );
            }
        }

        Column::new()
            .spacing(18)
            .push(search_card)
            .push(container(Column::new().spacing(10).extend(items)))
            .into()
    }

    fn performance(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let Some(sample) = self.sample else {
            return container(
                text(tr("Waiting for the first sample…"))
                    .size(14)
                    .color(sem.text_muted),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
        };

        // Each metric says why it is empty: no sampler here, no such driver,
        // the server is gone, or the first sample has no interval yet.
        let cpu = metric(&sample.cpu, |value| format!("{value:.1}%"));
        let memory = metric(&sample.memory, |bytes| format!("{} MiB", bytes / 1_048_576));
        let threads = metric(&sample.threads, |value| value.to_string());
        let processes = metric(&sample.processes, |value| value.to_string());
        let gpu = metric(&sample.gpu, |value| format!("{value:.1}%"));

        // Determine color based on usage level (if percentage)
        let cpu_color = match &sample.cpu {
            ferramari_core::metrics::Metric::Value(v) => {
                if *v > 80.0 {
                    sem.danger_tone
                } else if *v > 50.0 {
                    sem.warning_tone
                } else {
                    sem.success_tone
                }
            }
            _ => sem.text_muted,
        };
        let gpu_color = match &sample.gpu {
            ferramari_core::metrics::Metric::Value(v) => {
                if *v > 80.0 {
                    sem.danger_tone
                } else if *v > 50.0 {
                    sem.warning_tone
                } else {
                    sem.success_tone
                }
            }
            _ => sem.text_muted,
        };

        let stats = Row::new().spacing(14).extend([
            stat_tile(tr("CPU"), cpu, cpu_color, sem),
            stat_tile(tr("Memory"), memory, sem.text, sem),
            stat_tile(tr("Threads"), threads, sem.text, sem),
            stat_tile(tr("Processes"), processes, sem.text, sem),
            stat_tile(tr("GPU"), gpu, gpu_color, sem),
        ]);

        Column::new()
            .spacing(18)
            .push(container(stats))
            .push(
                container(
                    Column::new()
                        .spacing(10)
                        .push(section_title(tr("About metrics"), sem))
                        .push(
                            text(tr(
                                "Metrics cover the Ferramari process and its mpv/WebKit children.",
                            ))
                            .size(12)
                            .color(sem.text_muted),
                        ),
                )
                .padding(20),
            )
            .into()
    }

    fn settings(&self, sem: &'static Semantics) -> Element<'_, Message> {
        // Left column sections — build via Vec for conditional hwdec_status row
        let mut playback_rows = Vec::<Element<Message>>::new();
        // The server keeps the legacy identifiers on disk (`performance`,
        // `power`) so old versions can still read the config; the UI speaks the
        // newer names. Normalise before comparing so the selected segment
        // matches what the server actually reports.
        let active_profile =
            ferramari_core::eco::Profile::from_config(&self.state.playback_profile);
        playback_rows.push(segmented_row(
            tr("Profile"),
            tr("Quality prioritises smoothness; Eco reduces decoding"),
            vec![
                (
                    tr("Quality"),
                    active_profile == ferramari_core::eco::Profile::Quality,
                    Message::SetProfile("quality".into()),
                ),
                (
                    tr("Balanced"),
                    active_profile == ferramari_core::eco::Profile::Balanced,
                    Message::SetProfile("balanced".into()),
                ),
                (
                    tr("Eco"),
                    active_profile == ferramari_core::eco::Profile::Eco,
                    Message::SetProfile("eco".into()),
                ),
            ],
            sem,
        ));
        playback_rows.push(segmented_row(
            tr("Hardware decoding"),
            tr("Auto lets mpv choose; VA-API forces it; Off uses software"),
            vec![
                (
                    tr("Auto"),
                    self.state.hwdec == "auto",
                    Message::SetHwdec("auto".into()),
                ),
                (
                    "VA-API".into(),
                    self.state.hwdec == "vaapi",
                    Message::SetHwdec("vaapi".into()),
                ),
                (
                    tr("Off"),
                    self.state.hwdec == "none",
                    Message::SetHwdec("none".into()),
                ),
            ],
            sem,
        ));
        playback_rows.push(toggle_row(
            tr("Pause on battery"),
            tr("Stop playback when running on battery"),
            self.state.pause_on_battery,
            Message::TogglePauseOnBattery,
            sem,
        ));
        if let Some(status) = hwdec_status(&self.state, sem) {
            playback_rows.push(status);
        }

        // Right column rows — no conditional members, so build them in place.
        let rotation_rows = vec![
            slider_row(
                tr("Idle delay"),
                (self.state.idle_timeout_sec / 60).max(1),
                1..=60,
                Message::SetIdleTimeout,
                sem,
            ),
            segmented_row(
                tr("Mode"),
                tr("Per-monitor collection rotation"),
                vec![
                    (
                        tr("Off"),
                        self.state.rotation_mode == "off",
                        Message::SetRotationMode("off".into()),
                    ),
                    (
                        tr("In order"),
                        self.state.rotation_mode == "order",
                        Message::SetRotationMode("order".into()),
                    ),
                    (
                        tr("Random"),
                        self.state.rotation_mode == "random",
                        Message::SetRotationMode("random".into()),
                    ),
                ],
                sem,
            ),
            slider_row(
                tr("Interval"),
                (self.state.rotation_interval_sec / 60).max(1),
                1..=120,
                Message::SetRotationInterval,
                sem,
            ),
        ];

        let desktop_rows = vec![
            toggle_row(
                tr("Web wallpapers network access"),
                tr("Remote pages and subresources; local always allowed"),
                self.state.web_allow_network,
                Message::ToggleWebNetwork,
                sem,
            ),
            toggle_row(
                tr("Pause on maximized window"),
                tr("Auto-pause when maximized or fullscreen detected"),
                self.state.is_pause_when_maximized,
                Message::TogglePauseMaximized,
                sem,
            ),
            toggle_row(
                tr("Mute on maximized window"),
                tr("Auto-mute when maximized or fullscreen detected"),
                self.state.is_mute_when_maximized,
                Message::ToggleMuteMaximized,
                sem,
            ),
            rule::horizontal(1).into(),
            toggle_row(
                tr("Autostart after login"),
                tr("Start Ferramari with your desktop session"),
                self.state.autostart_enabled,
                Message::ToggleAutostart,
                sem,
            ),
        ];

        // A two-by-two grid of section cards. Four columns in a single row
        // gave every card about 215 px — less than a three-segment control
        // needs — so the segments spilled over the card edge and the rows
        // ended at different heights, which read as a diagonal "curve".
        // Two per row doubles the width and lets each card size itself to
        // its content, so the bottoms line up.
        let appearance = settings_card(
            tr("Appearance"),
            vec![
                segmented_row(
                    tr("Theme"),
                    tr("Window and control appearance"),
                    vec![
                        (
                            tr("System"),
                            self.state.theme == "system",
                            Message::SetTheme("system".into()),
                        ),
                        (
                            tr("Light"),
                            self.state.theme == "light",
                            Message::SetTheme("light".into()),
                        ),
                        (
                            tr("Dark"),
                            self.state.theme == "dark",
                            Message::SetTheme("dark".into()),
                        ),
                    ],
                    sem,
                ),
                segmented_row(
                    tr("Language"),
                    tr("Interface language"),
                    vec![
                        (
                            tr("System"),
                            self.state.language.is_empty(),
                            Message::SetLanguage(String::new()),
                        ),
                        (
                            "English".into(),
                            self.state.language == "en",
                            Message::SetLanguage("en".into()),
                        ),
                        (
                            "Русский".into(),
                            self.state.language == "ru",
                            Message::SetLanguage("ru".into()),
                        ),
                        (
                            "日本語".into(),
                            self.state.language == "ja",
                            Message::SetLanguage("ja".into()),
                        ),
                    ],
                    sem,
                ),
            ],
            sem,
        );
        let playback = settings_card(tr("Playback"), playback_rows, sem);
        let rotation = settings_card(tr("Rotation"), rotation_rows, sem);
        let desktop = settings_card(tr("Desktop integration"), desktop_rows, sem);

        Column::new()
            .spacing(18)
            .push(
                Row::new()
                    .spacing(18)
                    .align_y(Alignment::Start)
                    .width(Length::Fill)
                    .push(appearance)
                    .push(playback),
            )
            .push(
                Row::new()
                    .spacing(18)
                    .align_y(Alignment::Start)
                    .width(Length::Fill)
                    .push(rotation)
                    .push(desktop),
            )
            .into()
    }
}

// ============================================================================ //
// D-Bus helpers                                                                //
// ============================================================================ //

fn call(server: Option<&Server>, method: &'static str) -> Task<Message> {
    let Some(server) = server.cloned() else {
        return Task::none();
    };
    Task::perform(
        async move { server.command(method).await },
        Message::CommandFinished,
    )
}

fn set_i32(server: Option<&Server>, property: &'static str, value: i32) -> Task<Message> {
    let Some(server) = server.cloned() else {
        return Task::none();
    };
    Task::perform(
        async move { server.set_i32(property, value).await },
        Message::CommandFinished,
    )
}

fn set_bool(server: Option<&Server>, property: &'static str, value: bool) -> Task<Message> {
    let Some(server) = server.cloned() else {
        return Task::none();
    };
    Task::perform(
        async move { server.set_bool(property, value).await },
        Message::CommandFinished,
    )
}

fn set_string(server: Option<&Server>, property: &'static str, value: String) -> Task<Message> {
    let Some(server) = server.cloned() else {
        return Task::none();
    };
    Task::perform(
        async move { server.set_string(property, value).await },
        Message::CommandFinished,
    )
}

fn play_video(server: Option<&Server>, path: String) -> Task<Message> {
    play_video_on_monitor(server, path, String::new())
}

fn play_video_on_monitor(server: Option<&Server>, path: String, monitor: String) -> Task<Message> {
    let Some(server) = server.cloned() else {
        return Task::none();
    };
    Task::perform(
        async move { server.command_args("video", &(path, monitor)).await },
        Message::CommandFinished,
    )
}

// ============================================================================ //
// Theme + widgets                                                              //
// ============================================================================ //

#[derive(Clone, Copy, PartialEq)]
enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Nav,
    NavActive,
    Icon, // circular icon-only button (play, refresh, etc.)
}

// --------------------------------------------------------------------------- //
// Container styles                                                            //
// --------------------------------------------------------------------------- //

/// Root background — deep solid fill matching the backdrop tone.
fn root_style(sem: &'static Semantics) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Color(sem.bg_bottom)),
        text_color: Some(sem.text),
        ..Default::default()
    }
}

/// Floating glass card — default (no hover) variant.
fn floating_card_style(sem: &'static Semantics) -> iced::widget::container::Style {
    container::Style {
        background: Some(Background::Color(sem.surface)),
        border: Border {
            color: white_a(0.11),
            width: 1.5,
            radius: 20.0.into(),
        },
        shadow: Shadow {
            color: black_a(0.24),
            offset: Vector::new(0.0, 10.0),
            blur_radius: 32.0,
        },
        text_color: Some(sem.text),
        ..Default::default()
    }
}

/// Sidebar panel — stronger surface, brand accent tint, directional shadow.
fn sidebar_style(sem: &'static Semantics) -> iced::widget::container::Style {
    container::Style {
        background: Some(Background::Color(sem.surface_strong)),
        border: Border {
            color: sem.border,
            width: 1.0,
            radius: 20.0.into(),
        },
        shadow: Shadow {
            color: sem.card_shadow,
            offset: Vector::new(-4.0, 0.0),
            blur_radius: 28.0,
        },
        text_color: Some(sem.text),
        ..Default::default()
    }
}

/// Status line pill — error or success tone.
fn status_style(sem: &'static Semantics, error: bool) -> iced::widget::container::Style {
    let (bg, border_c, text_c) = if error {
        (sem.danger_soft, sem.danger, sem.danger)
    } else {
        (sem.accent_soft, sem.accent_border, sem.text_secondary)
    };
    container::Style {
        background: Some(Background::Color(bg)),
        border: Border {
            color: border_c,
            width: 1.0,
            radius: 12.0.into(),
        },
        text_color: Some(text_c),
        ..Default::default()
    }
}

/// Stat tile on the Performance page.
fn stat_tile_style(sem: &'static Semantics, value_color: Color) -> iced::widget::container::Style {
    container::Style {
        background: Some(Background::Color(sem.tile_bg)),
        border: Border {
            color: sem.border,
            width: 1.0,
            radius: 14.0.into(),
        },
        shadow: Shadow {
            color: sem.shadow_soft,
            offset: Vector::new(0.0, 6.0),
            blur_radius: 18.0,
        },
        text_color: Some(value_color),
        ..Default::default()
    }
}

fn button_style(
    sem: &'static Semantics,
    kind: ButtonKind,
) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (background, text_color, border_color, radius) = match (kind, hovered) {
            (ButtonKind::Primary, false) => {
                (sem.accent_bg, sem.accent_ink, Color::TRANSPARENT, 14.0)
            }
            (ButtonKind::Primary, true) => (
                sem.accent_bg_hover,
                sem.accent_ink,
                Color::TRANSPARENT,
                14.0,
            ),
            (ButtonKind::Secondary, false) => (sem.surface, sem.text, sem.border, 12.0),
            (ButtonKind::Secondary, true) => (sem.surface_hover, sem.text, sem.border_strong, 12.0),
            (ButtonKind::Ghost, false) => (
                Color::TRANSPARENT,
                sem.text_secondary,
                Color::TRANSPARENT,
                10.0,
            ),
            (ButtonKind::Ghost, true) => (sem.surface_hover, sem.text, sem.border_strong, 10.0),
            (ButtonKind::Nav, false) => (
                Color::TRANSPARENT,
                sem.text_secondary,
                Color::TRANSPARENT,
                10.0,
            ),
            (ButtonKind::Nav, true) => (sem.icon_hover, sem.text, Color::TRANSPARENT, 10.0),
            (ButtonKind::NavActive, false) => {
                (sem.glow_accent, sem.accent_text, sem.accent_border, 12.0)
            }
            (ButtonKind::NavActive, true) => {
                (sem.glow_accent, sem.accent_text, sem.accent_border, 12.0)
            }
            (ButtonKind::Icon, false) => (
                Color::TRANSPARENT,
                sem.text_secondary,
                Color::TRANSPARENT,
                18.0,
            ),
            (ButtonKind::Icon, true) => (sem.icon_hover, sem.text, sem.border, 18.0),
        };
        button::Style {
            background: Some(Background::Color(background)),
            text_color,
            border: Border {
                color: border_color,
                width: if border_color == Color::TRANSPARENT && kind != ButtonKind::Icon {
                    0.0
                } else {
                    1.0
                },
                radius: radius.into(),
            },
            ..Default::default()
        }
    }
}

fn card<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    sem: &'static Semantics,
) -> Container<'a, M> {
    container(content)
        .padding(20)
        .width(Length::Fill)
        .style(move |_| floating_card_style(sem))
}

/// A Settings section: title plus its rows, as a glass card that shares half
/// of a `Row` with its sibling. The card stretches to the row height so the
/// two cards in a line end at the same edge instead of stepping down.
fn settings_card<'a, M: 'a>(
    title: String,
    rows: Vec<Element<'a, M>>,
    sem: &'static Semantics,
) -> Element<'a, M> {
    container(
        Column::new()
            .spacing(16)
            .push(section_title(title, sem))
            .extend(rows)
            .push(Space::new().height(Length::Fill)),
    )
    .padding(20)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| floating_card_style(sem))
    .into()
}

fn section_title<M>(label: String, sem: &'static Semantics) -> Element<'static, M> {
    // `text` owns the string, so the title needs no borrow of `self`.
    text(label).size(15).color(sem.text).into()
}

fn chip<'a, M: 'a>(label: String, sem: &'static Semantics) -> Element<'a, M> {
    container(text(label).size(11).color(sem.text_secondary))
        .padding(Padding {
            top: 3.0,
            right: 8.0,
            bottom: 3.0,
            left: 8.0,
        })
        .style(move |_| iced::widget::container::Style {
            background: Some(Background::Color(sem.accent_soft)),
            border: Border {
                color: sem.accent_border,
                width: 1.0,
                radius: 7.0.into(),
            },
            text_color: Some(sem.text_secondary),
            ..Default::default()
        })
        .into()
}

fn primary_button<'a, M: Clone + 'a>(
    label: String,
    message: M,
    sem: &'static Semantics,
) -> Element<'a, M> {
    button(text(label).size(12))
        .padding(Padding {
            top: 8.0,
            right: 14.0,
            bottom: 8.0,
            left: 14.0,
        })
        .on_press(message)
        .style(button_style(sem, ButtonKind::Primary))
        .into()
}

fn secondary_button<'a, M: Clone + 'a>(
    label: String,
    message: M,
    sem: &'static Semantics,
) -> Element<'a, M> {
    button(text(label).size(12))
        .padding(Padding {
            top: 8.0,
            right: 14.0,
            bottom: 8.0,
            left: 14.0,
        })
        .on_press(message)
        .style(button_style(sem, ButtonKind::Secondary))
        .into()
}

fn toggle_row<'a, M: Clone + 'a>(
    title: String,
    hint: String,
    value: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
    sem: &'static Semantics,
) -> Element<'a, M> {
    Row::new()
        .spacing(14)
        .align_y(Alignment::Center)
        .push(
            Column::new()
                .spacing(2)
                .width(Length::Fill)
                .push(text(title).size(13).color(sem.text))
                .push(text(hint).size(11).color(sem.text_muted)),
        )
        .push(toggler(value).on_toggle(on_toggle))
        .into()
}

fn slider_row<'a, M: Clone + 'a>(
    title: String,
    value: i32,
    range: std::ops::RangeInclusive<i32>,
    on_change: impl Fn(i32) -> M + 'a,
    sem: &'static Semantics,
) -> Element<'a, M> {
    Column::new()
        .spacing(6)
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(text(title).size(13).color(sem.text).width(Length::Fill))
                .push(text(value.to_string()).size(12).color(sem.text_muted)),
        )
        .push(slider(range, value, on_change))
        .into()
}

fn segmented_row<'a, M: Clone + 'a>(
    title: String,
    hint: String,
    options: Vec<(String, bool, M)>,
    sem: &'static Semantics,
) -> Element<'a, M> {
    let mut row = Row::new().spacing(6).align_y(Alignment::Center);
    for (label, selected, message) in options {
        row = row.push(
            button(text(label).size(12))
                .padding(Padding {
                    top: 6.0,
                    right: 12.0,
                    bottom: 6.0,
                    left: 12.0,
                })
                .on_press(message)
                .style(button_style(
                    sem,
                    if selected {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Ghost
                    },
                )),
        );
    }
    Row::new()
        .spacing(14)
        .align_y(Alignment::Center)
        .push(
            Column::new()
                .spacing(2)
                .width(Length::Fill)
                .push(text(title).size(13).color(sem.text))
                .push(text(hint).size(11).color(sem.text_muted)),
        )
        .push(row)
        .into()
}

/// What the players are really decoding with, next to the setting.
///
/// The setting alone is not the truth: mpv accepts a hardware API it cannot
/// use and decodes on the CPU silently. The server reports the running
/// decoder over D-Bus, so the page can show whether the choice is honoured
/// instead of leaving the user to guess from CPU load.
///
/// Returns `None` when there is nothing to tell. An option rather than an
/// empty row keeps the card from gaining a blank line that shifts every
/// control below it while a file is loading.
fn hwdec_status(state: &ServerState, sem: &'static Semantics) -> Option<Element<'static, Message>> {
    use ferramari_core::hwdec::{HwDec, HwDecCheck};

    let current = HwDec::from_report(
        (!state.hwdec_running.is_empty()).then_some(state.hwdec_running.as_str()),
    );
    let check = HwDecCheck::evaluate(&state.hwdec, &current);
    if check.is_silent() {
        // Nothing decoding yet, or exactly what was asked for.
        return None;
    }
    // "auto landed on the CPU" is a fact, not a fault; anything else
    // contradicts an explicit request and reads as a warning.
    let color = if check == HwDecCheck::AutoIsSoftware {
        sem.text_muted
    } else {
        sem.danger
    };
    let mut line = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(text(tr(check.message())).size(11).color(color));
    if let Some(api) = check.decoder() {
        // The API in use, for example "(vulkan)": the detail to quote in a
        // bug report, and the proof the setting was not simply ignored.
        line = line.push(text(format!("({api})")).size(11).color(sem.text_muted));
    }
    Some(line.into())
}

/// Render one metric, naming the reason when there is no measurement. A dash
/// with no explanation reads like a bug; the reason tells the user whether a
/// restart, a driver or a second sample would fix it.
fn metric<T, F>(metric: &Metric<T>, format: F) -> String
where
    F: FnOnce(&T) -> String,
{
    match metric {
        Metric::Value(value) => format(value),
        Metric::Unsupported => tr("Not measurable here"),
        Metric::Unavailable => tr("Unavailable"),
        Metric::Pending => tr("Measuring…"),
    }
}

/// Performance stat tile with color-coded value indicator.
fn stat_tile<'a>(
    label: String,
    value: String,
    value_color: Color,
    sem: &'static Semantics,
) -> Element<'a, Message> {
    container(
        Column::new()
            .spacing(6)
            .push(text(label).size(11).color(sem.text_muted))
            .push(text(value).size(24).color(value_color)),
    )
    .width(Length::Fill)
    .padding(16)
    .style(move |_| stat_tile_style(sem, value_color))
    .into()
}

// ============================================================================ //
// i18n                                                                         //
// ============================================================================ //

fn open_folder(path: PathBuf) {
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("explorer");
    let _ = command.arg(path).spawn();
}

fn init_i18n() {
    // SAFETY: called from the main thread before the event loop starts.
    unsafe {
        let _ = gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    }
    let _ = gettextrs::bindtextdomain(paths::TRANSLATION_DOMAIN, locale_dir());
    let _ = gettextrs::textdomain(paths::TRANSLATION_DOMAIN);
}

/// Switch the gettext catalog at runtime. `""`/`system` follows the desktop
/// locale; anything else forces `en`, `ru` or `ja`.
fn apply_language(code: &str) {
    match code {
        "" | "system" => std::env::remove_var("LANGUAGE"),
        other => std::env::set_var("LANGUAGE", other),
    }
    // gettext looks the catalog up using the current locale, so re-running
    // setlocale after touching LANGUAGE is what makes the switch visible.
    unsafe {
        let _ = gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    }
}

fn locale_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("FERRAMARI_LOCALEDIR") {
        return dir.into();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(prefix) = exe.parent().and_then(|bin| bin.parent()) {
            let candidate = prefix.join("share/locale");
            if candidate.is_dir() {
                return candidate;
            }
        }
    }
    PathBuf::from("/usr/share/locale")
}

const UI_INSTANCE_NAME: &str = "io.github.ferramari.Ferramari.ui";

/// Keep a D-Bus name owned for the lifetime of the UI process. The server may
/// ask to launch the UI more than once (tray activation, desktop entry, CLI),
/// but only one control window should exist per session.
fn acquire_ui_instance() -> Result<zbus::blocking::Connection, String> {
    let connection = zbus::blocking::Connection::session().map_err(|error| error.to_string())?;
    let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
    connection
        .request_name_with_flags(UI_INSTANCE_NAME, flags)
        .map_err(|error| format!("another Ferramari control window is already running: {error}"))?;
    Ok(connection)
}

fn main() -> iced::Result {
    if let Some(language) = Config::language_override() {
        apply_language(&language);
    }
    init_i18n();

    // Keep this connection in scope until Iced exits; dropping it releases
    // the well-known name so the control surface can be opened again.
    let _instance = match acquire_ui_instance() {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("[GUI] {error}");
            return Ok(());
        }
    };

    iced::application(App::boot, App::update, App::view)
        .title("Ferramari")
        .theme(App::theme)
        .subscription(App::subscription)
        .window_size(Size::new(WIDTH, HEIGHT))
        .centered()
        .run()
}
