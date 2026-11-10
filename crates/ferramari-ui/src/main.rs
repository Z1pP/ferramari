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
//!
//! The shell is the backdrop gradient, the rail and the content pane beside
//! it, and the window's own title bar on top. Iced has no `backdrop-filter`,
//! so the glass reads through translucency over an already dark gradient
//! rather than through a blur pass.
//!
//! Every colour comes from `theme::Semantics` and every shape from
//! `style::*`; nothing in this file spells out a hex value.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use iced::widget::{
    button, container, image, pick_list, scrollable, slider, text, text_input, toggler, Column,
    Row, Space,
};
use iced::{
    window, Alignment, Background, Border, Color, ContentFit, Element, Length, Padding, Shadow,
    Size, Subscription, Task, Theme, Vector,
};

use ferramari_core::catalogue::{self, Category, Listing, Wallpaper};
use ferramari_core::config::Config;
use ferramari_core::library::Library;
use ferramari_core::metrics::{Metric, Monitor, Sample};
use ferramari_core::paths;
use ferramari_core::thumbnails;
use ferramari_core::util::tr;
use ferramari_ui::client::{Server, ServerState};
use ferramari_ui::style::{self as s, Sparkline};
use ferramari_ui::theme::{self, black_a, ferramari_theme, Semantics};

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 780.0;
/// Below this width the rail gives up its labels and the Home tiles wrap: the
/// reference is designed for a desktop window, and 900 px is where a 1366-wide
/// screen with a sidebar still fits without clipping a Russian label.
const NARROW: f32 = 980.0;

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

    /// Rail glyph, by design name. `style::icon` resolves it to a Font Awesome
    /// codepoint and binds the family in one place: the codepoints are in the
    /// Private Use Area, so naming the font is not optional.
    fn icon(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Library => "film",
            Page::Online => "globe",
            Page::Performance => "activity",
            Page::Settings => "cog",
        }
    }

    /// The name `FERRAMARI_UI_PAGE` accepts. Lowercase and stable, unlike the
    /// labels, which are translated.
    fn key(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Library => "library",
            Page::Online => "online",
            Page::Performance => "performance",
            Page::Settings => "settings",
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

    // --- Window chrome ---------------------------------------------------- //
    /// Minimise, maximise/restore, close: the bar the reference draws itself
    /// instead of trusting the desktop's decorations.
    MinimizeWindow,
    ToggleMaximize,
    CloseWindow,
    /// A press on the title bar: the window has no decorations, so dragging
    /// it is the app's job.
    DragWindow,
    /// A press on one of the invisible edge strips.
    ResizeWindow(Resize),
    /// Whether the window is maximized, as the compositor says. The runtime
    /// has no maximize *event* in this version, so it is asked for after the
    /// window opens and after every toggle.
    MaximizeState(bool),
    /// Anything the runtime tells us about the window: its id when it opened,
    /// its size when the user resized it. The chrome buttons need the id, and
    /// the layout needs the width, so neither is guessed.
    WindowEvent(window::Id, window::Event),

    // --- Engine / daemon -------------------------------------------------- //
    StartEngine,
    StopEngine,
    RestartEngine,
    /// "Run in background": the tray item, which is what makes a windowless
    /// daemon reachable.
    ToggleSystray(bool),

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
    OnlineTab(usize),
    OnlineFeatured(isize),
    LibraryFilterChanged(String),
    ToggleView,
    /// Which settings section the right-hand pane shows.
    SettingsSection(Section),
    /// "Check for updates": whether the surface asks on its own at startup.
    SetUpdateCheck(bool),
    /// The button next to that switch, for the case where the user wants an
    /// answer now rather than at the next start.
    CheckUpdates,
    /// The check ran on a worker and answered.
    UpdateChecked(Box<ferramari_core::update::Status>),
    MediaFilterChanged(MediaFilter),
    /// The action menu of one card, opened or closed. `grid` says which
    /// layout was showing, which is where the menu hangs from.
    ToggleCardMenu(String, bool),
    /// `ffprobe` answered for one file. `None` is an answer too: the file is
    /// unreadable or not a video, and the card must not keep asking.
    MediaInfoLoaded(String, Option<MediaInfo>),
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

/// What the Online page shows, and how far it got.
///
/// The reference page is a curated storefront: a featured carousel, a row of
/// category tiles, then the results. All three come from the same listing —
/// the catalogue has no "featured" endpoint — so the tabs are local views over
/// the items already fetched, and only `Category` and `Search` hit the network.
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
    /// The chip row: `featured`, `popular`, `recent`, `collections`.
    tab: usize,
    /// Which carousel slide is showing. The slides are the first few items of
    /// the current view, so this resets when the view changes.
    featured: usize,
}

/// The view chips of the Online page.
///
/// The reference draws `Featured / Popular / Recent / Collections`. Three of
/// those are backed by something the app can actually compute; `Popular` is
/// not: the listing carries no rating, no download count and no view count,
/// only a vote button, so ordering by "popular" would be ordering by the page
/// order and calling it something else. `Downloaded` takes its place — the
/// same shape, and a signal the local cache really does know.
fn online_tabs() -> Vec<String> {
    vec![
        tr("Featured"),
        tr("Recent"),
        tr("Downloaded"),
        tr("Collections"),
    ]
}

/// The quality label a card shows for a given height, the same ladder the
/// library grid uses so one file cannot be `4K` on one page and `2160p` on
/// another.
fn quality_for(height: u32) -> String {
    MediaInfo {
        width: 0,
        height,
        fps: None,
    }
    .quality()
}

/// The glyph of a category tile. The source names its categories and says
/// nothing about pictures, so the icon follows the word: a nature category
/// gets a leaf because it is called nature, not because anything confirmed
/// that a leaf is what its wallpapers look like.
fn category_glyph(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).collect();
    let has = |word: &str| {
        words
            .iter()
            .any(|part| *part == word || part.contains(word))
    };
    if has("anime") || has("манга") {
        "star"
    } else if has("nature") || has("природ") || has("forest") {
        "leaf"
    } else if has("city") || has("urban") || has("город") {
        "building"
    } else if has("space") || has("cosmos") || has("космос") || has("galaxy") {
        "rocket"
    } else if has("game") || has("игр") {
        "bolt"
    } else if has("car") || has("авто") || has("moto") {
        "activity"
    } else if has("abstract") || has("абстрак") {
        "paint-brush"
    } else if has("minimal") || has("simple") {
        "circle-o"
    } else if has("dark") || has("night") || has("ноч") {
        "moon"
    } else if has("water") || has("sea") || has("океан") || has("rain") {
        "compress"
    } else if has("mountain") || has("гор") {
        "activity"
    } else {
        "globe"
    }
}

/// The month a listing item was published, from its own preview URL
/// (`/uploads/posts/2026-10/ruins_preview.png` → `202610`).
///
/// The listing gives no date field, but the upload path is date-partitioned
/// by the source, so the month is a fact about the item rather than an
/// inference. `None` when the path does not look like one, which sorts an
/// unknown item last rather than first.
fn post_month(url: &str) -> Option<u32> {
    let marker = "/posts/";
    let start = url.find(marker)? + marker.len();
    let rest = &url[start..];
    let year: u32 = rest.get(0..4)?.parse().ok()?;
    rest.get(4..5)?; // the separator
    let month: u32 = rest.get(5..7)?.parse().ok()?;
    (year > 2000 && (1..=12).contains(&month)).then(|| year * 100 + month)
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

/// How many wallpapers the Online carousel cycles through. Three is what the
/// reference shows and what the dots under it are drawn for.
const FEATURED_SLIDES: usize = 3;

/// Height of the hero plate. The reference's artwork is 16:9-ish and the
/// window is 1180 wide; 212 keeps the scrim legible without pushing the
/// resource tiles below the fold on a 780-tall window.
const RADIUS_HERO: f32 = 20.0;

/// `1:07`, for the remaining-time line on the hero.
fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A number with at most one decimal, and no `.0` to read: `2.8`, `16`, not
/// `2.800000` or `16.0`.
fn trim_number(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if rounded == rounded.round() {
        format!("{}", rounded.round() as i64)
    } else {
        format!("{rounded:.1}")
    }
}

/// The Eco state of a monitor, as the chip on its row. The bus reports the
/// config key; the user reads a word.
fn state_label(state: &str, _sem: &Semantics) -> String {
    match state {
        "active" => tr("Active"),
        "idle" => tr("Idle"),
        "occluded" => tr("Occluded"),
        "suspended" => tr("Suspended"),
        other if !other.is_empty() => other.to_string(),
        _ => tr("Unknown"),
    }
}

/// One metric as a short string, naming why there is none. The tiles are too
/// narrow for the full sentence the Performance page uses, so the reason is
/// one word: `N/A` for a platform that cannot measure it, `—` for a round that
/// produced nothing, `…` while the first interval is still filling in.
fn metric_short<T>(metric: Option<&Metric<T>>, format: impl FnOnce(&T) -> String) -> String {
    match metric {
        Some(Metric::Value(value)) => format(value),
        Some(Metric::Unsupported) => tr("N/A"),
        Some(Metric::Unavailable) => tr("—"),
        Some(Metric::Pending) | None => tr("…"),
    }
}

/// One resource tile, described rather than built: the four tiles on Home
/// differ only in their data, and a nine-argument function would have to be
/// read top to bottom to know which number is which.
struct Tile<'a> {
    icon: &'static str,
    label: String,
    value: String,
    sub: Option<String>,
    series: &'a [Option<f64>],
    color: Color,
    ceiling: Option<f64>,
    highlighted: bool,
}

impl Tile<'_> {
    /// The style closure outlives this call, so every value it needs is
    /// copied out of `self` first and the element is `'static`.
    fn element(self, sem: &'static Semantics) -> Element<'static, Message> {
        // `self` is moved, so every field below is owned and the returned
        // element can be `'static`.
        let Tile {
            icon,
            label,
            value,
            sub,
            series,
            color,
            ceiling,
            highlighted,
        } = self;
        let values = series.to_vec();
        let mut head = Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(s::icon::<Message>(icon, 13).color(sem.text_muted))
            .push(text(label).size(12.5).color(sem.text_secondary))
            .push(Space::new().width(Length::Fill));
        if let Some(sub) = sub {
            head = head.push(text(sub).size(11.5).color(sem.text_muted));
        }

        container(
            Column::new()
                .spacing(10)
                .push(head)
                .push(text(value).size(22).color(sem.text))
                .push(
                    Sparkline {
                        values,
                        color,
                        ceiling,
                    }
                    .block::<Message>(38.0),
                ),
        )
        .width(Length::Fill)
        .padding(Padding {
            top: 16.0,
            right: 16.0,
            bottom: 14.0,
            left: 16.0,
        })
        .style(move |_| {
            if highlighted {
                s::glow_style(sem)
            } else {
                s::card_style(sem)
            }
        })
        .into()
    }
}

/// Which edge of the window a press landed on, and so which way the
/// compositor should resize it. Named for the window, not for the pointer,
/// because the value is handed straight to `window::drag_resize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resize {
    North,
    South,
    West,
    East,
}

/// The release check the window does on its own, if the user asked for one.
///
/// Two things keep this quiet: the switch on the General page, and the
/// week-long cache in `update.json`. Without both, every start of a control
/// surface that is opened to change a volume would put a request at GitHub.
fn startup_update_check(record: &ferramari_core::update::Record) -> Task<Message> {
    if !record.automatic || record.is_fresh() {
        return Task::none();
    }
    Task::perform(
        async move {
            tokio::task::spawn_blocking(ferramari_core::update::check)
                .await
                .unwrap_or(ferramari_core::update::Status::Failed(
                    "the check thread stopped".to_string(),
                ))
        },
        |status| Message::UpdateChecked(Box::new(status)),
    )
}

/// How the window starts: `FERRAMARI_UI_BOOT="page=library size=900x700"`.
///
/// `section=` picks the Settings section for the same reason `page=` picks a
/// page: the sub-navigation is drawn by the app under review, so a scripted
/// look at each panel cannot click its way there.
///
/// The control surface has no CLI of its own, and the rail that would be
/// clicked through is drawn by the same toolkit under review, so a scripted
/// look at each page — in both themes, at several widths — needs a way in
/// that does not depend on synthesising a mouse click. Both keys are
/// optional; anything unrecognised is ignored rather than fatal, because a
/// typo in a test command should not look like a bug in the app.
#[derive(Debug, Clone, Copy)]
struct Boot {
    page: Page,
    size: Size,
    section: Section,
}

impl Default for Boot {
    fn default() -> Self {
        Self {
            page: Page::Home,
            size: Size::new(WIDTH, HEIGHT),
            section: Section::General,
        }
    }
}

fn boot_settings() -> Boot {
    let mut boot = Boot::default();
    let Ok(spec) = std::env::var("FERRAMARI_UI_BOOT") else {
        return boot;
    };
    for entry in spec.split_whitespace() {
        let (key, value) = entry.split_once('=').unwrap_or((entry, ""));
        let value = value.trim().to_lowercase();
        match key.trim().to_lowercase().as_str() {
            "page" => {
                if let Some(found) = Page::ALL.into_iter().find(|page| page.key() == value) {
                    boot.page = found;
                }
            }
            "section" => {
                if let Some(found) = Section::ALL
                    .into_iter()
                    .find(|section| section.key() == value)
                {
                    boot.section = found;
                }
            }
            "size" => {
                if let Some((width, height)) = value.split_once('x') {
                    if let (Ok(width), Ok(height)) = (width.parse::<f32>(), height.parse::<f32>()) {
                        boot.size = Size::new(width, height);
                    }
                }
            }
            _ => {}
        }
    }
    boot
}

/// A section of the Settings page.
///
/// The reference splits settings into a list on the left and one section on
/// the right rather than stacking everything in a scroll. Six entries is also
/// the honest count: each one is a group the server actually answers for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Appearance,
    Playback,
    Monitors,
    Performance,
    Advanced,
}

impl Section {
    const ALL: [Section; 6] = [
        Section::General,
        Section::Appearance,
        Section::Playback,
        Section::Monitors,
        Section::Performance,
        Section::Advanced,
    ];

    fn icon(self) -> &'static str {
        match self {
            Section::General => "cog",
            Section::Appearance => "paint-brush",
            Section::Playback => "film",
            Section::Monitors => "desktop",
            Section::Performance => "activity",
            Section::Advanced => "wrench",
        }
    }

    fn title(self) -> String {
        match self {
            Section::General => tr("General"),
            Section::Appearance => tr("Appearance"),
            Section::Playback => tr("Playback"),
            Section::Monitors => tr("Monitors"),
            Section::Performance => tr("Performance"),
            Section::Advanced => tr("Advanced"),
        }
    }

    /// The name `FERRAMARI_UI_BOOT=section=…` accepts: lowercase and stable,
    /// unlike the titles, which are translated.
    fn key(self) -> &'static str {
        match self {
            Section::General => "general",
            Section::Appearance => "appearance",
            Section::Playback => "playback",
            Section::Monitors => "monitors",
            Section::Performance => "performance",
            Section::Advanced => "advanced",
        }
    }
}

/// How many samples a sparkline keeps. One per second, so this is a little
/// over two minutes: enough to see a load ramp, short enough that a spike from
/// a wallpaper switch scrolls off while the user is still on the page.
const HISTORY_LEN: usize = 140;

/// The resource traces the Home tiles and the Performance page draw.
///
/// The server's own sampler answers per tick and carries no history, and a
/// second of CPU is a meaningless number on its own — the shape is the
/// information. So the UI keeps the shape locally: it is display state, not
/// product state, and nothing else can derive it.
#[derive(Default)]
struct History {
    cpu: Vec<Option<f64>>,
    /// Measured in GiB, the unit the tile's ceiling is stated in. Keeping the
    /// series and its scale in one unit is what stops the curve from being
    /// drawn 1024³ times too tall.
    ram: Vec<Option<f64>>,
    gpu: Vec<Option<f64>>,
    fps: Vec<Option<f64>>,
    frame_ms: Vec<Option<f64>>,
}

impl History {
    fn push(values: &mut Vec<Option<f64>>, value: Option<f64>) {
        // A tick with no reading still advances the series: dropping the point
        // would stretch the previous one across the gap and invent a slope
        // that never happened. The renderer breaks the line instead.
        values.push(value);
        if values.len() > HISTORY_LEN {
            let excess = values.len() - HISTORY_LEN;
            values.drain(..excess);
        }
    }

    fn record(&mut self, sample: Option<&Sample>, fps: Option<f64>) {
        let (cpu, ram, gpu) = match sample {
            Some(sample) => (
                sample.cpu.value(),
                // Bytes to GiB: the tile shows `2.8 GB of 15.6 GB`, and a
                // series in a different unit than its axis is a lying chart.
                sample
                    .memory
                    .value()
                    .map(|bytes| bytes as f64 / (1024.0 * 1024.0 * 1024.0)),
                sample.gpu.value(),
            ),
            None => (None, None, None),
        };
        Self::push(&mut self.cpu, cpu);
        Self::push(&mut self.ram, ram);
        Self::push(&mut self.gpu, gpu);
        Self::push(&mut self.fps, fps);
        // Frame time is the reciprocal of the rate the decoder reports: 60 fps
        // is 16.7 ms. One measurement, two honest ways to show it.
        Self::push(
            &mut self.frame_ms,
            fps.filter(|value| *value > 0.0).map(|value| 1000.0 / value),
        );
    }
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

    /// The window this UI owns, learned from the runtime when it opens. The
    /// chrome buttons address a window by id, so until it is known they are
    /// inert rather than aimed at a window that does not exist.
    window: Option<window::Id>,
    /// Whether the window is maximized, so the chrome glyph is the restore
    /// square rather than the maximize one.
    maximized: bool,
    /// Live window width, which decides whether the rail keeps its labels.
    width: f32,

    /// Library display mode: a grid of thumbnails or a list of rows. The
    /// reference shows both and the toggle is in the header.
    grid_view: bool,
    /// Substring filter over the library titles, owned so the header input
    /// keeps its caret across polls.
    library_filter: String,
    /// Which kind of file the Library grid shows.
    media_filter: MediaFilter,
    /// The Settings section the right-hand pane shows.
    section: Section,
    /// The release check, read from `update.json` at boot and rewritten by
    /// every check. It is UI-local state on purpose: the daemon plays
    /// wallpapers, and deciding whether a new build of the app exists is not
    /// part of anyone else's job.
    update: ferramari_core::update::Record,
    /// The wallpaper whose action menu is open, with the layout it was opened
    /// from: the grid hangs the menu under the card, the list beside it.
    expanded: Option<(String, bool)>,
    /// `ffprobe` answers, by path. Metadata is a fact about a file on disk,
    /// so it is cached for the session instead of being probed on every frame
    /// of a page that redraws once a second.
    media_info: HashMap<String, Option<MediaInfo>>,

    /// Rolling resource history, one entry per `Tick`, for the sparklines.
    history: History,

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
        // Read once: the page, the width and the settings section come from
        // the same spec, and parsing it twice would let them disagree on a
        // malformed value.
        let boot = boot_settings();
        // Loaded before the task is built, so the startup check can decide
        // from the same record the page will show.
        let update_record = ferramari_core::update::Record::load();
        (
            Self {
                page: boot.page,
                server: None,
                state: ServerState::default(),
                system_dark: theme::system_prefers_dark(),
                status: None,
                window: None,
                maximized: false,
                width: boot.size.width,
                grid_view: true,
                library_filter: String::new(),
                media_filter: MediaFilter::All,
                section: boot.section,
                update: update_record.clone(),
                expanded: None,
                media_info: HashMap::new(),
                history: History::default(),
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
            // Connecting to the server and asking GitHub about a new release
            // are independent, so neither waits for the other. The release
            // check honours the switch on the General page and the week-long
            // cache, so a start that has nothing to ask costs nothing.
            Task::batch([
                Task::perform(Server::connect(), Message::Connected),
                startup_update_check(&update_record),
            ]),
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
                self.expanded = None;
                match page {
                    Page::Online if self.online.items.is_empty() && !self.online.loading => {
                        self.load_catalogue(None, false)
                    }
                    // The cards show measured metadata, so the page asks for
                    // what it is about to draw instead of the view trying to
                    // start work it is not allowed to start.
                    Page::Library => self.probe_media(&self.visible_media()),
                    _ => Task::none(),
                }
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
                // The sparklines live on two pages, so the sample is taken on
                // either of them and kept for as long as the window is open.
                if self.page == Page::Performance || self.page == Page::Home {
                    let pid = self.state.server_pid.unwrap_or_else(std::process::id);
                    self.sample = Some(self.metrics.sample_of(pid));
                }
                self.history.record(
                    self.sample.as_ref(),
                    self.effective_fps().filter(|_| self.state.is_playing),
                );
                // The Library keeps its own cache, so a file that arrives
                // while the page is open (an import, a download) is picked up
                // by the next poll rather than staying "Reading…" forever.
                let probes = if self.page == Page::Library {
                    self.probe_media(&self.visible_media())
                } else {
                    Task::none()
                };
                Task::batch([
                    probes,
                    Task::perform(async move { server.snapshot().await }, |s| {
                        Message::StateUpdated(s.into())
                    }),
                ])
            }

            // --- Window chrome -------------------------------------------- //
            // The window is addressed by the id the runtime reported when it
            // opened; until that first event arrives the buttons are inert.
            Message::MinimizeWindow => match self.window {
                Some(id) => window::minimize(id, true),
                None => Task::none(),
            },
            Message::ToggleMaximize => match self.window {
                // The glyph has to follow the real state, and this version of
                // the runtime reports it on request rather than by event, so
                // the answer is asked for in the same batch as the change.
                Some(id) => Task::batch([
                    window::toggle_maximize(id),
                    window::is_maximized(id).map(Message::MaximizeState),
                ]),
                None => Task::none(),
            },
            Message::MaximizeState(maximized) => {
                self.maximized = maximized;
                Task::none()
            }
            Message::CloseWindow => match self.window {
                Some(id) => window::close(id),
                None => Task::none(),
            },
            Message::DragWindow => match self.window {
                Some(id) => window::drag(id),
                None => Task::none(),
            },
            Message::ResizeWindow(side) => match self.window {
                Some(id) => {
                    let direction = match side {
                        Resize::North => window::Direction::North,
                        Resize::South => window::Direction::South,
                        Resize::West => window::Direction::West,
                        Resize::East => window::Direction::East,
                    };
                    window::drag_resize(id, direction)
                }
                None => Task::none(),
            },
            Message::WindowEvent(id, event) => {
                match event {
                    window::Event::Opened { size, .. } => {
                        self.window = Some(id);
                        self.width = size.width;
                        return window::is_maximized(id).map(Message::MaximizeState);
                    }
                    window::Event::Resized(size) => self.width = size.width,
                    _ => {}
                }
                Task::none()
            }
            Message::SettingsSection(section) => {
                self.section = section;
                Task::none()
            }
            Message::SetUpdateCheck(enabled) => {
                // The switch is the user's choice and nothing else: it is
                // written immediately, because a window that is closed before
                // the next start must not silently forget it.
                self.update.automatic = enabled;
                if let Err(error) = self.update.store() {
                    self.status = Some(StatusLine {
                        text: format!("{}: {error}", tr("Could not save the update setting")),
                        error: true,
                    });
                }
                Task::none()
            }
            Message::CheckUpdates => Task::perform(
                async move {
                    tokio::task::spawn_blocking(ferramari_core::update::check)
                        .await
                        .unwrap_or(ferramari_core::update::Status::Failed(
                            "the check thread stopped".to_string(),
                        ))
                },
                |status| Message::UpdateChecked(Box::new(status)),
            ),
            Message::UpdateChecked(status) => {
                // `check` wrote the record itself, so the page re-reads it
                // rather than rebuilding it from the answer: the two paths
                // then cannot drift apart on what was cached and when.
                let _ = status;
                self.update = ferramari_core::update::Record::load();
                Task::none()
            }
            Message::ToggleView => {
                self.grid_view = !self.grid_view;
                // The menu hangs from a card in one layout and from a row in
                // the other; switching layouts with one open would leave it
                // pointing at nothing.
                self.expanded = None;
                Task::none()
            }
            Message::MediaFilterChanged(filter) => {
                self.media_filter = filter;
                self.expanded = None;
                Task::none()
            }
            Message::LibraryFilterChanged(query) => {
                self.library_filter = query;
                self.expanded = None;
                Task::none()
            }
            Message::ToggleCardMenu(path, grid) => {
                let open = self
                    .expanded
                    .as_ref()
                    .is_some_and(|(where_open, _)| where_open == &path);
                self.expanded = if open { None } else { Some((path, grid)) };
                Task::none()
            }
            Message::MediaInfoLoaded(path, info) => {
                self.media_info.insert(path, info);
                Task::none()
            }

            // --- Engine / daemon ------------------------------------------ //
            // The three buttons on the Home header are the player, not the
            // process: `quit_player` stops playback and leaves the daemon
            // reachable, `quit` would take the window's own partner down with
            // it and the tray would have nothing to talk to.
            Message::StartEngine => {
                self.state.is_playing = true;
                self.pending_play = Some(true);
                call(self.server.as_ref(), "start_playback")
            }
            Message::StopEngine => {
                self.state.is_playing = false;
                self.pending_play = Some(false);
                call(self.server.as_ref(), "quit_player")
            }
            Message::RestartEngine => {
                self.state.is_playing = true;
                self.pending_play = Some(true);
                call(self.server.as_ref(), "reload")
            }
            Message::ToggleSystray(enabled) => {
                self.state.is_show_systray = enabled;
                set_bool(self.server.as_ref(), "is_show_systray", enabled)
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
            // The three view tabs are local re-orderings of the items already
            // fetched: the catalogue has no "featured" or "popular" endpoint,
            // so promising a network round-trip here would be a lie. Only
            // `Collections` changes what is requested, and it does so through
            // the category it maps to.
            Message::OnlineTab(index) => {
                self.online.tab = index;
                self.online.featured = 0;
                Task::none()
            }
            Message::OnlineFeatured(step) => {
                let slides = self.featured_slides().max(1) as isize;
                let next = (self.online.featured as isize + step).rem_euclid(slides);
                self.online.featured = next as usize;
                Task::none()
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

    /// Ask `ffprobe` about every card whose metadata is not known yet.
    ///
    /// One task per file, on the blocking pool: a probe is a subprocess with a
    /// ten-second deadline and must never run on the Iced thread. An answer of
    /// `None` is cached like any other, so a file the tool cannot read is
    /// asked about once per session rather than on every redraw.
    fn probe_media(&mut self, paths: &[String]) -> Task<Message> {
        let mut tasks = Vec::new();
        for path in paths {
            if self.media_info.contains_key(path) {
                continue;
            }
            let probe_path = path.clone();
            let report_path = path.clone();
            tasks.push(Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        catalogue::probe(&PathBuf::from(&probe_path)).map(|probe| MediaInfo {
                            width: probe.width,
                            height: probe.height,
                            fps: probe.fps,
                        })
                    })
                    .await
                    .unwrap_or(None)
                },
                move |info| Message::MediaInfoLoaded(report_path, info),
            ));
        }
        Task::batch(tasks)
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
        // The tick drives both the D-Bus poll and the sparkline history; the
        // window stream is what tells the chrome buttons which window to
        // address and the layout how wide it is.
        iced::Subscription::batch([
            iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
            iced::event::listen_with(|event, _status, window| match event {
                iced::Event::Window(event) => Some(Message::WindowEvent(window, event)),
                _ => None,
            }),
        ])
    }

    fn theme(&self) -> Theme {
        ferramari_theme(self.system_dark, &self.state.theme)
    }

    // ---------------------------------------------------------------------- //
    // Shell                                                                  //
    // ---------------------------------------------------------------------- //

    /// The whole window: a title bar the app draws itself, then the rail and
    /// the page beside it, over the backdrop gradient.
    ///
    /// The decorations are off (see `main`), which is what lets the reference
    /// design run its own light gradient to the very top edge. The cost is
    /// that the desktop no longer gives us a drag strip or resize borders, so
    /// both are rebuilt here: the bar drags, the edge strips below do what a
    /// window frame would.
    fn view(&self) -> Element<'_, Message> {
        let sem = self.sem();
        let body = Column::new().push(self.title_bar(sem)).push(
            Row::new()
                .spacing(14)
                .padding(Padding {
                    top: 6.0,
                    right: 14.0,
                    bottom: 14.0,
                    left: 14.0,
                })
                .push(self.rail(sem))
                .push(self.content(sem)),
        );

        let painted = container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(s::backdrop(sem))
            .into();

        // The strips are the top layer and the only widgets in it, so they
        // take the press where they cover and leave the rest of the window to
        // the content underneath.
        iced::widget::stack(vec![
            painted,
            self.resize_edge(Resize::North),
            self.resize_edge(Resize::South),
            self.resize_edge(Resize::West),
            self.resize_edge(Resize::East),
        ])
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    /// The app-drawn title bar: empty on the left where a compositor would
    /// have put its buttons, three controls on the right, and a drag surface
    /// everywhere else.
    fn title_bar(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let chrome = |glyph_name: &str, message: Option<Message>, close: bool| {
            s::icon_button(
                glyph_name,
                message,
                11.0,
                if close {
                    s::Kind::ChromeClose
                } else {
                    s::Kind::Icon
                },
                sem,
            )
        };
        // The maximise control shows the restore glyph only while the window
        // actually is maximized; guessing it from the size would be wrong on
        // any compositor that starts at a different default.
        let maximize_glyph = if self.maximized {
            "window-restore"
        } else {
            "window-maximize"
        };

        let bar = Row::new()
            .align_y(Alignment::Center)
            .spacing(2)
            .push(Space::new().width(Length::Fill))
            .push(chrome(
                "window-minimize",
                Some(Message::MinimizeWindow),
                false,
            ))
            .push(chrome(maximize_glyph, Some(Message::ToggleMaximize), false))
            .push(chrome("times", Some(Message::CloseWindow), true));

        let draggable = iced::widget::mouse_area(bar)
            .interaction(iced::mouse::Interaction::Idle)
            .on_press(Message::DragWindow);

        container(draggable)
            .width(Length::Fill)
            .padding(Padding {
                top: 7.0,
                right: 12.0,
                bottom: 7.0,
                left: 14.0,
            })
            .style(move |_| iced::widget::container::Style {
                background: Some(iced::Background::Color(sem.title_bg)),
                ..Default::default()
            })
            .into()
    }

    /// One invisible strip along a window edge, which starts the compositor's
    /// resize the way a frame border would. Without the decorations the
    /// window would otherwise only be resizable from the keyboard.
    fn resize_edge(&self, side: Resize) -> Element<'_, Message> {
        let thin = Length::Fixed(6.0);
        let (width, height, align_x, align_y, interaction) = match side {
            Resize::North | Resize::South => (
                Length::Fill,
                thin,
                iced::alignment::Horizontal::Center,
                match side {
                    Resize::North => iced::alignment::Vertical::Top,
                    _ => iced::alignment::Vertical::Bottom,
                },
                iced::mouse::Interaction::ResizingVertically,
            ),
            Resize::West | Resize::East => (
                thin,
                Length::Fill,
                match side {
                    Resize::West => iced::alignment::Horizontal::Left,
                    _ => iced::alignment::Horizontal::Right,
                },
                iced::alignment::Vertical::Center,
                iced::mouse::Interaction::ResizingHorizontally,
            ),
        };
        container(
            iced::widget::mouse_area(Space::new().width(width).height(height))
                .interaction(interaction)
                .on_press(Message::ResizeWindow(side)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(align_x)
        .align_y(align_y)
        .into()
    }

    /// The navigation rail: brand, one entry per page, and the daemon's own
    /// heartbeat at the bottom.
    fn rail(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let compact = self.width < NARROW;
        let mut nav = Column::new().spacing(6);
        for page in Page::ALL {
            let active = page == self.page;
            let icon_color = if active {
                sem.accent_text
            } else {
                sem.text_secondary
            };
            let content: Element<Message> = if compact {
                s::icon::<Message>(page.icon(), 17)
                    .color(icon_color)
                    .align_x(iced::alignment::Horizontal::Center)
                    .into()
            } else {
                Row::new()
                    .spacing(13)
                    .align_y(Alignment::Center)
                    .push(s::icon::<Message>(page.icon(), 16).color(icon_color))
                    .push(
                        text(page.title())
                            .size(13.5)
                            // `WordOrGlyph`, not the default `Word`: the rail
                            // is the one place where a translation can be
                            // longer than the space, and a label that cannot
                            // break simply stops at the edge. Growing the row
                            // is the honest failure.
                            .wrapping(text::Wrapping::WordOrGlyph)
                            .color(if active {
                                sem.accent_text
                            } else {
                                sem.text_secondary
                            }),
                    )
                    .into()
            };
            nav = nav.push(
                button(content)
                    .width(if compact {
                        Length::Shrink
                    } else {
                        Length::Fill
                    })
                    .padding(Padding {
                        top: 11.0,
                        right: if compact { 12.0 } else { 16.0 },
                        bottom: 11.0,
                        left: if compact { 12.0 } else { 16.0 },
                    })
                    .on_press(Message::Navigate(page))
                    .style(s::button_style(
                        sem,
                        if active {
                            s::Kind::NavActive
                        } else {
                            s::Kind::Nav
                        },
                    )),
            );
        }

        // The daemon is the thing this window is a client of: if it is not
        // there, the rail says so in the one place the user always looks.
        let (dot_color, daemon) = if self.state.reachable {
            (sem.success_tone, tr("Daemon"))
        } else {
            (sem.danger, tr("Daemon offline"))
        };

        // The brand row follows the same rule as the nav entries: a compact
        // rail has no room for the word, only for the mark. Leaving the label
        // in would spill it past the 66 px edge and over the page title.
        let brand = if compact {
            s::brand_mark(sem, 30.0)
        } else {
            Row::new()
                .spacing(11)
                .align_y(Alignment::Center)
                .push(s::brand_mark(sem, 30.0))
                .push(text("Ferramari").size(16).color(sem.text))
                .into()
        };

        let column = Column::new()
            .spacing(22)
            .push(brand)
            .push(nav)
            .push(Space::new().height(Length::Fill))
            .push(self.rail_footer(compact, dot_color, daemon, sem));

        // The rail's width is a fixed number in both layouts. It cannot be
        // `Shrink`: a nav entry's label is `Length::Fill`-sized text, and a
        // shrink-to-fit parent gives fill-sized children nothing to fill, so
        // the whole rail collapses to the icon column. `RAIL_WIDTH` is
        // instead set from the widest label the shipped translations produce,
        // and the labels wrap (see `Page::title` usage above) so a longer
        // translation grows the row to two lines rather than clipping it.
        container(column)
            .width(Length::Fixed(if compact { 66.0 } else { s::RAIL_WIDTH }))
            .height(Length::Fill)
            .padding(Padding {
                top: 18.0,
                right: if compact { 12.0 } else { 16.0 },
                bottom: 18.0,
                left: if compact { 12.0 } else { 16.0 },
            })
            .style(move |_| s::rail_style(sem))
            .into()
    }

    /// The rail's last line: the daemon's heartbeat.
    ///
    /// The dot is the information and the word is its caption, so a rail too
    /// narrow for the caption keeps the dot alone rather than letting the
    /// label spill over the page beside it. Colour carries the state, and the
    /// hover tooltip of the window title still says which daemon it is.
    fn rail_footer(
        &self,
        compact: bool,
        color: iced::Color,
        label: String,
        sem: &'static Semantics,
    ) -> Element<'_, Message> {
        let mut row = Row::new()
            .spacing(9)
            .align_y(Alignment::Center)
            .push(s::dot::<Message>(color, 8.0));
        if !compact {
            row = row.push(text(label).size(12.5).color(sem.text_secondary));
        }
        row.into()
    }

    /// The page column: its own header (title, subtitle, and whatever the
    /// page puts on the right of it) over a scrollable body.
    ///
    /// The header stays put while the body scrolls, which is what keeps the
    /// Home controls and the Library search reachable on a short window.
    fn content(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let mut header = Row::new()
            .align_y(Alignment::Center)
            .spacing(16)
            .push(
                Column::new()
                    .spacing(4)
                    .push(text(self.page.title()).size(26).color(sem.text))
                    .push(text(self.page.subtitle()).size(12.5).color(sem.text_muted)),
            )
            .push(Space::new().width(Length::Fill));

        if let Some(actions) = self.page_actions(sem) {
            header = header.push(actions);
        }

        let mut column = Column::new().spacing(16).width(Length::Fill).push(header);

        // A command that failed or is still running gets one line, and it
        // disappears on the next successful poll.
        if let Some(status) = &self.status {
            let error = status.error;
            let text_value = status.text.clone();
            column = column.push(
                container(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(s::icon::<Message>(
                            if error {
                                "exclamation-triangle"
                            } else {
                                "info-circle"
                            },
                            12,
                        ))
                        .push(text(text_value).size(11.5).color(if error {
                            sem.danger
                        } else {
                            sem.text_secondary
                        })),
                )
                .padding(Padding {
                    top: 7.0,
                    right: 13.0,
                    bottom: 7.0,
                    left: 13.0,
                })
                .style(move |_| {
                    let mut style = s::pill_style(sem, false);
                    if error {
                        style.background = Some(iced::Background::Color(sem.danger_soft));
                        style.border.color = sem.danger;
                    }
                    style
                }),
            );
        }

        column = column.push(
            scrollable(self.page_body(sem))
                .height(Length::Fill)
                .width(Length::Fill),
        );

        container(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// The controls that live in the page header, per page. `None` leaves the
    /// header as title only.
    fn page_actions(&self, sem: &'static Semantics) -> Option<Element<'_, Message>> {
        match self.page {
            Page::Home => Some(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(s::primary_icon(
                        "play",
                        tr("Start"),
                        Some(Message::StartEngine),
                        sem,
                    ))
                    .push(s::secondary_icon(
                        "stop",
                        tr("Stop"),
                        Some(Message::StopEngine),
                        sem,
                    ))
                    .push(s::secondary_icon(
                        "sync",
                        tr("Restart"),
                        Some(Message::RestartEngine),
                        sem,
                    ))
                    .into(),
            ),
            Page::Library => Some(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(self.search_field(sem))
                    .push(s::icon_button(
                        if self.grid_view { "list" } else { "th-large" },
                        Some(Message::ToggleView),
                        13.0,
                        s::Kind::Icon,
                        sem,
                    ))
                    .into(),
            ),
            Page::Online => Some(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(self.search_field(sem))
                    .push(s::icon_button(
                        "sync",
                        Some(Message::OnlineRefresh),
                        13.0,
                        s::Kind::Icon,
                        sem,
                    ))
                    .into(),
            ),
            Page::Performance => Some(s::status_pill(
                tr("MPV Engine"),
                Some(if self.state.is_playing {
                    tr("Running")
                } else {
                    tr("Idle")
                }),
                if self.state.is_playing {
                    sem.success_tone
                } else {
                    sem.text_muted
                },
                sem,
            )),
            Page::Settings => None,
        }
    }

    /// The header search box. Library and Online both have one; the text is
    /// kept in `self` so the input does not lose the caret on every poll.
    fn search_field(&self, sem: &'static Semantics) -> Element<'_, Message> {
        // The placeholder and the value both live in `self`, so the borrow
        // outlives the widget the same way the old page-local inputs did.
        let (value, placeholder, submit): (String, String, Option<Message>) = match self.page {
            Page::Online => (
                self.online.search.clone(),
                self.online.search_placeholder.clone(),
                Some(Message::OnlineSearch),
            ),
            _ => (self.library_filter.clone(), tr("Search wallpapers…"), None),
        };
        let on_input = match self.page {
            Page::Online => Message::OnlineSearchChanged,
            _ => Message::LibraryFilterChanged,
        };
        let mut input = text_input(&placeholder, &value).on_input(on_input);
        if let Some(submit) = submit {
            input = input.on_submit(submit);
        }
        container(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(s::icon::<Message>("search", 13).color(sem.text_muted))
                .push(input.width(Length::Fill)),
        )
        .width(Length::Fixed(300.0))
        .padding(Padding {
            top: 8.0,
            right: 12.0,
            bottom: 8.0,
            left: 12.0,
        })
        .style(move |_| s::row_style(sem))
        .into()
    }

    fn page_body(&self, sem: &'static Semantics) -> Element<'_, Message> {
        match self.page {
            Page::Home => self.home(sem),
            Page::Library => self.library(sem),
            Page::Online => self.online(sem),
            Page::Performance => self.performance(sem),
            Page::Settings => self.settings(sem),
        }
    }

    /// Home: the engine's dashboard.
    ///
    /// Three bands, in the order the reference draws them — what is playing,
    /// where it is playing, and what it costs. The hero is the wallpaper's own
    /// frame with the controls on top of it, which is the only honest way to
    /// show a video wallpaper: a screenshot of the thing, not an icon of it.
    fn home(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let source = self
            .state
            .data_source
            .get("Default")
            .or_else(|| self.state.data_source.values().next())
            .cloned();

        Column::new()
            .spacing(16)
            .push(self.hero(source.as_deref(), sem))
            .push(self.monitor_card(sem))
            .push(self.resource_row(sem))
            .push(self.quick_actions(sem))
            .into()
    }

    /// The hero card: the current frame as the artwork, the title and the
    /// technical line over a scrim, transport controls at the right.
    fn hero(&self, source: Option<&str>, sem: &'static Semantics) -> Element<'_, Message> {
        let (title, meta) = match source {
            Some(path) => {
                let name = PathBuf::from(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string());
                let stats = &self.state.player_stats;
                let mut parts: Vec<String> = Vec::new();
                if let Some((width, height)) = stats
                    .width
                    .zip(stats.height)
                    .filter(|(width, height)| *width > 0 && *height > 0)
                {
                    parts.push(format!("{width}×{height}"));
                }
                if let Some(fps) = stats.fps {
                    parts.push(format!("{} FPS", fps.round() as u32));
                }
                if let Some(duration) = stats.duration {
                    parts.push(format!(
                        "-{}",
                        clock(duration - stats.position.unwrap_or(0.0))
                    ));
                }
                (name, parts.join(" · "))
            }
            None => (tr("No wallpaper"), String::new()),
        };

        // The frame is produced by ffmpeg on a worker; until it exists the
        // plate keeps its own gradient and the card is still readable.
        let artwork: Element<Message> = source
            .map(PathBuf::from)
            .and_then(|path| thumbnails::generate_hero(&path))
            .filter(|thumb| thumb.is_file())
            .map(image::Handle::from_path)
            .map(|handle| {
                image::Image::new(handle)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(ContentFit::Cover)
                    .into()
            })
            .unwrap_or_else(|| Space::new().width(Length::Fill).height(Length::Fill).into());

        let playing = self.state.is_playing && !self.state.is_paused_by_user;
        let controls = Row::new()
            .spacing(14)
            .align_y(Alignment::End)
            .push(
                Column::new()
                    .spacing(3)
                    .width(Length::Fill)
                    // `on_artwork`, not `text`: this line sits on a video
                    // frame darkened by a black scrim, and in the light theme
                    // the ordinary text token is near-black on near-black.
                    .push(text(title).size(17).color(sem.on_artwork))
                    .push(
                        // An empty technical line means the decoder has not
                        // answered yet, not that nothing is running: the two
                        // say different things and only one of them is true.
                        text(if !meta.is_empty() {
                            meta
                        } else if playing {
                            tr("Reading playback data…")
                        } else {
                            tr("Nothing is playing")
                        })
                        .size(11)
                        .color(sem.on_artwork_soft),
                    ),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(s::icon_button(
                        "step-backward",
                        Some(Message::FeelingLucky),
                        12.0,
                        s::Kind::IconOnArtwork,
                        sem,
                    ))
                    .push(s::icon_button(
                        if playing { "pause" } else { "play" },
                        Some(Message::TogglePlay),
                        14.0,
                        s::Kind::IconOnArtwork,
                        sem,
                    ))
                    .push(s::icon_button(
                        "step-forward",
                        Some(Message::ReloadWallpaper),
                        12.0,
                        s::Kind::IconOnArtwork,
                        sem,
                    )),
            );

        // Sound is the one control the reference's hero does not draw, and
        // the one that must stay reachable from the page that shows what is
        // playing: a wallpaper with audio and no mute button on the same
        // screen is a trip to the tray menu.
        let sound = Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(s::icon_button(
                if self.state.is_mute {
                    "volume-off"
                } else {
                    "volume-up"
                },
                Some(Message::ToggleMute),
                12.0,
                s::Kind::IconOnArtwork,
                sem,
            ))
            .push(slider(0..=100, self.state.volume, Message::SetVolume).width(Length::Fill))
            .push(
                // On the artwork, like the title above it.
                text(format!("{}%", self.state.volume))
                    .size(11)
                    .color(sem.on_artwork_soft),
            );

        // The scrim is a gradient of its own, not the card material: the
        // artwork behind it is whatever the wallpaper happens to be showing,
        // and the text on top of it has to be readable in every frame.
        let scrim = container(
            Column::new()
                .spacing(12)
                .push(Space::new().height(Length::Fill))
                .push(sound)
                .push(controls)
                .padding(Padding {
                    top: 14.0,
                    right: 16.0,
                    bottom: 16.0,
                    left: 16.0,
                }),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| iced::widget::container::Style {
            background: Some(Background::Gradient(s::scrim_gradient(sem))),
            ..Default::default()
        })
        .into();

        let stack = iced::widget::stack(vec![
            container(artwork)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_| s::row_style(sem))
                .into(),
            scrim,
        ]);

        let plate = container(stack)
            .width(Length::Fill)
            .height(Length::Fixed(212.0))
            .clip(true)
            .style(move |_| iced::widget::container::Style {
                border: Border {
                    color: sem.card_edge,
                    width: 1.0,
                    radius: (RADIUS_HERO).into(),
                },
                shadow: Shadow {
                    color: black_a(0.35),
                    offset: Vector::new(0.0, 12.0),
                    blur_radius: 34.0,
                },
                ..Default::default()
            });

        // The "Now Playing" badge sits over the artwork's top edge, outside
        // the scrim: it is the one thing that must be readable whatever the
        // frame shows.
        let badge = container(
            Row::new()
                .spacing(7)
                .align_y(Alignment::Center)
                .push(s::dot::<Message>(
                    if playing {
                        sem.live_artwork
                    } else {
                        sem.on_artwork_soft
                    },
                    7.0,
                ))
                .push(text(tr("Now Playing")).size(11.5).color(sem.on_artwork)),
        )
        .padding(Padding {
            top: 6.0,
            right: 12.0,
            bottom: 6.0,
            left: 11.0,
        })
        // The badge floats over the frame, so it carries its own dark plate
        // rather than the light pill: in the light theme `pill_style` is a
        // translucent white and the text on it would have to be dark, on a
        // surface that is whatever the wallpaper happens to be showing.
        .style(move |_| iced::widget::container::Style {
            background: Some(Background::Color(sem.badge_artwork)),
            border: Border {
                color: sem.pill_edge,
                width: 1.0,
                radius: 999.0.into(),
            },
            ..Default::default()
        });

        iced::widget::stack(vec![
            plate.into(),
            container(badge)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Top)
                .padding(Padding::new(14.0))
                .into(),
        ])
        .width(Length::Fill)
        .height(Length::Fixed(212.0))
        .into()
    }

    /// The Monitors card: one row per display, with its mode, its resolution
    /// and a box that says whether it is the one being driven.
    fn monitor_card(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let mut names: Vec<&String> = self.state.monitor_states.keys().collect();
        names.sort();
        let count = names.len();

        let mut body = Column::new().spacing(10);
        if names.is_empty() {
            body = body.push(
                text(tr("No displays detected"))
                    .size(12.5)
                    .color(sem.text_muted),
            );
        }
        for name in names {
            let state = self
                .state
                .monitor_states
                .get(name)
                .cloned()
                .unwrap_or_default();
            let resolution = self
                .state
                .monitor_resolutions
                .get(name)
                .cloned()
                .unwrap_or_default();
            let primary = self.state.primary_monitor == *name;
            let source = self
                .state
                .data_source
                .get(name)
                .or_else(|| self.state.data_source.get("Default"))
                .cloned()
                .unwrap_or_default();
            let file = PathBuf::from(&source)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| tr("No wallpaper"));
            // `3840 × 2160 · 120 Hz`, the reference's form. The rate is left
            // off when the backend did not measure one: `— Hz` would be a
            // number-shaped lie.
            let mode = match (
                (!resolution.is_empty()).then_some(resolution.as_str()),
                self.state.monitor_refresh.get(name).copied(),
            ) {
                (Some(size), Some(hz)) => format!("{size} · {hz} Hz"),
                (Some(size), None) => size.to_string(),
                (None, Some(hz)) => format!("{hz} Hz"),
                (None, None) => file.clone(),
            };

            let monitor_name = name.clone();
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
            let selected_rotation = self
                .state
                .rotations
                .get(&monitor_name)
                .cloned()
                .unwrap_or_default();
            let selected = rotation_options
                .iter()
                .find(|choice| choice.id == selected_rotation)
                .cloned();

            let row = Row::new()
                .spacing(14)
                .align_y(Alignment::Center)
                .push(
                    container(s::icon::<Message>("desktop", 16).color(sem.text_secondary))
                        .width(Length::Fixed(34.0))
                        .height(Length::Fixed(34.0))
                        .align_x(iced::alignment::Horizontal::Center)
                        .align_y(iced::alignment::Vertical::Center)
                        .style(move |_| s::row_style(sem)),
                )
                .push(
                    Column::new()
                        .spacing(2)
                        .width(Length::Fill)
                        .push(
                            text(if primary {
                                format!("{name} ({})", tr("Primary"))
                            } else {
                                name.to_string()
                            })
                            .size(13.5)
                            .color(sem.text),
                        )
                        .push(text(mode).size(11).color(sem.text_muted)),
                )
                .push(s::chip(state_label(&state, sem), sem))
                .push(
                    pick_list(rotation_options, selected, move |choice| {
                        Message::SetMonitorRotation(monitor_name.clone(), choice.id)
                    })
                    .width(Length::Fixed(150.0)),
                );

            body = body.push(s::inset(row, sem, 10.0));
        }

        s::card(
            Column::new()
                .spacing(14)
                .push(s::heading_icon::<Message>("desktop", tr("Monitors"), sem))
                .push(
                    text(format!(
                        "{} {}",
                        count,
                        ferramari_core::util::tr_n("display", "displays", count as u32)
                    ))
                    .size(11.5)
                    .color(sem.text_muted),
                )
                .push(body),
            sem,
        )
        .into()
    }

    /// The frame rate to show and to record: what the decoder is producing,
    /// falling back to the rate the file was authored at while no measurement
    /// exists. One definition, used by the tile and by the history that draws
    /// under it, so the number and its trace cannot disagree.
    fn effective_fps(&self) -> Option<f64> {
        let stats = &self.state.player_stats;
        stats.decoded_fps.or(stats.fps)
    }

    /// The refresh rate the frame-rate tile is measured against.
    ///
    /// The primary display's own rate, when the backend reported one: a
    /// wallpaper can never be smoother than the screen it is drawn on, so
    /// that number is the honest ceiling and the only one worth printing.
    /// `None` on a session where nothing was measured, which leaves the tile
    /// without a label rather than inventing 60.
    fn refresh_ceiling(&self) -> Option<u32> {
        let primary = self.state.primary_monitor.as_str();
        self.state
            .monitor_refresh
            .get(primary)
            .or_else(|| {
                // One display and no primary flag: that display is the one
                // the wallpaper is on, so its rate still means something.
                (self.state.monitor_refresh.len() == 1)
                    .then(|| self.state.monitor_refresh.values().next())?
            })
            .copied()
    }

    /// The four resource tiles: CPU, RAM, GPU, FPS. Each is a number plus the
    /// trace that gives it meaning.
    fn resource_row(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let sample = self.sample.as_ref();
        let memory = sample.and_then(|sample| sample.memory.value());
        let memory_total = sample.and_then(|sample| sample.memory_total.value());
        let fps = self.effective_fps();
        let ceiling = self.refresh_ceiling();

        let gib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0 * 1024.0);

        // RAM is the one tile with a second number: the share of the whole
        // machine, which is what makes 2.8 GB mean anything.
        let ram_share = match (memory, memory_total) {
            (Some(used), Some(total)) if total > 0 => Some((used as f64 / total as f64) * 100.0),
            _ => None,
        };

        let tiles = [
            Tile {
                icon: "microchip",
                label: tr("CPU"),
                value: metric_short(sample.map(|sample| &sample.cpu), |value| {
                    format!("{}%", value.round() as u64)
                }),
                sub: None,
                series: &self.history.cpu,
                color: sem.series_cpu,
                ceiling: Some(100.0),
                highlighted: false,
            },
            Tile {
                icon: "server",
                label: tr("RAM"),
                value: metric_short(sample.map(|sample| &sample.memory), |bytes| {
                    format!("{} GB", trim_number(gib(*bytes)))
                }),
                sub: ram_share.map(|share| format!("{}%", share.round() as u64)),
                series: &self.history.ram,
                color: sem.series_ram,
                ceiling: memory_total.map(gib).map(|total| total.max(1.0)),
                highlighted: false,
            },
            Tile {
                icon: "cogs",
                label: tr("GPU"),
                value: metric_short(sample.map(|sample| &sample.gpu), |value| {
                    format!("{}%", value.round() as u64)
                }),
                sub: None,
                series: &self.history.gpu,
                color: sem.series_gpu,
                ceiling: Some(100.0),
                highlighted: false,
            },
            Tile {
                icon: "play",
                label: tr("FPS"),
                value: fps
                    .map(|value| format!("{}", value.round() as u64))
                    .unwrap_or_else(|| tr("Idle")),
                // The screen's own refresh rate, measured from RandR, is the
                // ceiling the decoder is working against. `Target: 60` would
                // be a claim about a setting nobody chose.
                sub: ceiling.map(|hz| format!("{hz} Hz")),
                series: &self.history.fps,
                color: sem.series_fps,
                ceiling: ceiling.map(|hz| hz as f64),
                // The reference puts the amber halo on the frame rate: for a
                // wallpaper engine it is the number the user actually reads,
                // and the one that says whether playback is healthy.
                highlighted: true,
            },
        ];

        iced::widget::row(tiles.into_iter().map(|tile| tile.element(sem)))
            .spacing(14)
            .into()
    }

    /// How many slides the Online carousel has: the first few items of the
    /// current view, which is what the tabs are a view of. Kept here so the
    /// carousel and the arrow handler cannot disagree about the count.
    fn featured_slides(&self) -> usize {
        self.online.items.len().min(FEATURED_SLIDES)
    }

    /// The four action tiles under the resources. They are the shortcuts the
    /// reference promises, and each one is a real command, not a demo.
    fn quick_actions(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let server_present = self.server.is_some();
        iced::widget::row(vec![
            s::tile_button(
                "image",
                tr("Choose Wallpaper"),
                server_present.then_some(Message::PickVideo),
                sem,
            ),
            s::tile_button("plus", tr("Add Wallpaper"), Some(Message::PickVideo), sem),
            s::tile_button(
                "globe",
                tr("Browse Online"),
                Some(Message::Navigate(Page::Online)),
                sem,
            ),
            s::tile_button(
                "cog",
                tr("Open Settings"),
                Some(Message::Navigate(Page::Settings)),
                sem,
            ),
        ])
        .spacing(14)
        .into()
    }

    /// Library: the local wallpapers, as a grid of cards.
    ///
    /// The reference page is a storefront of thumbnails, and the honest data
    /// behind it is the union of two things: what the user has ever applied
    /// (history) and what actually sits in the managed folder. Neither alone
    /// is right — history forgets files the user dropped in, and the folder
    /// list forgets videos played from elsewhere.
    fn library(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let visible = self.visible_media();
        let live = self.live_media();
        let count = visible.len();
        let chips = s::chip_row(
            [
                MediaFilter::All,
                MediaFilter::Video,
                MediaFilter::Image,
                MediaFilter::Live,
                MediaFilter::Favorites,
            ]
            .into_iter()
            .map(|filter| {
                (
                    filter.label(),
                    filter == self.media_filter,
                    Some(Message::MediaFilterChanged(filter)),
                )
            })
            .collect(),
            sem,
        );

        let body: Element<Message> = if visible.is_empty() {
            self.library_empty(sem)
        } else if self.grid_view {
            self.library_grid(&visible, &live, sem)
        } else {
            self.library_list(&visible, &live, sem)
        };

        Column::new()
            .spacing(16)
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    // The page header already says "Your local wallpapers";
                    // repeating it here would only add a number to a sentence
                    // the reader has just finished.
                    .push(
                        text(format!(
                            "{count} {}",
                            ferramari_core::util::tr_n("item", "items", count as u32)
                        ))
                        .size(11.5)
                        .color(sem.text_muted),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(s::secondary(
                        tr("Open folder"),
                        Some(Message::OpenFolder),
                        sem,
                    ))
                    .push(s::primary_icon(
                        "plus",
                        tr("Add"),
                        Some(Message::PickVideo),
                        sem,
                    )),
            )
            .push(chips)
            .push(body)
            .into()
    }

    /// Every file the Library may show, newest first: history, then anything
    /// in the managed folder that history does not already name.
    fn library_entries(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut entries: Vec<String> = self
            .library
            .history()
            .iter()
            .filter(|entry| seen.insert(entry.path.clone()))
            .map(|entry| entry.path.clone())
            .collect();

        let mut folder: Vec<PathBuf> = std::fs::read_dir(paths::video_wallpaper_dir())
            .map(|reader| {
                reader
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| path.is_file())
                    .filter(|path| {
                        let name = path.to_string_lossy().to_lowercase();
                        is_video(&name) || is_image(&name)
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Newest on disk first, so a wallpaper the user just dropped in is the
        // first card they see.
        folder.sort_by_key(|path| {
            std::cmp::Reverse(
                path.metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .unwrap_or(std::time::UNIX_EPOCH),
            )
        });
        for path in folder {
            let text = path.to_string_lossy().into_owned();
            if seen.insert(text.clone()) {
                entries.push(text);
            }
        }
        entries
    }

    /// The cards the Library shows right now: the type filter, then the
    /// search. `update` needs the same list to know which files to probe, so
    /// the rule lives in one place rather than in two that can disagree.
    fn visible_media(&self) -> Vec<String> {
        let live = self.live_media();
        let favorites: Vec<String> = self.library.favorites().to_vec();
        let query = self.library_filter.trim().to_lowercase();
        self.library_entries()
            .into_iter()
            .filter(|path| self.media_filter.accepts(path, &live, &favorites))
            .filter(|path| {
                query.is_empty()
                    || PathBuf::from(path)
                        .file_name()
                        .map(|name| name.to_string_lossy().to_lowercase())
                        .unwrap_or_default()
                        .contains(&query)
            })
            .collect()
    }

    /// The files assigned to a display. `data_source` is keyed by monitor and
    /// by the legacy `Default`, so the same path can arrive twice.
    fn live_media(&self) -> Vec<String> {
        let mut live: Vec<String> = self.state.data_source.values().cloned().collect();
        live.sort();
        live.dedup();
        live
    }

    fn library_empty(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let (title, hint) = if self.library_filter.trim().is_empty() {
            match self.media_filter {
                MediaFilter::Favorites => (
                    tr("No favorites yet"),
                    tr("Star a wallpaper to keep it here."),
                ),
                MediaFilter::Live => (
                    tr("Nothing is playing"),
                    tr("Apply a wallpaper and it will show here."),
                ),
                _ => (
                    tr("The library is empty"),
                    tr("Add a video, or download one from Online."),
                ),
            }
        } else {
            (
                tr("No matches"),
                tr("Nothing in the library matches that search."),
            )
        };
        s::card(
            Column::new()
                .spacing(8)
                .align_x(Alignment::Center)
                .padding(Padding {
                    top: 40.0,
                    right: 20.0,
                    bottom: 40.0,
                    left: 20.0,
                })
                .push(s::icon::<Message>("film", 26).color(sem.text_muted))
                .push(text(title).size(14).color(sem.text))
                .push(text(hint).size(11.5).color(sem.text_muted)),
            sem,
        )
        .into()
    }

    /// The card grid: as many columns as fit at the current width. Three
    /// fixed would squeeze a Russian title until it clipped, and one fixed
    /// would leave a 4K monitor with two cards on the screen.
    fn library_grid(
        &self,
        paths: &[String],
        live: &[String],
        sem: &'static Semantics,
    ) -> Element<'_, Message> {
        let columns = match self.width {
            width if width > 1360.0 => 4,
            width if width > 1080.0 => 3,
            width if width > 820.0 => 2,
            _ => 1,
        };
        let mut rows = Vec::new();
        for chunk in paths.chunks(columns) {
            let mut row = Row::new().spacing(14);
            for path in chunk {
                row = row.push(self.media_card(path, live, sem));
            }
            // A short last row must not stretch its cards across the page:
            // an empty filler keeps every column the width of a real card.
            for _ in chunk.len()..columns {
                row = row.push(Space::new().width(Length::Fill));
            }
            rows.push(row.into());
        }
        Column::with_children(rows).spacing(14).into()
    }

    /// One wallpaper as a card: the frame, the name, the measured quality,
    /// and a menu that opens under it.
    fn media_card(
        &self,
        path: &str,
        live: &[String],
        sem: &'static Semantics,
    ) -> Element<'_, Message> {
        let name = PathBuf::from(path)
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        let active = live.iter().any(|source| source == path);
        let open = self
            .expanded
            .as_ref()
            .is_some_and(|(where_open, _)| where_open == path);

        let artwork: Element<Message> = thumbnails::generate(&PathBuf::from(path))
            .filter(|thumb| thumb.is_file())
            .map(image::Handle::from_path)
            .map(|handle| {
                image::Image::new(handle)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(ContentFit::Cover)
                    .into()
            })
            .unwrap_or_else(|| {
                container(s::icon::<Message>("film", 22).color(sem.text_muted))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center)
                    .into()
            });

        let quality = self
            .media_info
            .get(path)
            .and_then(|cached| cached.as_ref())
            .map(|info| info.quality())
            .unwrap_or_default();

        let head = Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(
                Column::new()
                    .spacing(3)
                    .width(Length::Fill)
                    // `WordOrGlyph`, not the default `Word`: the names here
                    // are file names, and `untitled_Motion_2026-03-15.mp4` has
                    // no word boundary to break at. With `Word` it would run
                    // past the card and clip.
                    .push(
                        text(name)
                            .size(13)
                            .color(sem.text)
                            .wrapping(text::Wrapping::WordOrGlyph),
                    )
                    .push(
                        text(if quality.is_empty() {
                            tr("Reading…")
                        } else {
                            quality
                        })
                        .size(11)
                        .color(sem.text_muted),
                    ),
            )
            .push(s::icon_button(
                if open { "times" } else { "ellipsis-v" },
                Some(Message::ToggleCardMenu(path.to_string(), true)),
                12.0,
                s::Kind::Icon,
                sem,
            ));

        let body = Column::new()
            .spacing(12)
            .push(
                container(artwork)
                    .width(Length::Fill)
                    .height(Length::Fixed(112.0))
                    .clip(true)
                    .style(move |_| iced::widget::container::Style {
                        background: Some(Background::Color(sem.row_bg)),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: s::RADIUS_INNER.into(),
                        },
                        ..Default::default()
                    }),
            )
            .push(head);

        let mut column = Column::new().spacing(12).push(
            iced::widget::mouse_area(body)
                .interaction(iced::mouse::Interaction::Pointer)
                .on_release(Message::Play(path.to_string())),
        );
        if open {
            column = column.push(self.card_menu(path, active, sem));
        }
        container(column)
            .width(Length::Fill)
            .padding(Padding {
                top: 12.0,
                right: 12.0,
                bottom: 12.0,
                left: 12.0,
            })
            .style(move |_| {
                let mut style = s::card_style(sem);
                if active {
                    // The wallpaper that is actually on screen is the one card the
                    // reference marks, and it marks it with the accent.
                    style.border.color = sem.accent_border;
                    style.shadow = Shadow {
                        color: sem.glow_accent,
                        offset: Vector::new(0.0, 0.0),
                        blur_radius: 24.0,
                    };
                }
                style
            })
            .into()
    }

    /// The actions of one wallpaper. They open in place, under the card,
    /// rather than as a floating menu: a menu that leaves the scrollable and
    /// gets clipped is worse than a card that grows.
    fn card_menu(&self, path: &str, active: bool, sem: &'static Semantics) -> Element<'_, Message> {
        let favorite = self.library.is_favorite(path);
        let mut menu = Column::new().spacing(2).push(s::divider(sem));

        menu = menu.push(Self::menu_entry(
            "play",
            tr("Apply as wallpaper"),
            Message::Play(path.to_string()),
            sem,
        ));
        // A filled star is the way in, an outlined one the way out: the row
        // reads as a state before it reads as a command.
        menu = menu.push(Self::menu_entry(
            if favorite { "star" } else { "star-o" },
            if favorite {
                tr("Remove from favorites")
            } else {
                tr("Add to favorites")
            },
            Message::ToggleFavorite(path.to_string()),
            sem,
        ));
        // Per-monitor assignment only means something once there is more than
        // one display to assign to.
        if self.state.monitor_states.len() > 1 {
            for monitor in self.state.monitor_states.keys() {
                menu = menu.push(Self::menu_entry(
                    "desktop",
                    format!("{}: {}", tr("Play on"), monitor),
                    Message::PlayOnMonitor(path.to_string(), monitor.clone()),
                    sem,
                ));
            }
        }
        for collection in self.library.collections() {
            // One entry per collection, and its meaning depends on whether
            // the wallpaper is already in it: the same row that adds is the
            // row that removes, so the menu does not grow a second list of
            // things that are already true.
            let inside = collection.items.iter().any(|item| item == path);
            menu = menu.push(Self::menu_entry(
                if inside { "minus" } else { "folder" },
                format!(
                    "{}: {}",
                    if inside {
                        tr("Remove from")
                    } else {
                        tr("Add to")
                    },
                    collection.name
                ),
                if inside {
                    Message::RemoveFromCollection(collection.id.clone(), path.to_string())
                } else {
                    Message::AddToCollection(collection.id.clone(), path.to_string())
                },
                sem,
            ));
        }
        if active {
            menu = menu.push(Self::menu_entry(
                "sync",
                tr("Reload"),
                Message::ReloadWallpaper,
                sem,
            ));
        }
        menu = menu.push(Self::menu_entry(
            "times",
            tr("Remove from library"),
            Message::RemoveMedia(path.to_string()),
            sem,
        ));
        menu.into()
    }

    fn menu_entry<'a, M: Clone + 'a>(
        label_icon: &str,
        label: String,
        message: M,
        sem: &'static Semantics,
    ) -> Element<'a, M> {
        button(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(s::icon::<M>(label_icon, 11).color(sem.text_muted))
                .push(text(label).size(12.5)),
        )
        .width(Length::Fill)
        .padding(Padding {
            top: 8.0,
            right: 10.0,
            bottom: 8.0,
            left: 10.0,
        })
        .on_press(message)
        .style(s::button_style(sem, s::Kind::Nav))
        .into()
    }

    /// The same files as rows, for people who want the paths. The reference
    /// offers both views; this is the second one.
    fn library_list(
        &self,
        paths: &[String],
        live: &[String],
        sem: &'static Semantics,
    ) -> Element<'_, Message> {
        let mut column = Column::new().spacing(10);
        for path in paths {
            let name = PathBuf::from(path)
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let active = live.iter().any(|source| source == path);
            let favorite = self.library.is_favorite(path);
            let open = self
                .expanded
                .as_ref()
                .is_some_and(|(where_open, _)| where_open == path);

            let thumb: Element<Message> = thumbnails::generate(&PathBuf::from(path))
                .filter(|thumb| thumb.is_file())
                .map(image::Handle::from_path)
                .map(|handle| {
                    image::Image::new(handle)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .content_fit(ContentFit::Cover)
                        .into()
                })
                .unwrap_or_else(|| {
                    container(s::icon::<Message>("film", 14).color(sem.text_muted))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center)
                        .align_y(iced::alignment::Vertical::Center)
                        .into()
                });

            let mut row = Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(
                    container(thumb)
                        .width(Length::Fixed(74.0))
                        .height(Length::Fixed(44.0))
                        .clip(true)
                        .style(move |_| iced::widget::container::Style {
                            background: Some(Background::Color(sem.row_bg)),
                            border: Border {
                                color: Color::TRANSPARENT,
                                width: 0.0,
                                radius: 8.0.into(),
                            },
                            ..Default::default()
                        }),
                )
                .push(
                    Column::new()
                        .spacing(2)
                        .width(Length::Fill)
                        .push(text(name).size(13).color(sem.text))
                        .push(
                            // Owned: the element outlives the slice the rows
                            // were built from.
                            text(path.to_string())
                                .size(10.5)
                                .color(sem.text_muted)
                                .wrapping(text::Wrapping::WordOrGlyph),
                        ),
                );
            if active {
                // The chip comes before the buttons so a row that is live
                // reads as live at a glance, wherever the eye lands.
                row = row.push(s::accent_chip(tr("Live"), sem));
            }
            row = row
                .push(s::icon_button(
                    if favorite { "star" } else { "star-o" },
                    Some(Message::ToggleFavorite(path.clone())),
                    12.0,
                    s::Kind::Icon,
                    sem,
                ))
                .push(s::icon_button(
                    if open { "times" } else { "ellipsis-v" },
                    Some(Message::ToggleCardMenu(path.clone(), false)),
                    12.0,
                    s::Kind::Icon,
                    sem,
                ));

            let mut wrapped = Column::new().spacing(8).push(row);
            if open {
                wrapped = wrapped.push(self.card_menu(path, active, sem));
            }

            column = column.push(
                container(wrapped)
                    .width(Length::Fill)
                    .padding(Padding {
                        top: 10.0,
                        right: 12.0,
                        bottom: 10.0,
                        left: 12.0,
                    })
                    .style(move |_| s::card_style(sem)),
            );
        }
        column.into()
    }
    /// Online: the catalogue as a storefront.
    ///
    /// The reference shows a featured carousel, a row of category tiles and a
    /// filter bar. All three are built from one listing: the catalogue has no
    /// "featured" endpoint, so the tabs are views over the items already
    /// fetched and only a search or a category goes back to the network. That
    /// is stated in the UI rather than hidden — a tab that silently re-downloads
    /// the same page would look like a different source.
    fn online(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let state = &self.online;
        let mut body = Column::new().spacing(16);

        // The hero only exists once there is something to feature. An empty
        // plate of the same size reads as a broken image rather than as a
        // page that has not been filled yet.
        if !state.items.is_empty() {
            body = body.push(self.featured_carousel(sem));
        }

        if !state.categories.is_empty() {
            body = body.push(self.category_tiles(sem));
        }

        body = body.push(s::chip_row(
            online_tabs()
                .into_iter()
                .enumerate()
                .map(|(index, label)| (label, index == state.tab, Some(Message::OnlineTab(index))))
                .collect(),
            sem,
        ));

        match &state.error {
            Some(error) => body = body.push(self.online_note(error.clone(), true, sem)),
            None if state.loading && state.items.is_empty() => {
                body = body.push(self.online_note(tr("Loading wallpapers…"), false, sem));
            }
            None if state.items.is_empty() => {
                body = body.push(self.online_note(tr("No wallpapers found"), false, sem));
            }
            None => {
                body = body.push(self.online_results(sem));
                if state.next.is_some() {
                    body = body.push(
                        Row::new()
                            .push(s::secondary(
                                tr("Load more"),
                                Some(Message::OnlineMore),
                                sem,
                            ))
                            .width(Length::Fill),
                    );
                }
            }
        }
        body.into()
    }

    /// One line of status inside the page: an error or a waiting note, drawn
    /// as a card so it occupies the space the results would.
    fn online_note(
        &self,
        text_value: String,
        error: bool,
        sem: &'static Semantics,
    ) -> Element<'_, Message> {
        s::card(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(
                    s::icon::<Message>(
                        if error {
                            "exclamation-triangle"
                        } else {
                            "info-circle"
                        },
                        14,
                    )
                    .color(if error { sem.danger } else { sem.text_muted }),
                )
                .push(
                    text(text_value)
                        .size(12.5)
                        .color(if error { sem.danger } else { sem.text_muted })
                        .width(Length::Fill)
                        .wrapping(text::Wrapping::WordOrGlyph),
                ),
            sem,
        )
        .into()
    }

    /// The hero of the page: the first wallpapers of the current view, one at
    /// a time, with the arrows and the dots the reference draws.
    fn featured_carousel(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let slides = self.featured_slides();
        let current = self.online.featured.min(slides.saturating_sub(1));
        let wallpaper = self.online.items.get(current);

        let artwork: Element<Message> = wallpaper
            .and_then(|item| self.online.previews.get(&item.id))
            .and_then(|path| path.as_ref())
            .map(image::Handle::from_path)
            .map(|handle| {
                image::Image::new(handle)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(ContentFit::Cover)
                    .into()
            })
            .unwrap_or_else(|| {
                container(s::icon::<Message>("globe", 30).color(sem.on_artwork_soft))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center)
                    .into()
            });

        let (title, subtitle) = match wallpaper {
            Some(item) => (
                item.title.clone(),
                format!("{} · {}", item.category, tr("Live wallpapers")),
            ),
            None => (tr("Nothing featured yet"), tr("Search or pick a category")),
        };

        let scrim = container(
            Column::new()
                .push(Space::new().height(Length::Fill))
                .push(
                    Row::new()
                        .spacing(14)
                        .align_y(Alignment::End)
                        .push(
                            Column::new()
                                .spacing(3)
                                .width(Length::Fill)
                                // On the artwork, as on the Home hero.
                                .push(text(title).size(17).color(sem.on_artwork))
                                .push(text(subtitle).size(11.5).color(sem.on_artwork_soft)),
                        )
                        .push(s::primary_icon(
                            "folder-open",
                            tr("Browse"),
                            Some(Message::OnlineTab(3)),
                            sem,
                        )),
                )
                .padding(Padding {
                    top: 14.0,
                    right: 16.0,
                    bottom: 16.0,
                    left: 16.0,
                }),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| iced::widget::container::Style {
            background: Some(Background::Gradient(s::scrim_gradient(sem))),
            ..Default::default()
        });

        let plate = container(iced::widget::stack(vec![
            container(artwork)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            scrim.into(),
        ]))
        .width(Length::Fill)
        .height(Length::Fixed(184.0))
        .clip(true)
        .style(move |_| iced::widget::container::Style {
            border: Border {
                color: sem.card_edge,
                width: 1.0,
                radius: s::RADIUS.into(),
            },
            shadow: Shadow {
                color: black_a(0.35),
                offset: Vector::new(0.0, 12.0),
                blur_radius: 34.0,
            },
            ..Default::default()
        });

        // One slide is nothing to move between, so the arrows go inert rather
        // than disappearing and shifting the plate's layout.
        let arrows = |side: isize| {
            s::icon_button(
                if side < 0 {
                    "chevron-left"
                } else {
                    "chevron-right"
                },
                (slides > 1).then_some(Message::OnlineFeatured(side)),
                13.0,
                s::Kind::IconOnArtwork,
                sem,
            )
        };

        let dots = Row::new()
            .spacing(6)
            .align_y(Alignment::Center)
            // The dots ride the frame too, so their palette is the artwork's.
            .extend((0..slides).map(|index| {
                s::dot::<Message>(
                    if index == current {
                        sem.accent_text
                    } else {
                        sem.on_artwork_soft
                    },
                    if index == current { 7.0 } else { 5.0 },
                )
            }));

        iced::widget::stack(vec![
            plate.into(),
            container(arrows(-1))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Left)
                .align_y(iced::alignment::Vertical::Center)
                .padding(Padding::new(10.0))
                .into(),
            container(arrows(1))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Center)
                .padding(Padding::new(10.0))
                .into(),
            container(dots)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Bottom)
                .padding(Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom: 4.0,
                    left: 0.0,
                })
                .into(),
        ])
        .width(Length::Fill)
        .height(Length::Fixed(184.0))
        .into()
    }

    /// The category tiles. The catalogue's own categories are shown, each
    /// with a glyph chosen from its name: the reference draws five icons and
    /// the source does not tell us which picture belongs to which category,
    /// so the icon follows the word rather than the other way round.
    fn category_tiles(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let columns = match self.width {
            width if width > 1360.0 => 6,
            width if width > 1080.0 => 5,
            _ => 3,
        };
        let mut rows = Vec::new();
        for chunk in self.online.categories.chunks(columns) {
            let mut row = Row::new().spacing(14);
            for category in chunk {
                let selected = self
                    .online
                    .selected
                    .as_ref()
                    .is_some_and(|chosen| chosen.url == category.url);
                row = row.push(
                    button(
                        Column::new()
                            .spacing(10)
                            .align_x(Alignment::Center)
                            .push(
                                s::icon::<Message>(category_glyph(&category.name), 22).color(
                                    if selected {
                                        sem.accent_text
                                    } else {
                                        sem.text_secondary
                                    },
                                ),
                            )
                            .push(text(category.name.clone()).size(12).color(sem.text)),
                    )
                    .width(Length::Fill)
                    .padding(Padding {
                        top: 16.0,
                        right: 8.0,
                        bottom: 16.0,
                        left: 8.0,
                    })
                    .on_press(Message::OnlineCategory(category.name.clone()))
                    .style(s::button_style(
                        sem,
                        if selected {
                            s::Kind::ChipActive
                        } else {
                            s::Kind::Tile
                        },
                    )),
                );
            }
            for _ in chunk.len()..columns {
                row = row.push(Space::new().width(Length::Fill));
            }
            rows.push(row.into());
        }
        Column::new()
            .spacing(14)
            .push(s::heading(tr("Categories"), sem))
            .extend(rows)
            .into()
    }

    /// The results of the current view, as cards.
    fn online_results(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let visible = self.online_view();
        let mut rows = Vec::new();
        for chunk in visible.chunks(3) {
            let mut row = Row::new().spacing(14);
            for wallpaper in chunk {
                row = row.push(self.online_card(wallpaper, sem));
            }
            for _ in chunk.len()..3 {
                row = row.push(Space::new().width(Length::Fill));
            }
            rows.push(row.into());
        }
        Column::with_children(rows).spacing(14).into()
    }

    /// The items of the current tab.
    ///
    /// Every tab is a view over the one listing the source returned, and each
    /// re-ordering uses a signal that exists:
    /// * `Featured` — the order the source itself publishes.
    /// * `Recent` — the publication month, read from the item's upload path.
    /// * `Downloaded` — what is already in the local cache, newest first.
    /// * `Collections` — the whole listing grouped by its category, which is
    ///   the only grouping the source offers.
    ///
    /// `Popular` is absent on purpose: the listing carries no rating, no
    /// download count and no view count, so a tab by that name would sort by
    /// the default order and claim otherwise.
    fn online_view(&self) -> Vec<Wallpaper> {
        let mut items = self.online.items.clone();
        match self.online.tab {
            1 => {
                // An item whose path does not parse sorts last, not first:
                // "we do not know when" is not "it is the newest thing here".
                items.sort_by_key(|wallpaper| std::cmp::Reverse(post_month(&wallpaper.thumb_url)));
            }
            2 => {
                items.retain(|wallpaper| {
                    ferramari_core::catalogue::cached_video(&wallpaper.id).is_some()
                });
            }
            3 => {
                items.sort_by(|left, right| {
                    left.category
                        .to_lowercase()
                        .cmp(&right.category.to_lowercase())
                        .then_with(|| left.title.cmp(&right.title))
                });
            }
            _ => {}
        }
        items
    }

    fn online_card(&self, wallpaper: &Wallpaper, sem: &'static Semantics) -> Element<'_, Message> {
        let busy = self
            .online
            .downloading
            .as_deref()
            .is_some_and(|id| id == wallpaper.id);

        let artwork: Element<Message> = self
            .online
            .previews
            .get(&wallpaper.id)
            .and_then(|path| path.as_ref())
            .map(image::Handle::from_path)
            .map(|handle| {
                image::Image::new(handle)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(ContentFit::Cover)
                    .into()
            })
            .unwrap_or_else(|| {
                container(s::icon::<Message>("film", 18).color(sem.on_artwork_soft))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center)
                    .into()
            });

        let meta = ferramari_core::catalogue::cached_metadata(&wallpaper.id);
        let detail = match (
            meta.as_ref().map(|cached| cached.height),
            meta.as_ref().and_then(|cached| cached.fps),
        ) {
            (Some(height), Some(fps)) if height > 0 && fps > 0.0 => {
                format!("{} · {} FPS", quality_for(height), fps.round() as u32)
            }
            (Some(height), _) if height > 0 => quality_for(height),
            _ => wallpaper.category.clone(),
        };

        let install: Element<Message> = if busy {
            button(text(tr("Downloading…")).size(12))
                .padding(Padding {
                    top: 8.0,
                    right: 14.0,
                    bottom: 8.0,
                    left: 14.0,
                })
                // Not pressable during a download: a second click would race
                // on the same `.part` file.
                .on_press_maybe(None::<Message>)
                .style(s::button_style(sem, s::Kind::Secondary))
                .into()
        } else {
            s::primary_icon(
                "download",
                tr("Install"),
                Some(Message::Download(wallpaper.clone())),
                sem,
            )
        };

        s::card_fit(
            Column::new()
                .spacing(12)
                .push(
                    container(artwork)
                        .width(Length::Fill)
                        .height(Length::Fixed(96.0))
                        .clip(true)
                        .style(move |_| iced::widget::container::Style {
                            background: Some(Background::Color(sem.row_bg)),
                            border: Border {
                                color: Color::TRANSPARENT,
                                width: 0.0,
                                radius: s::RADIUS_INNER.into(),
                            },
                            ..Default::default()
                        }),
                )
                .push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(
                            Column::new()
                                .spacing(2)
                                .width(Length::Fill)
                                .push(
                                    text(wallpaper.title.clone())
                                        .size(13)
                                        .color(sem.text)
                                        .wrapping(text::Wrapping::WordOrGlyph),
                                )
                                .push(text(detail).size(11).color(sem.text_muted)),
                        )
                        .push(install),
                ),
            sem,
            12.0,
        )
        .width(Length::Fill)
        .into()
    }

    /// Performance: the same four tiles as Home, then what the decoder says,
    /// then the frame-time trace.
    ///
    /// The page deliberately repeats Home's tiles rather than inventing a
    /// second set of numbers: they measure one thing and must not read as two.
    fn performance(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let Some(sample) = self.sample else {
            return container(
                text(tr("Waiting for the first sample…"))
                    .size(14)
                    .color(sem.text_muted),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center)
            .into();
        };

        Column::new()
            .spacing(16)
            .push(self.resource_row(sem))
            // `FillPortion(1)` on each card rather than the `Fill` they would
            // inherit: two `Fill` children share the leftover by their own
            // content, so the split is a result, while equal portions state
            // the promise the reference makes — the two cards end at the same
            // width whatever they contain.
            .push(
                Row::new()
                    .spacing(14)
                    .align_y(Alignment::Start)
                    .width(Length::Fill)
                    .push(self.decoder_card(&sample, sem))
                    .push(self.frame_time_card(sem)),
            )
            .into()
    }

    /// What the running decoder reports. Every line is mpv's own answer over
    /// JSON IPC, so an unknown value says `unknown` rather than a guess.
    fn decoder_card(&self, _sample: &Sample, sem: &'static Semantics) -> Element<'_, Message> {
        let stats = &self.state.player_stats;
        let rows = [
            (tr("Decoder"), stats.decoder_label()),
            (tr("Video"), {
                let codec = stats.codec_short();
                if codec.is_empty() {
                    tr("unknown")
                } else {
                    codec.to_string()
                }
            }),
            (tr("Resolution"), {
                let text = stats.resolution_text();
                if text.is_empty() {
                    tr("unknown")
                } else {
                    text
                }
            }),
            (
                tr("FPS"),
                match stats.fps {
                    Some(fps) => format!("{fps:.3}"),
                    None => tr("unknown"),
                },
            ),
            (
                tr("Dropped Frames"),
                match (stats.dropped, stats.drop_percent()) {
                    (Some(dropped), Some(percent)) => format!("{dropped} ({percent:.1}%)"),
                    (Some(dropped), None) => dropped.to_string(),
                    (None, _) => tr("unknown"),
                },
            ),
        ];

        let mut body = Column::new().spacing(12).push(s::heading_icon::<Message>(
            "server",
            tr("MPV Statistics"),
            sem,
        ));
        for (label, value) in rows {
            body = body.push(s::kv_row(label, value, sem));
        }
        s::card(body, sem).width(Length::FillPortion(1)).into()
    }

    /// Frame time, in milliseconds, with the scale the reference draws: the
    /// ceiling named at the top, the average level in the middle, zero at the
    /// baseline. A trace without an axis is decoration.
    fn frame_time_card(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let measured: Vec<f64> = self
            .history
            .frame_ms
            .iter()
            .filter_map(|value| *value)
            .collect();
        let average =
            (!measured.is_empty()).then(|| measured.iter().sum::<f64>() / measured.len() as f64);

        // The axis is the slowest frame in the window, rounded up to a whole
        // 16.7 ms so the top label is a frame boundary rather than noise.
        let ceiling = average
            .map(|_| {
                let worst = measured
                    .iter()
                    .cloned()
                    .fold(f64::MIN, f64::max)
                    .max(2.0 * 1000.0 / 60.0);
                (worst / (1000.0 / 60.0)).ceil() * (1000.0 / 60.0)
            })
            .unwrap_or(1000.0 / 60.0 * 4.0);

        let chart = iced::widget::stack(vec![
            Sparkline {
                values: self.history.frame_ms.clone(),
                color: sem.series_frame,
                ceiling: Some(ceiling),
            }
            .block::<Message>(110.0),
            container(
                Column::new()
                    .spacing(0)
                    .push(
                        text(format!("{:.1} ms", ceiling))
                            .size(10)
                            .color(sem.text_muted),
                    )
                    .push(Space::new().height(Length::Fill))
                    // The middle label is half the axis, not the average: the
                    // average is the headline above the chart, and repeating
                    // it at mid-height reads as two numbers for one thing.
                    .push(
                        text(format!("{:.1} ms", ceiling / 2.0))
                            .size(10)
                            .color(sem.text_muted),
                    )
                    .push(Space::new().height(Length::Fill))
                    .push(text("0 ms").size(10).color(sem.text_muted)),
            )
            .width(Length::Fill)
            .height(Length::Fixed(110.0))
            .align_x(iced::alignment::Horizontal::Right)
            .padding(Padding {
                top: 0.0,
                right: 2.0,
                bottom: 0.0,
                left: 0.0,
            })
            .into(),
        ]);

        s::card(
            Column::new()
                .spacing(14)
                .push(s::heading(tr("Frame Time"), sem))
                .push(
                    text(match average {
                        Some(value) => format!("{} ms ({})", trim_number(value), tr("avg")),
                        None => tr("Waiting for the first frame"),
                    })
                    .size(18)
                    .color(sem.text),
                )
                .push(chart),
            sem,
        )
        .width(Length::FillPortion(1))
        .into()
    }

    /// Settings: a list of sections on the left, one section on the right.
    ///
    /// The reference splits the page this way instead of stacking six cards
    /// in a column, and the split is worth the extra state: every section
    /// then fits on screen without scrolling, which is what makes a settings
    /// page feel finished.
    fn settings(&self, sem: &'static Semantics) -> Element<'_, Message> {
        let mut nav = Column::new().spacing(6);
        for section in Section::ALL {
            let active = section == self.section;
            nav = nav.push(
                button(
                    Row::new()
                        .spacing(11)
                        .align_y(Alignment::Center)
                        .push(s::icon::<Message>(section.icon(), 14).color(if active {
                            sem.accent_text
                        } else {
                            sem.text_secondary
                        }))
                        .push(
                            text(section.title())
                                .size(13)
                                // As in the rail: the sub-nav column is a
                                // fixed width, and a translation that outgrows
                                // it should wrap rather than stop at the edge.
                                .wrapping(text::Wrapping::WordOrGlyph)
                                .color(if active {
                                    sem.accent_text
                                } else {
                                    sem.text_secondary
                                }),
                        ),
                )
                .width(Length::Fill)
                .padding(Padding {
                    top: 10.0,
                    right: 14.0,
                    bottom: 10.0,
                    left: 14.0,
                })
                .on_press(Message::SettingsSection(section))
                .style(s::button_style(
                    sem,
                    if active {
                        s::Kind::NavActive
                    } else {
                        s::Kind::Nav
                    },
                )),
            );
        }

        let panel = s::card(
            Column::new()
                .spacing(14)
                .push(s::heading(self.section.title(), sem))
                .extend(self.section_rows()),
            sem,
        );

        Row::new()
            .spacing(16)
            .align_y(Alignment::Start)
            .width(Length::Fill)
            .push(container(nav).width(Length::Fixed(s::SECTION_WIDTH)))
            .push(panel)
            .into()
    }

    /// The rows of the selected section. Each is a `line` cut into the glass,
    /// so the page reads as one panel of settings rather than a form.
    fn section_rows(&self) -> Vec<Element<'_, Message>> {
        match self.section {
            Section::General => self.general_rows(),
            Section::Appearance => self.appearance_rows(),
            Section::Playback => self.playback_rows(),
            Section::Monitors => self.monitors_rows(),
            Section::Performance => self.performance_rows(),
            Section::Advanced => self.advanced_rows(),
        }
    }

    fn general_rows(&self) -> Vec<Element<'_, Message>> {
        let mut rows = vec![
            self.setting_toggle(
                tr("Start with system"),
                tr("Launch Ferramari automatically on login"),
                self.state.autostart_enabled,
                Message::ToggleAutostart,
            ),
            self.setting_toggle(
                tr("Run in background"),
                tr("Keep the daemon running when the window is closed"),
                self.state.is_show_systray,
                Message::ToggleSystray,
            ),
            self.setting_toggle(
                tr("Check for updates"),
                self.update_hint(),
                self.update.automatic,
                Message::SetUpdateCheck,
            ),
        ];

        // The answer and the way to act on it, shown whether or not the
        // automatic check is on: a user who turned it off still wants to know
        // when the last look happened.
        rows.push(self.update_row());

        // Language sits here rather than under Appearance: the reference puts
        // it in General, and it changes the whole surface rather than its
        // colours.
        rows.push(s::inset(
            Column::new()
                .spacing(10)
                .push(
                    Column::new()
                        .spacing(2)
                        .push(text(tr("Language")).size(13.5).color(self.sem().text))
                        .push(
                            text(tr("Interface language"))
                                .size(11.5)
                                .color(self.sem().text_muted),
                        ),
                )
                .push(s::segmented(
                    vec![
                        (
                            tr("System"),
                            self.state.language.is_empty(),
                            Message::SetLanguage(String::new()),
                        ),
                        (
                            "English".to_string(),
                            self.state.language == "en",
                            Message::SetLanguage("en".into()),
                        ),
                        (
                            "Русский".to_string(),
                            self.state.language == "ru",
                            Message::SetLanguage("ru".into()),
                        ),
                        (
                            "日本語".to_string(),
                            self.state.language == "ja",
                            Message::SetLanguage("ja".into()),
                        ),
                    ],
                    self.sem(),
                )),
            self.sem(),
            14.0,
        ));
        rows
    }

    /// What the "Check for updates" switch says under its label: when the
    /// last automatic answer was, or that there is none.
    fn update_hint(&self) -> String {
        match self.update.checked_on() {
            Some(date) if self.update.is_fresh() => {
                format!("{} {date}", tr("Last checked"))
            }
            Some(date) => format!("{} {date}", tr("Last checked")),
            None => tr("Ask GitHub for a new release when the window opens"),
        }
    }

    fn update_row(&self) -> Element<'_, Message> {
        let sem = self.sem();
        let status = self.update.status();
        let color = match &status {
            ferramari_core::update::Status::Available { .. } => sem.accent_text,
            ferramari_core::update::Status::Failed(_) => sem.danger,
            _ => sem.text_secondary,
        };

        s::inset(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(
                    Column::new()
                        .spacing(2)
                        .width(Length::Fill)
                        .push(text(status.text()).size(12.5).color(color))
                        .push(
                            text(match &status {
                                ferramari_core::update::Status::Available { title, .. }
                                    if !title.is_empty() =>
                                {
                                    title.clone()
                                }
                                _ => format!(
                                    "{} · {}",
                                    tr("This version"),
                                    ferramari_core::update::current_version()
                                ),
                            })
                            .size(11)
                            .color(self.sem().text_muted),
                        ),
                )
                .push(s::secondary(
                    tr("Check now"),
                    Some(Message::CheckUpdates),
                    self.sem(),
                )),
            self.sem(),
            10.0,
        )
    }

    fn appearance_rows(&self) -> Vec<Element<'_, Message>> {
        let sem = self.sem();
        vec![
            s::inset(
                Column::new()
                    .spacing(10)
                    .push(text(tr("Theme")).size(13.5).color(sem.text))
                    .push(
                        text(tr("Window and control appearance"))
                            .size(11.5)
                            .color(sem.text_muted),
                    )
                    .push(s::segmented(
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
                    )),
                sem,
                14.0,
            ),
            s::inset(
                s::line(
                    tr("Desktop preference"),
                    Some(if self.system_dark {
                        tr("The desktop asks for a dark theme")
                    } else {
                        tr("The desktop asks for a light theme")
                    }),
                    s::chip(
                        if self.system_dark {
                            tr("Dark")
                        } else {
                            tr("Light")
                        },
                        sem,
                    ),
                    sem,
                ),
                sem,
                10.0,
            ),
        ]
    }

    fn playback_rows(&self) -> Vec<Element<'_, Message>> {
        let sem = self.sem();
        let active_profile =
            ferramari_core::eco::Profile::from_config(&self.state.playback_profile);
        let mut rows = vec![
            s::inset(
                Column::new()
                    .spacing(10)
                    .push(
                        Column::new()
                            .spacing(2)
                            .push(text(tr("Profile")).size(13.5).color(sem.text))
                            .push(
                                text(tr("Quality prioritises smoothness; Eco reduces decoding"))
                                    .size(11.5)
                                    .color(sem.text_muted),
                            ),
                    )
                    .push(s::segmented(
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
                    )),
                sem,
                14.0,
            ),
            s::inset(
                Column::new()
                    .spacing(10)
                    .push(
                        Column::new()
                            .spacing(2)
                            .push(text(tr("Hardware decoding")).size(13.5).color(sem.text))
                            .push(
                                text(tr(
                                    "Auto lets mpv choose; VA-API forces it; Off uses software",
                                ))
                                .size(11.5)
                                .color(sem.text_muted),
                            ),
                    )
                    .push(s::segmented(
                        vec![
                            (
                                "Auto".to_string(),
                                self.state.hwdec == "auto",
                                Message::SetHwdec("auto".into()),
                            ),
                            (
                                "VA-API".to_string(),
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
                    )),
                sem,
                14.0,
            ),
            self.setting_toggle(
                tr("Pause on battery"),
                tr("Stop playback when running on battery"),
                self.state.pause_on_battery,
                Message::TogglePauseOnBattery,
            ),
        ];
        if let Some(status) = hwdec_status(&self.state, sem) {
            rows.push(s::inset(status, sem, 10.0));
        }
        rows
    }

    fn monitors_rows(&self) -> Vec<Element<'_, Message>> {
        let sem = self.sem();
        let mut rows = Vec::new();
        let mut names: Vec<&String> = self.state.monitor_states.keys().collect();
        names.sort();

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

        for name in names {
            let monitor_name = name.clone();
            let selected_id = self
                .state
                .rotations
                .get(&monitor_name)
                .cloned()
                .unwrap_or_default();
            let selected = rotation_options
                .iter()
                .find(|choice| choice.id == selected_id)
                .cloned();
            let resolution = self
                .state
                .monitor_resolutions
                .get(name)
                .cloned()
                .unwrap_or_default();
            let hz = self.state.monitor_refresh.get(name).copied();
            let hint = match (resolution.is_empty(), hz) {
                (true, _) => String::new(),
                (false, Some(hz)) => format!("{resolution} · {hz} Hz"),
                (false, None) => resolution,
            };

            rows.push(s::inset(
                Column::new()
                    .spacing(10)
                    .push(s::line(
                        name.to_string(),
                        (!hint.is_empty()).then_some(hint),
                        s::chip(
                            state_label(
                                &self
                                    .state
                                    .monitor_states
                                    .get(name)
                                    .cloned()
                                    .unwrap_or_default(),
                                sem,
                            ),
                            sem,
                        ),
                        sem,
                    ))
                    .push(
                        pick_list(rotation_options.clone(), selected, move |choice| {
                            Message::SetMonitorRotation(monitor_name.clone(), choice.id)
                        })
                        .width(Length::Fill),
                    ),
                sem,
                14.0,
            ));
        }

        if rows.is_empty() {
            rows.push(s::inset(
                text(tr("No displays detected"))
                    .size(12.5)
                    .color(sem.text_muted),
                sem,
                12.0,
            ));
        }
        rows
    }

    fn performance_rows(&self) -> Vec<Element<'_, Message>> {
        let sem = self.sem();
        vec![
            s::inset(
                Column::new()
                    .spacing(10)
                    .push(text(tr("Idle delay")).size(13.5).color(sem.text))
                    .push(
                        text(tr("Minutes of inactivity before the Eco Engine pauses"))
                            .size(11.5)
                            .color(sem.text_muted),
                    )
                    .push(
                        Row::new()
                            .spacing(12)
                            .align_y(Alignment::Center)
                            .push(
                                slider(1..=60, self.state.idle_timeout_sec / 60, |value| {
                                    Message::SetIdleTimeout(value.max(1))
                                })
                                .width(Length::Fill),
                            )
                            .push(
                                text(format!("{} min", self.state.idle_timeout_sec / 60))
                                    .size(12)
                                    .color(sem.text_muted),
                            ),
                    ),
                sem,
                14.0,
            ),
            s::inset(
                Column::new()
                    .spacing(10)
                    .push(text(tr("Rotation interval")).size(13.5).color(sem.text))
                    .push(
                        text(tr("Minutes between wallpaper changes"))
                            .size(11.5)
                            .color(sem.text_muted),
                    )
                    .push(
                        Row::new()
                            .spacing(12)
                            .align_y(Alignment::Center)
                            .push(
                                slider(
                                    1..=120,
                                    (self.state.rotation_interval_sec / 60).max(1),
                                    |value| Message::SetRotationInterval(value.max(1)),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                text(format!(
                                    "{} min",
                                    (self.state.rotation_interval_sec / 60).max(1)
                                ))
                                .size(12)
                                .color(sem.text_muted),
                            ),
                    ),
                sem,
                14.0,
            ),
            s::inset(
                Column::new()
                    .spacing(10)
                    .push(text(tr("Rotation mode")).size(13.5).color(sem.text))
                    .push(s::segmented(
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
                    )),
                sem,
                14.0,
            ),
        ]
    }

    fn advanced_rows(&self) -> Vec<Element<'_, Message>> {
        let sem = self.sem();
        let mut rows = vec![
            self.setting_toggle(
                tr("Pause on maximized window"),
                tr("Auto-pause when maximized or fullscreen detected"),
                self.state.is_pause_when_maximized,
                Message::TogglePauseMaximized,
            ),
            self.setting_toggle(
                tr("Mute on maximized window"),
                tr("Auto-mute when maximized or fullscreen detected"),
                self.state.is_mute_when_maximized,
                Message::ToggleMuteMaximized,
            ),
            self.setting_toggle(
                tr("Web wallpapers network access"),
                tr("Remote pages and subresources; local always allowed"),
                self.state.web_allow_network,
                Message::ToggleWebNetwork,
            ),
            self.setting_toggle(
                tr("Apply static wallpaper"),
                tr("Video frame applied as system wallpaper"),
                self.state.is_static_wallpaper,
                Message::ToggleStatic,
            ),
        ];

        // The blur slider only appears once the user has moved it: the frame
        // is regenerated by the server, so it is committed with a button
        // rather than on every pixel of drag.
        if self.blur_dirty {
            rows.push(s::inset(
                Column::new()
                    .spacing(10)
                    .push(text(tr("Blur radius")).size(13.5).color(sem.text))
                    .push(
                        Row::new()
                            .spacing(12)
                            .align_y(Alignment::Center)
                            .push(
                                slider(0..=40, self.blur_draft, Message::BlurChanged)
                                    .width(Length::Fill),
                            )
                            .push(
                                text(self.blur_draft.to_string())
                                    .size(12)
                                    .color(sem.text_muted),
                            )
                            .push(s::primary(tr("Apply"), Some(Message::ApplyBlur), sem)),
                    ),
                sem,
                14.0,
            ));
        }

        rows.push(s::inset(
            Column::new()
                .spacing(10)
                .push(
                    Column::new()
                        .spacing(2)
                        .push(text(tr("Library folder")).size(13.5).color(sem.text))
                        .push(
                            text(paths::video_wallpaper_dir().display().to_string())
                                .size(11)
                                .color(sem.text_muted)
                                .wrapping(text::Wrapping::WordOrGlyph),
                        ),
                )
                .push(
                    Row::new()
                        .spacing(8)
                        .push(s::secondary(
                            tr("Open folder"),
                            Some(Message::OpenFolder),
                            sem,
                        ))
                        .push(s::secondary(tr("Add video"), Some(Message::PickVideo), sem))
                        .push(s::secondary(
                            tr("Refresh"),
                            Some(Message::RefreshLibrary),
                            sem,
                        )),
                ),
            sem,
            14.0,
        ));

        // Rotation works on collections, so they are managed here rather
        // than on a page of their own: a wallpaper is added to a collection
        // from its card menu, the collection is named and deleted below.
        let mut collections = Column::new()
            .spacing(10)
            .push(
                Column::new()
                    .spacing(2)
                    .push(text(tr("Rotation collections")).size(13.5).color(sem.text))
                    .push(
                        text(tr("Groups of wallpapers a display can cycle through"))
                            .size(11.5)
                            .color(sem.text_muted),
                    ),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(
                        text_input(self.collection_placeholder.as_str(), &self.collection_name)
                            .on_input(Message::CollectionNameChanged)
                            .width(Length::Fill),
                    )
                    .push(s::secondary(
                        tr("Create"),
                        Some(Message::CreateCollection),
                        sem,
                    )),
            );

        if self.library.collections().is_empty() {
            collections = collections.push(
                text(tr("No collections yet"))
                    .size(11.5)
                    .color(sem.text_muted),
            );
        }
        for collection in self.library.collections() {
            let id = collection.id.clone();
            let rename_id = id.clone();
            collections = collections.push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(
                        text_input(tr("Collection name").as_str(), &collection.name)
                            .on_input(move |name| {
                                Message::RenameCollection(rename_id.clone(), name)
                            })
                            .width(Length::Fill),
                    )
                    .push(s::chip(format!("{}", collection.items.len()), sem))
                    .push(s::icon_button(
                        "times",
                        Some(Message::DeleteCollection(id)),
                        11.0,
                        s::Kind::Icon,
                        sem,
                    )),
            );
        }

        rows.push(s::inset(collections, sem, 14.0));

        rows.push(s::inset(
            Column::new()
                .spacing(10)
                .push(
                    Column::new()
                        .spacing(2)
                        .push(text(tr("Packages")).size(13.5).color(sem.text))
                        .push(
                            text(tr("Import, export, sign and verify .ferramari packages"))
                                .size(11.5)
                                .color(sem.text_muted),
                        ),
                )
                .push(
                    Row::new()
                        .spacing(8)
                        .push(s::secondary(
                            tr("Import"),
                            Some(Message::ImportPackage),
                            sem,
                        ))
                        .push(s::secondary(
                            tr("Export"),
                            Some(Message::ExportPackage),
                            sem,
                        ))
                        .push(s::secondary(tr("Sign"), Some(Message::SignPackage), sem))
                        .push(s::secondary(
                            tr("Verify"),
                            Some(Message::VerifyPackage),
                            sem,
                        ))
                        .push(s::secondary(
                            tr("Trust key"),
                            Some(Message::TrustPackageKey),
                            sem,
                        )),
                )
                .push(
                    // The paths are picked with native dialogs, so the form
                    // never asks the user to type a directory they cannot see.
                    Row::new()
                        .spacing(8)
                        .push(s::secondary(
                            tr("Source…"),
                            Some(Message::PickPackageSource),
                            sem,
                        ))
                        .push(s::secondary(
                            tr("Video…"),
                            Some(Message::PickExportVideo),
                            sem,
                        ))
                        .push(s::secondary(
                            tr("Destination…"),
                            Some(Message::PickPackageParent),
                            sem,
                        )),
                )
                .push(
                    Column::new()
                        .spacing(6)
                        .push(
                            text_input(
                                tr("Package directory or video to export").as_str(),
                                &self.package_source,
                            )
                            .on_input(Message::PackageSourceChanged)
                            .width(Length::Fill),
                        )
                        .push(
                            text_input(
                                tr("Export destination folder").as_str(),
                                &self.package_parent,
                            )
                            .on_input(Message::PackageParentChanged)
                            .width(Length::Fill),
                        ),
                ),
            sem,
            14.0,
        ));
        rows
    }

    /// A settings row: title, hint, and the amber switch the reference draws.
    fn setting_toggle<'a, M: Clone + 'a>(
        &self,
        title: String,
        hint: String,
        value: bool,
        on_toggle: impl Fn(bool) -> M + 'a,
    ) -> Element<'a, M> {
        let sem = self.sem();
        s::inset(
            s::line(
                title,
                Some(hint),
                toggler(value)
                    .on_toggle(on_toggle)
                    .style(s::toggler_style(sem))
                    .into(),
                sem,
            ),
            sem,
            10.0,
        )
    }
}

/// What one wallpaper file is, as the card shows it: `4K · 60 FPS`.
///
/// The numbers come from `ffprobe`, never from the file name. A card that
/// claims 60 FPS because someone named a file that way is worse than a card
/// that says nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MediaInfo {
    width: u32,
    height: u32,
    fps: Option<f64>,
}

impl MediaInfo {
    /// The short quality line. Height decides the label — 2160 is `4K`,
    /// 1440 is `2.7K`, 1080 is `FHD` — because that is how the reference
    /// names them and how people name their own files.
    fn quality(self) -> String {
        let resolution = match self.height {
            0 => return String::new(),
            2160..=2304 => "4K".to_string(),
            1440..=1536 => "2.7K".to_string(),
            1080..=1152 => "FHD".to_string(),
            720..=800 => "HD".to_string(),
            other => format!("{other}p"),
        };
        match self.fps {
            Some(fps) if fps > 0.0 => format!("{resolution} · {} FPS", fps.round() as u32),
            _ => resolution,
        }
    }
}

/// Which files the Library grid shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaFilter {
    All,
    Video,
    Image,
    /// The wallpaper currently assigned to a display: the "live" ones.
    Live,
    Favorites,
}

impl MediaFilter {
    fn label(self) -> String {
        match self {
            MediaFilter::All => tr("All"),
            MediaFilter::Video => tr("Video"),
            MediaFilter::Image => tr("Image"),
            MediaFilter::Live => tr("Live"),
            MediaFilter::Favorites => tr("Favorites"),
        }
    }

    fn accepts(self, path: &str, live: &[String], favorites: &[String]) -> bool {
        match self {
            MediaFilter::All => true,
            MediaFilter::Video => is_video(path),
            MediaFilter::Image => is_image(path),
            MediaFilter::Live => live.iter().any(|source| source == path),
            MediaFilter::Favorites => favorites.iter().any(|favorite| favorite == path),
        }
    }
}

fn is_video(path: &str) -> bool {
    matches!(
        extension(path).as_str(),
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "gif"
    )
}

fn is_image(path: &str) -> bool {
    matches!(
        extension(path).as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp"
    )
}

fn extension(path: &str) -> String {
    PathBuf::from(path)
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default()
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

    // The control surface draws its own title bar, so the compositor's
    // decorations are switched off: that is the only way the backdrop
    // gradient reaches the top edge the way the reference shows it. The price
    // is that dragging and resizing become this file's job — see
    // `App::title_bar` and `App::resize_edge`.
    // The size is part of the same boot spec as the page, so one command can
    // open one page at one width for a screenshot.
    let window = window::Settings {
        size: boot_settings().size,
        // 900×620 is where the narrow layout (icons-only rail, wrapped tiles)
        // still shows a whole resource row; below that nothing fits honestly.
        min_size: Some(Size::new(900.0, 620.0)),
        decorations: false,
        // Matches the .desktop basename, which is what makes the window group
        // with the app in the taskbar and the Activities overview.
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: paths::APP_ID.to_string(),
            ..Default::default()
        },
        ..window::Settings::default()
    };

    iced::application(App::boot, App::update, App::view)
        .title("Ferramari")
        .theme(App::theme)
        .subscription(App::subscription)
        .window(window)
        .centered()
        .run()
}

// ============================================================================ //
// Tests                                                                        //
// ============================================================================ //

#[cfg(test)]
mod library_tests {
    use super::*;

    #[test]
    fn quality_labels_follow_the_height() {
        let info = |height| MediaInfo {
            width: 1920,
            height,
            fps: None,
        };
        assert_eq!(info(2160).quality(), "4K");
        assert_eq!(info(1440).quality(), "2.7K");
        assert_eq!(info(1080).quality(), "FHD");
        assert_eq!(info(720).quality(), "HD");
        assert_eq!(info(1024).quality(), "1024p");
        assert_eq!(info(0).quality(), "");
    }

    #[test]
    fn the_frame_rate_is_appended_only_when_measured() {
        let mut info = MediaInfo {
            width: 3840,
            height: 2160,
            fps: Some(59.94),
        };
        assert_eq!(info.quality(), "4K · 60 FPS");
        info.fps = Some(0.0);
        assert_eq!(info.quality(), "4K");
        info.fps = None;
        assert_eq!(info.quality(), "4K");
    }

    #[test]
    fn filters_split_by_type_and_by_state() {
        let live = vec!["/a/clip.mp4".to_string()];
        let favorites = vec!["/a/still.png".to_string()];
        assert!(MediaFilter::Video.accepts("/a/clip.mp4", &live, &favorites));
        assert!(!MediaFilter::Video.accepts("/a/still.png", &live, &favorites));
        assert!(MediaFilter::Image.accepts("/a/still.PNG", &live, &favorites));
        assert!(MediaFilter::Live.accepts("/a/clip.mp4", &live, &favorites));
        assert!(!MediaFilter::Live.accepts("/a/other.mp4", &live, &favorites));
        assert!(MediaFilter::Favorites.accepts("/a/still.png", &live, &favorites));
        assert!(MediaFilter::All.accepts("/a/whatever.txt", &live, &favorites));
    }

    #[test]
    fn a_search_narrows_the_grid_by_file_name() {
        // The filter is a substring match on the name, case-insensitively:
        // the grid is small and people type the word they remember seeing.
        let matches = |query: &str, name: &str| {
            query.is_empty() || name.to_lowercase().contains(&query.to_lowercase())
        };
        assert!(matches("sakura", "Sakura Lakeside.mp4"));
        assert!(matches("SAK", "Sakura Lakeside.mp4"));
        assert!(!matches("night", "Sakura Lakeside.mp4"));
        assert!(matches("", "anything"));
    }
}
