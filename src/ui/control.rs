//! The control window: local videos, streaming URLs, web pages and quick
//! toggles. Built in code (no GtkBuilder XML) so widget names are checked by
//! the compiler.
//!
//! Layout: a custom header, a glass sidebar with navigation and live playback
//! state, and a content stack. The video library is a real FlowBox card grid;
//! the smaller screens hide the sidebar and promote navigation to a top rail.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gdk_pixbuf::Pixbuf;
use gtk::atk::prelude::AtkObjectExt as _;
use gtk::gdk;
use gtk::gdk::prelude::GdkContextExt as _;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;

use crate::app::Command;
use crate::config::{Config, Mode};
use crate::library::Library;
use crate::monitors::MonitorInfo;
use crate::paths;
use crate::thumbnails;
use crate::ui::online;
use crate::ui::theme::Theme;
use crate::util::{self, tr, tr_n};

pub(crate) type ThumbSlot = Rc<RefCell<Option<Pixbuf>>>;
type ThumbStates = Rc<RefCell<Vec<Option<(ThumbSlot, gtk::DrawingArea)>>>>;

#[derive(Clone, PartialEq, Eq)]
enum LibraryView {
    All,
    Favorites,
    Recent,
    Collection(String),
}

/// A GLib source that may already have removed itself after returning
/// `ControlFlow::Break`. `SourceId::remove` panics for such sources, so the
/// completion flag lets cleanup skip them.
pub(crate) struct PendingTimer {
    pub(crate) id: glib::SourceId,
    pub(crate) done: Rc<Cell<bool>>,
}

impl PendingTimer {
    pub(crate) fn cancel(self) {
        if !self.done.get() {
            self.id.remove();
        }
    }
}

struct Shared {
    tx: crate::app::CommandSender,
    videos: RefCell<Vec<PathBuf>>,
    monitors: RefCell<Vec<MonitorInfo>>,
    menu: RefCell<Option<gtk::Menu>>,
    thumb_timer: RefCell<Option<PendingTimer>>,
    thumb_cancel: RefCell<Option<Arc<AtomicBool>>>,
}

struct HeaderParts {
    header: gtk::HeaderBar,
    play_button: gtk::Button,
    status_chip: gtk::Box,
    status_dot: gtk::Box,
    status_text: gtk::Label,
    brand_subtitle: gtk::Label,
}

struct VolumeParts {
    button: gtk::MenuButton,
    scale: gtk::Scale,
    toggle: gtk::ToggleButton,
    label: gtk::Label,
}

struct SettingsPage {
    root: gtk::ScrolledWindow,
    monitors_summary: gtk::Label,
    combo_theme: gtk::ComboBoxText,
    combo_language: gtk::ComboBoxText,
    rotation_mode: gtk::ComboBoxText,
    rotation_interval: gtk::SpinButton,
    profile: gtk::ComboBoxText,
    switch_pause_battery: gtk::Switch,
    battery_status: gtk::Label,
    switch_autostart: gtk::Switch,
    switch_static: gtk::Switch,
    switch_pause_max: gtk::Switch,
    switch_mute_max: gtk::Switch,
    spin_blur: gtk::SpinButton,
}

#[derive(Clone)]
struct VideoPage {
    root: gtk::Box,
    refresh: gtk::Button,
    flow: gtk::FlowBox,
    library_stack: gtk::Stack,
    empty_title: gtk::Label,
    empty_body: gtk::Label,
    count: gtk::Label,
    selection_label: gtk::Label,
    apply_button: gtk::Button,
    empty_folder: gtk::Button,
    empty_clear: gtk::Button,
    search: gtk::Entry,
    view: gtk::ComboBoxText,
    view_syncing: Rc<Cell<bool>>,
    new_collection: gtk::Button,
    rename_collection: gtk::Button,
    delete_collection: gtk::Button,
    add_to_collection: gtk::Button,
    remove_from_collection: gtk::Button,
}

struct StreamPage {
    root: gtk::ScrolledWindow,
    entry: gtk::Entry,
    spinner: gtk::Spinner,
    status_box: gtk::Box,
    status: gtk::Label,
}

struct WebPage {
    root: gtk::ScrolledWindow,
    entry: gtk::Entry,
}

struct MonitorCard {
    name: String,
    source: gtk::Label,
    meta: gtk::Label,
    state: gtk::Label,
    combo: gtk::ComboBoxText,
    rotation: gtk::ComboBoxText,
}

struct HomePage {
    root: gtk::ScrolledWindow,
    now_title: gtk::Label,
    now_source: gtk::Label,
    now_status: gtk::Label,
    monitors_box: gtk::Box,
    cards: RefCell<Vec<MonitorCard>>,
    videos: RefCell<Vec<PathBuf>>,
    collections: RefCell<Vec<(String, String)>>,
    syncing: Rc<Cell<bool>>,
    tx: crate::app::CommandSender,
}

/// The parts that switch between the sidebar and the compact top rail.
struct Responsive {
    window: gtk::ApplicationWindow,
    sidebar: gtk::Box,
    top_nav: gtk::Box,
    status_chip: gtk::Box,
    brand_subtitle: gtk::Label,
    last: Cell<Option<(bool, bool, bool, bool)>>,
}

impl Responsive {
    fn apply(&self, width: i32) {
        let narrow = width < 900;
        let show_status = width >= 1020;
        let show_subtitle = width >= 800;
        let state = (narrow, show_status, show_subtitle, width < 620);
        if self.last.get() == Some(state) {
            return;
        }
        self.last.set(Some(state));

        self.sidebar.set_visible(!narrow);
        self.top_nav.set_visible(narrow);
        self.status_chip.set_visible(show_status);
        self.brand_subtitle.set_visible(show_subtitle);

        let context = self.window.style_context();
        if narrow {
            context.add_class("narrow");
        } else {
            context.remove_class("narrow");
        }
    }
}

pub struct ControlWindow {
    pub window: gtk::ApplicationWindow,
    shared: Rc<Shared>,
    _theme: Rc<Theme>,
    scale_volume: gtk::Scale,
    toggle_mute: gtk::ToggleButton,
    volume_button: gtk::MenuButton,
    volume_label: gtk::Label,
    spin_blur: gtk::SpinButton,
    switch_autostart: gtk::Switch,
    switch_static: gtk::Switch,
    switch_pause_max: gtk::Switch,
    switch_mute_max: gtk::Switch,
    combo_theme: gtk::ComboBoxText,
    combo_language: gtk::ComboBoxText,
    rotation_mode: gtk::ComboBoxText,
    rotation_interval: gtk::SpinButton,
    profile: gtk::ComboBoxText,
    switch_pause_battery: gtk::Switch,
    battery_status: gtk::Label,
    settings_monitors: gtk::Label,
    play_button: gtk::Button,
    stack: gtk::Stack,
    video_search: gtk::Entry,
    web_entry: gtk::Entry,
    syncing: Rc<Cell<bool>>,
    responsive: Rc<Responsive>,
    status_dot: gtk::Box,
    status_text: gtk::Label,
    now_dot: gtk::Box,
    now_status: gtk::Label,
    now_mode: gtk::Label,
    now_source: gtk::Label,
    now_monitor: gtk::Label,
    stream_page: StreamPage,
    home: HomePage,
    library: Rc<RefCell<Library>>,
    video_page: VideoPage,
}

impl ControlWindow {
    pub fn new(
        app: &gtk::Application,
        tx: crate::app::CommandSender,
        version: &str,
        library: Rc<RefCell<Library>>,
    ) -> Self {
        let window = gtk::ApplicationWindow::new(app);
        window.set_title("Ferramari");
        window.set_default_size(1100, 680);
        window.set_size_request(680, 520);
        window.set_position(gtk::WindowPosition::Center);
        window.set_icon_name(Some(paths::APP_ID));

        let theme = Theme::install(&window);
        let syncing = Rc::new(Cell::new(false));
        let shared = Rc::new(Shared {
            tx: tx.clone(),
            videos: RefCell::new(Vec::new()),
            monitors: RefCell::new(Vec::new()),
            menu: RefCell::new(None),
            thumb_timer: RefCell::new(None),
            thumb_cancel: RefCell::new(None),
        });

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(180);
        stack.set_hexpand(true);
        stack.set_vexpand(true);

        let volume = build_volume_popover(&tx, &syncing);
        let header_parts = build_header(&tx, &volume, &stack);

        let home_page = build_home_page(&stack, &tx);
        stack.add_titled(&home_page.root, "home", &tr("Home"));

        let settings_page = build_settings_page(&window, &tx, &syncing, version);
        stack.add_titled(&settings_page.root, "settings", &tr("Settings"));

        let video_page = build_video_page();
        stack.add_titled(&video_page.root, "video", &tr("Local Video"));

        let online_context = {
            let apply_shared = Rc::downgrade(&shared);
            let refresh_shared = Rc::downgrade(&shared);
            let refresh_page = video_page.clone();
            let refresh_library = Rc::clone(&library);
            online::Context {
                apply: Rc::new(move |button, path| {
                    if let Some(shared) = apply_shared.upgrade() {
                        show_monitor_menu(&shared, button, None, path);
                    }
                }),
                refresh_library: Rc::new(move || {
                    if let Some(shared) = refresh_shared.upgrade() {
                        let view = view_from_page(&refresh_page);
                        fill_grid(&shared, &refresh_page, &refresh_library, &view);
                    }
                }),
            }
        };
        let online_page = online::build(online_context);
        stack.add_titled(&online_page.root, "online", &tr("Wallpapers"));
        {
            let online_page = Rc::clone(&online_page);
            stack.connect_notify_local(Some("visible-child-name"), move |stack, _| {
                if stack.visible_child_name().as_deref() == Some("online") {
                    online::show(&online_page);
                }
            });
        }

        let stream_page = build_stream_page(&tx);
        stack.add_titled(&stream_page.root, "stream", &tr("Streaming"));

        let web_page = build_web_page(&tx);
        stack.add_titled(&web_page.root, "webpage", &tr("Web Page"));

        let (sidebar, top_nav) = build_navigation(&stack);
        let now = build_now_playing_card();
        let version_label = label(&format!("Ferramari {version}"), "field-hint");
        version_label.set_margin_start(8);
        version_label.set_margin_top(4);
        sidebar.pack_end(&version_label, false, false, 0);
        sidebar.pack_end(&now.card, false, false, 0);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        root.set_margin_start(18);
        root.set_margin_end(18);
        root.set_margin_top(18);
        root.set_margin_bottom(18);

        let main_area = gtk::Box::new(gtk::Orientation::Vertical, 12);
        main_area.set_hexpand(true);
        main_area.set_vexpand(true);
        main_area.pack_start(&top_nav, false, false, 0);
        main_area.pack_start(&stack, true, true, 0);

        root.pack_start(&sidebar, false, false, 0);
        root.pack_start(&main_area, true, true, 0);

        let overlay = gtk::Overlay::new();
        overlay.add(&theme.background());
        overlay.add_overlay(&root);
        window.add(&overlay);
        window.set_titlebar(Some(&header_parts.header));

        // The sidebar and top rail are both navigation views of the same
        // stack; the responsive handler only chooses which one is visible.
        stack.set_visible_child_name("home");

        {
            let tx = tx.clone();
            header_parts.play_button.connect_clicked(move |_| {
                let _ = tx.send(Command::TogglePause);
            });
        }

        wire_video_page(&video_page, &shared, &library);
        refresh_collection_choices(&video_page, &library.borrow());
        fill_grid(&shared, &video_page, &library, &LibraryView::All);

        window.connect_delete_event(|window, _| {
            window.hide();
            glib::Propagation::Stop
        });

        let responsive = Rc::new(Responsive {
            window: window.clone(),
            sidebar: sidebar.clone(),
            top_nav: top_nav.clone(),
            status_chip: header_parts.status_chip.clone(),
            brand_subtitle: header_parts.brand_subtitle.clone(),
            last: Cell::new(None),
        });
        {
            let responsive = Rc::clone(&responsive);
            window.connect_size_allocate(move |_, allocation| {
                responsive.apply(allocation.width());
            });
        }

        let control = Self {
            window,
            shared,
            _theme: theme,
            scale_volume: volume.scale,
            toggle_mute: volume.toggle,
            volume_button: volume.button,
            volume_label: volume.label,
            spin_blur: settings_page.spin_blur,
            switch_autostart: settings_page.switch_autostart,
            switch_static: settings_page.switch_static,
            switch_pause_max: settings_page.switch_pause_max,
            switch_mute_max: settings_page.switch_mute_max,
            combo_theme: settings_page.combo_theme,
            combo_language: settings_page.combo_language,
            rotation_mode: settings_page.rotation_mode,
            rotation_interval: settings_page.rotation_interval,
            profile: settings_page.profile,
            switch_pause_battery: settings_page.switch_pause_battery,
            battery_status: settings_page.battery_status,
            settings_monitors: settings_page.monitors_summary,
            play_button: header_parts.play_button,
            stack: stack.clone(),
            video_search: video_page.search.clone(),
            web_entry: web_page.entry.clone(),
            syncing,
            responsive,
            status_dot: header_parts.status_dot,
            status_text: header_parts.status_text,
            now_dot: now.dot,
            now_status: now.status,
            now_mode: now.mode,
            now_source: now.source,
            now_monitor: now.monitor,
            stream_page,
            home: home_page,
            library: Rc::clone(&library),
            video_page: video_page.clone(),
        };

        control.responsive.apply(control.window.size().0);
        control
    }

    pub fn present(&self) {
        self.responsive.last.set(None);
        self.window.show_all();
        self.responsive.apply(self.window.size().0);
        self.window.present();
        self.focus_active_page();
    }

    /// Rebuild the local library grid after favourites, history or collections
    /// changed outside this window (for example when the app applied a video).
    pub fn reload_library(&self) {
        let view = view_from_page(&self.video_page);
        fill_grid(&self.shared, &self.video_page, &self.library, &view);
    }

    pub fn refresh(
        &self,
        config: &Config,
        monitors: &[MonitorInfo],
        autostart_enabled: bool,
        paused_by_user: bool,
        is_playing: bool,
        battery: crate::power::PowerStatus,
    ) {
        self.home.update(
            config,
            monitors,
            paused_by_user,
            is_playing,
            &self.library.borrow(),
        );
        self.syncing.set(true);
        *self.shared.monitors.borrow_mut() = monitors.to_vec();

        self.switch_autostart.set_active(autostart_enabled);
        let rotation = match config.rotation_mode.as_str() {
            "order" => "order",
            "random" => "random",
            _ => "off",
        };
        if !self.rotation_mode.set_active_id(Some(rotation)) {
            self.rotation_mode.set_active_id(Some("off"));
        }
        self.rotation_interval
            .set_value((config.rotation_interval_sec as f64 / 60.0).clamp(1.0, 1440.0));
        let profile = match config.playback_profile.as_str() {
            "performance" => "performance",
            "power" => "power",
            _ => "balanced",
        };
        if !self.profile.set_active_id(Some(profile)) {
            self.profile.set_active_id(Some("balanced"));
        }
        self.switch_pause_battery
            .set_active(config.pause_on_battery);
        let battery_text = match battery.percent {
            Some(percent) if battery.on_battery => {
                format!("{} {percent}% · {}", tr("Battery"), tr("on battery"))
            }
            Some(percent) => {
                format!("{} {percent}% · {}", tr("Battery"), tr("on AC power"))
            }
            None => tr("No battery detected"),
        };
        self.battery_status.set_text(&battery_text);
        if monitors.is_empty() {
            self.settings_monitors.set_text(&tr("No displays detected"));
        } else {
            let summary = monitors
                .iter()
                .map(|monitor| format!("{} · {} × {}", monitor.name, monitor.width, monitor.height))
                .collect::<Vec<_>>()
                .join("\n");
            self.settings_monitors.set_text(&summary);
        }
        let language = if config.language.is_empty() {
            "system"
        } else {
            config.language.as_str()
        };
        if !self.combo_theme.set_active_id(Some(config.theme.as_str())) {
            self.combo_theme.set_active_id(Some("system"));
        }
        if !self.combo_language.set_active_id(Some(language)) {
            self.combo_language.set_active_id(Some("system"));
        }
        self.switch_static.set_active(config.is_static_wallpaper);
        self.switch_pause_max
            .set_active(config.is_pause_when_maximized);
        self.switch_mute_max
            .set_active(config.is_mute_when_maximized);
        self.scale_volume.set_value(config.volume as f64);
        self.volume_label.set_text(&format!("{}%", config.volume));
        self.spin_blur.set_value(config.blur_radius as f64);
        self.toggle_mute.set_active(config.is_mute);
        self.update_mute_icon(config.volume, config.is_mute);
        self.scale_volume.set_sensitive(!config.is_mute);
        self.spin_blur.set_sensitive(config.is_static_wallpaper);
        self.play_button.set_image(Some(&icon_image(
            if paused_by_user {
                "media-playback-start-symbolic"
            } else {
                "media-playback-pause-symbolic"
            },
            16,
        )));
        set_a11y_name(
            &self.play_button,
            &if paused_by_user {
                tr("Resume wallpaper")
            } else {
                tr("Pause wallpaper")
            },
        );

        let source = util::primary_source(config, monitors);
        let has_source = !matches!(config.mode, Mode::Null | Mode::Unknown) && !source.is_empty();
        let status = if !has_source {
            tr("Ready")
        } else if paused_by_user || !is_playing {
            tr("Paused")
        } else {
            tr("Playing")
        };
        let state_class = if !has_source || paused_by_user || !is_playing {
            "paused"
        } else {
            "playing"
        };
        set_dot_state(&self.status_dot, state_class);
        set_dot_state(&self.now_dot, state_class);
        self.now_status.set_text(&status);
        self.status_text.set_text(&if has_source {
            format!("{} · {}", status, mode_text(config.mode))
        } else {
            status
        });

        self.now_mode.set_text(&if has_source {
            mode_text(config.mode)
        } else {
            tr("No wallpaper set")
        });
        self.now_source.set_text(&source_display(source));
        let source_tooltip = if source.is_empty() {
            tr("No wallpaper set")
        } else {
            source.to_string()
        };
        self.now_source.set_tooltip_text(Some(&source_tooltip));
        self.now_monitor.set_text(&monitor_text(monitors));

        self.syncing.set(false);
    }

    fn focus_active_page(&self) {
        let target = match self
            .stack
            .visible_child_name()
            .as_ref()
            .map(|name| name.as_str())
        {
            Some("stream") => &self.stream_page.entry,
            Some("webpage") => &self.web_entry,
            Some("settings") => return,
            _ => &self.video_search,
        };
        target.grab_focus();
    }

    /// Toggle the streaming page's progress state. The command loop calls this
    /// around yt-dlp resolution so the UI reflects real work.
    pub fn set_streaming(&self, busy: bool) {
        self.stream_page.entry.set_sensitive(!busy);
        if busy {
            self.stream_page.status.set_text(&tr("Resolving stream…"));
            self.stream_page.spinner.start();
            self.stream_page.status_box.show();
            set_dot_state(&self.status_dot, "busy");
            self.status_text.set_text(&tr("Resolving stream…"));
            set_dot_state(&self.now_dot, "busy");
            self.now_status.set_text(&tr("Resolving stream…"));
        } else {
            self.stream_page.spinner.stop();
            self.stream_page.status_box.hide();
            self.stream_page.entry.set_sensitive(true);
        }
    }

    fn update_mute_icon(&self, volume: i64, muted: bool) {
        let icon = if volume == 0 || muted {
            "audio-volume-muted-symbolic"
        } else if volume < 30 {
            "audio-volume-low-symbolic"
        } else if volume < 60 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        self.toggle_mute.set_image(Some(&icon_image(icon, 16)));
        self.volume_button.set_image(Some(&icon_image(icon, 16)));
        let mute_label = if muted { tr("Unmute") } else { tr("Mute") };
        self.toggle_mute.set_tooltip_text(Some(&mute_label));
        set_a11y_name(&self.toggle_mute, &mute_label);
        set_a11y_name(&self.volume_button, &tr("Volume"));
    }

    pub fn show_error(&self, message: &str) {
        show_message(
            Some(self.window.upcast_ref::<gtk::Window>()),
            gtk::MessageType::Error,
            &tr("Oops!"),
            message,
        );
    }

    pub fn show_welcome(&self) {
        show_message(
            Some(self.window.upcast_ref::<gtk::Window>()),
            gtk::MessageType::Info,
            &tr("Welcome to Ferramari 🤗"),
            &tr("Quickstart for adding local videos:\n\
                 ・Click the folder icon to open the Ferramari folder\n\
                 ・Put your videos there\n\
                 ・Click the refresh button"),
        );
    }
}

fn build_header(
    tx: &crate::app::CommandSender,
    volume: &VolumeParts,
    stack: &gtk::Stack,
) -> HeaderParts {
    let header = gtk::HeaderBar::new();
    header.style_context().add_class("ferramari-header");
    header.set_show_close_button(true);
    header.set_title(Some("Ferramari"));

    let brand_mark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    brand_mark.style_context().add_class("brand-mark");
    brand_mark.set_valign(gtk::Align::Center);
    brand_mark.pack_start(&brand_mark_widget(), true, true, 0);

    let brand_text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    brand_text.set_valign(gtk::Align::Center);
    let brand_title = label("Ferramari", "brand-title");
    let brand_subtitle = label(&tr("Video wallpaper"), "brand-subtitle");
    brand_subtitle.set_visible(true);
    brand_text.pack_start(&brand_title, false, false, 0);
    brand_text.pack_start(&brand_subtitle, false, false, 0);

    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    brand.set_valign(gtk::Align::Center);
    brand.pack_start(&brand_mark, false, false, 0);
    brand.pack_start(&brand_text, false, false, 0);
    header.pack_start(&brand);

    let status_dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status_dot.style_context().add_class("status-dot");
    status_dot.set_size_request(8, 8);
    status_dot.set_valign(gtk::Align::Center);
    let status_text = label(&tr("Ready"), "status-text");
    status_text.set_xalign(0.0);
    let status_chip = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    status_chip.style_context().add_class("status-chip");
    status_chip.set_valign(gtk::Align::Center);
    status_chip.pack_start(&status_dot, false, false, 0);
    status_chip.pack_start(&status_text, false, false, 0);
    header.set_custom_title(Some(&status_chip));

    let play_button = gtk::Button::new();
    play_button.style_context().add_class("transport");
    play_button.set_valign(gtk::Align::Center);
    play_button.set_image(Some(&icon_image("media-playback-pause-symbolic", 16)));
    play_button.set_tooltip_text(Some(&tr("Toggle Play/Pause")));
    set_a11y_name(&play_button, &tr("Pause wallpaper"));

    let lucky_button = icon_button("media-playlist-shuffle-symbolic", &tr("I'm Feeling Lucky"));
    let volume_button = volume.button.clone();
    volume_button.set_valign(gtk::Align::Center);
    volume_button.style_context().add_class("icon-button");

    let settings_button = gtk::Button::new();
    settings_button.style_context().add_class("icon-button");
    settings_button.set_valign(gtk::Align::Center);
    settings_button.set_image(Some(&icon_image("preferences-system-symbolic", 16)));
    settings_button.set_tooltip_text(Some(&tr("Settings")));
    set_a11y_name(&settings_button, &tr("Settings"));
    {
        let stack = stack.clone();
        settings_button.connect_clicked(move |_| stack.set_visible_child_name("settings"));
    }

    header.pack_end(&settings_button);
    header.pack_end(&volume_button);
    header.pack_end(&lucky_button);
    header.pack_end(&play_button);

    {
        let tx = tx.clone();
        lucky_button.connect_clicked(move |_| {
            let _ = tx.send(Command::FeelingLucky);
        });
    }

    status_chip.show_all();
    status_chip.set_no_show_all(true);
    brand_subtitle.show_all();
    brand_subtitle.set_no_show_all(true);

    HeaderParts {
        header,
        play_button,
        status_chip,
        status_dot,
        status_text,
        brand_subtitle,
    }
}

fn brand_mark_widget() -> gtk::Widget {
    if let Some(theme) = gtk::IconTheme::default() {
        if let Ok(Some(pixbuf)) = theme.load_icon(paths::APP_ID, 26, gtk::IconLookupFlags::empty())
        {
            let image = gtk::Image::from_pixbuf(Some(&pixbuf));
            image.set_pixel_size(26);
            return image.upcast();
        }
    }

    let area = gtk::DrawingArea::new();
    area.set_size_request(24, 24);
    area.connect_draw(|area, cr| {
        let width = area.allocated_width() as f64;
        let height = area.allocated_height() as f64;
        let cx = width / 2.0;
        let cy = height / 2.0;
        let radius = width.min(height) * 0.24;
        cr.set_source_rgba(0.95, 0.78, 0.42, 0.95);
        cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        cr.set_line_width(1.7);
        cr.set_source_rgba(0.95, 0.78, 0.42, 0.75);
        for index in 0..8 {
            let angle = index as f64 * std::f64::consts::TAU / 8.0;
            let inner = radius + 3.0;
            let outer = radius + 6.5;
            cr.move_to(cx + angle.cos() * inner, cy + angle.sin() * inner);
            cr.line_to(cx + angle.cos() * outer, cy + angle.sin() * outer);
        }
        let _ = cr.stroke();
        glib::Propagation::Proceed
    });
    area.upcast()
}

fn build_volume_popover(tx: &crate::app::CommandSender, syncing: &Rc<Cell<bool>>) -> VolumeParts {
    let popover = gtk::Popover::builder().width_request(304).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_start(18);
    content.set_margin_end(18);
    content.set_margin_top(16);
    content.set_margin_bottom(16);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = label(&tr("Volume"), "popover-title");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    let label = label("50%", "value-pill");
    header.pack_start(&title, true, true, 0);
    header.pack_end(&label, false, false, 0);
    content.pack_start(&header, false, false, 0);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.set_tooltip_text(Some(&tr("Volume")));
    set_a11y_name(&scale, &tr("Volume"));

    let toggle = gtk::ToggleButton::new();
    toggle.style_context().add_class("icon-button");
    toggle.set_tooltip_text(Some(&tr("Mute")));
    toggle.set_image(Some(&icon_image("audio-volume-high-symbolic", 16)));
    toggle.set_always_show_image(true);
    set_a11y_name(&toggle, &tr("Mute"));

    row.pack_start(&scale, true, true, 0);
    row.pack_end(&toggle, false, false, 0);
    content.pack_start(&row, false, false, 0);
    popover.add(&content);
    content.show_all();

    let button = gtk::MenuButton::new();
    button.set_tooltip_text(Some(&tr("Volume")));
    button.set_image(Some(&icon_image("audio-volume-high-symbolic", 16)));
    button.set_popover(Some(&popover));
    set_a11y_name(&button, &tr("Volume"));

    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        let label = label.clone();
        scale.connect_value_changed(move |scale| {
            label.set_text(&format!("{}%", scale.value() as i32));
            if syncing.get() {
                return;
            }
            let _ = tx.send(Command::SetVolume(scale.value() as i64));
        });
    }
    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        toggle.connect_toggled(move |button| {
            if syncing.get() {
                return;
            }
            let _ = tx.send(Command::SetMute(button.is_active()));
        });
    }

    VolumeParts {
        button,
        scale,
        toggle,
        label,
    }
}

fn build_settings_page(
    window: &gtk::ApplicationWindow,
    tx: &crate::app::CommandSender,
    syncing: &Rc<Cell<bool>>,
    version: &str,
) -> SettingsPage {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_start(26);
    content.set_margin_end(26);
    content.set_margin_top(26);
    content.set_margin_bottom(26);

    content.pack_start(
        &page_heading(
            &tr("Settings"),
            &tr("Playback, wallpaper and system behaviour"),
        ),
        false,
        false,
        0,
    );

    let playback_label = section_label(&tr("Playback"));
    content.pack_start(&playback_label, false, false, 0);

    let switch_pause_max = gtk::Switch::new();
    switch_pause_max.set_valign(gtk::Align::Center);
    let row_pause_max = switch_row(
        &tr("Pause when maximized window"),
        &tr("Automatically pauses playback when maximized window or full screen mode is detected"),
        &switch_pause_max,
    );
    content.pack_start(&row_pause_max, false, false, 0);

    let switch_mute_max = gtk::Switch::new();
    switch_mute_max.set_valign(gtk::Align::Center);
    let row_mute_max = switch_row(
        &tr("Mute when maximized window"),
        &tr("Automatically mutes playback when maximized window or full screen mode is detected"),
        &switch_mute_max,
    );
    content.pack_start(&row_mute_max, false, false, 0);

    let playback_separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    content.pack_start(&playback_separator, false, false, 0);
    let wallpaper_label = section_label(&tr("Wallpaper"));
    content.pack_start(&wallpaper_label, false, false, 0);

    let switch_static = gtk::Switch::new();
    switch_static.set_valign(gtk::Align::Center);
    let row_static = switch_row(
        &tr("Apply static wallpaper"),
        &tr("Video frame can be applied as system wallpaper, look great in GNOME"),
        &switch_static,
    );
    content.pack_start(&row_static, false, false, 0);

    let row_blur = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row_blur.style_context().add_class("setting-row");
    let blur_text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let blur_title = label(&tr("Static wallpaper blur radius"), "setting-title");
    let blur_hint = label("0 – 20", "setting-description");
    blur_text.pack_start(&blur_title, false, false, 0);
    blur_text.pack_start(&blur_hint, false, false, 0);
    let spin_blur = gtk::SpinButton::with_range(0.0, 20.0, 1.0);
    spin_blur.set_numeric(true);
    spin_blur.set_valign(gtk::Align::Center);
    spin_blur.set_tooltip_text(Some(&tr("Static wallpaper blur radius")));
    set_a11y_name(&spin_blur, &tr("Static wallpaper blur radius"));
    row_blur.pack_start(&blur_text, true, true, 0);
    row_blur.pack_end(&spin_blur, false, false, 0);
    content.pack_start(&row_blur, false, false, 0);

    let wallpaper_separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    content.pack_start(&wallpaper_separator, false, false, 0);

    content.pack_start(&section_label(&tr("Rotation")), false, false, 0);
    let rotation_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    rotation_row.style_context().add_class("setting-row");
    let rotation_text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    rotation_text.pack_start(
        &label(&tr("Rotate wallpapers"), "setting-title"),
        false,
        false,
        0,
    );
    rotation_text.pack_start(
        &label(
            &tr("Use the collection assigned to each monitor on Home"),
            "setting-description",
        ),
        false,
        false,
        0,
    );
    let rotation_mode = gtk::ComboBoxText::new();
    rotation_mode.append(Some("off"), &tr("Off"));
    rotation_mode.append(Some("order"), &tr("In order"));
    rotation_mode.append(Some("random"), &tr("Random"));
    rotation_mode.set_active_id(Some("off"));
    rotation_mode.set_valign(gtk::Align::Center);
    rotation_mode.set_size_request(150, -1);
    set_a11y_name(&rotation_mode, &tr("Rotation mode"));
    rotation_row.pack_start(&rotation_text, true, true, 0);
    rotation_row.pack_end(&rotation_mode, false, false, 0);
    content.pack_start(&rotation_row, false, false, 0);

    let interval_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    interval_row.style_context().add_class("setting-row");
    let interval_text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    interval_text.pack_start(
        &label(&tr("Rotation interval"), "setting-title"),
        false,
        false,
        0,
    );
    interval_text.pack_start(
        &label(&tr("Minutes between changes"), "setting-description"),
        false,
        false,
        0,
    );
    let rotation_interval = gtk::SpinButton::with_range(1.0, 1440.0, 1.0);
    rotation_interval.set_numeric(true);
    rotation_interval.set_value(5.0);
    rotation_interval.set_valign(gtk::Align::Center);
    set_a11y_name(&rotation_interval, &tr("Rotation interval"));
    interval_row.pack_start(&interval_text, true, true, 0);
    interval_row.pack_end(&rotation_interval, false, false, 0);
    content.pack_start(&interval_row, false, false, 0);

    let performance_separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    content.pack_start(&performance_separator, false, false, 0);
    content.pack_start(&section_label(&tr("Performance")), false, false, 0);

    let profile_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    profile_row.style_context().add_class("setting-row");
    let profile_text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    profile_text.pack_start(
        &label(&tr("Playback profile"), "setting-title"),
        false,
        false,
        0,
    );
    profile_text.pack_start(
        &label(
            &tr("Performance prioritises smoothness; Power saving reduces decoding and scaling"),
            "setting-description",
        ),
        false,
        false,
        0,
    );
    let profile = gtk::ComboBoxText::new();
    profile.append(Some("performance"), &tr("Performance"));
    profile.append(Some("balanced"), &tr("Balanced"));
    profile.append(Some("power"), &tr("Power saving"));
    profile.set_active_id(Some("balanced"));
    profile.set_valign(gtk::Align::Center);
    profile.set_size_request(150, -1);
    set_a11y_name(&profile, &tr("Playback profile"));
    profile_row.pack_start(&profile_text, true, true, 0);
    profile_row.pack_end(&profile, false, false, 0);
    content.pack_start(&profile_row, false, false, 0);

    let switch_pause_battery = gtk::Switch::new();
    switch_pause_battery.set_valign(gtk::Align::Center);
    let row_pause_battery = switch_row(
        &tr("Pause on battery"),
        &tr("Stop playback when running on battery"),
        &switch_pause_battery,
    );
    content.pack_start(&row_pause_battery, false, false, 0);
    let battery_status = label("", "field-hint");
    battery_status.set_margin_start(4);
    content.pack_start(&battery_status, false, false, 0);

    let system_separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    content.pack_start(&system_separator, false, false, 0);
    content.pack_start(&section_label(&tr("System")), false, false, 0);

    let row_theme = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row_theme.style_context().add_class("setting-row");
    let theme_text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    theme_text.pack_start(&label(&tr("Theme"), "setting-title"), false, false, 0);
    theme_text.pack_start(
        &label(&tr("Window and control appearance"), "setting-description"),
        false,
        false,
        0,
    );
    let combo_theme = gtk::ComboBoxText::new();
    combo_theme.append(Some("system"), &tr("System default"));
    combo_theme.append(Some("light"), &tr("Light"));
    combo_theme.append(Some("dark"), &tr("Dark"));
    combo_theme.set_active_id(Some("system"));
    combo_theme.set_valign(gtk::Align::Center);
    set_a11y_name(&combo_theme, &tr("Theme"));
    row_theme.pack_start(&theme_text, true, true, 0);
    row_theme.pack_end(&combo_theme, false, false, 0);
    content.pack_start(&row_theme, false, false, 0);

    let row_language = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row_language.style_context().add_class("setting-row");
    let language_text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let language_title = label(&tr("Language"), "setting-title");
    let language_hint = label(&tr("Interface language"), "setting-description");
    language_text.pack_start(&language_title, false, false, 0);
    language_text.pack_start(&language_hint, false, false, 0);
    let combo_language = gtk::ComboBoxText::new();
    combo_language.append(Some("system"), &tr("System default"));
    combo_language.append(Some("en"), "English");
    combo_language.append(Some("ru"), "Русский");
    combo_language.append(Some("ja"), "日本語");
    combo_language.set_active_id(Some("system"));
    combo_language.set_valign(gtk::Align::Center);
    set_a11y_name(&combo_language, &tr("Language"));
    row_language.pack_start(&language_text, true, true, 0);
    row_language.pack_end(&combo_language, false, false, 0);
    content.pack_start(&row_language, false, false, 0);

    let switch_autostart = gtk::Switch::new();
    switch_autostart.set_valign(gtk::Align::Center);
    let row_autostart = switch_row(&tr("Autostart after login"), "", &switch_autostart);
    content.pack_start(&row_autostart, false, false, 0);

    content.pack_start(
        &gtk::Separator::new(gtk::Orientation::Horizontal),
        false,
        false,
        0,
    );
    content.pack_start(&section_label(&tr("Monitors")), false, false, 0);
    let monitors_summary = label("", "field-hint");
    monitors_summary.set_line_wrap(true);
    monitors_summary.set_max_width_chars(64);
    content.pack_start(&monitors_summary, false, false, 0);

    content.pack_start(
        &gtk::Separator::new(gtk::Orientation::Horizontal),
        false,
        false,
        0,
    );

    let config_button = menu_action_button("preferences-system-symbolic", &tr("Open config file"));
    let about_button = menu_action_button("help-about-symbolic", &tr("About Ferramari"));
    let quit_button = menu_action_button("application-exit-symbolic", &tr("Quit Ferramari"));
    content.pack_start(&config_button, false, false, 0);
    content.pack_start(&about_button, false, false, 0);
    content.pack_start(&quit_button, false, false, 0);

    let root = page_scroller(&content);

    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        switch_pause_max.connect_active_notify(move |switch| {
            if !syncing.get() {
                let _ = tx.send(Command::SetPauseWhenMaximized(switch.is_active()));
            }
        });
    }
    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        switch_mute_max.connect_active_notify(move |switch| {
            if !syncing.get() {
                let _ = tx.send(Command::SetMuteWhenMaximized(switch.is_active()));
            }
        });
    }
    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        switch_static.connect_active_notify(move |switch| {
            if !syncing.get() {
                let _ = tx.send(Command::SetStaticWallpaper(switch.is_active()));
            }
        });
    }
    {
        let spin = spin_blur.clone();
        switch_static.connect_active_notify(move |switch| {
            spin.set_sensitive(switch.is_active());
        });
    }
    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        switch_autostart.connect_active_notify(move |switch| {
            if !syncing.get() {
                let _ = tx.send(Command::SetAutostart(switch.is_active()));
            }
        });
    }
    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        rotation_mode.connect_changed(move |combo| {
            if syncing.get() {
                return;
            }
            let mode = combo
                .active_id()
                .map(|id| id.to_string())
                .unwrap_or_else(|| "off".to_string());
            let _ = tx.send(Command::SetRotationMode(mode));
        });
    }
    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        rotation_interval.connect_value_changed(move |spin| {
            if !syncing.get() {
                let _ = tx.send(Command::SetRotationInterval((spin.value() * 60.0) as i64));
            }
        });
    }
    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        profile.connect_changed(move |combo| {
            if syncing.get() {
                return;
            }
            let profile = combo
                .active_id()
                .map(|id| id.to_string())
                .unwrap_or_else(|| "balanced".to_string());
            let _ = tx.send(Command::SetPlaybackProfile(profile));
        });
    }
    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        switch_pause_battery.connect_active_notify(move |switch| {
            if !syncing.get() {
                let _ = tx.send(Command::SetPauseOnBattery(switch.is_active()));
            }
        });
    }
    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        combo_theme.connect_changed(move |combo| {
            if syncing.get() {
                return;
            }
            let theme = combo
                .active_id()
                .map(|id| id.to_string())
                .unwrap_or_else(|| "system".to_string());
            let _ = tx.send(Command::SetTheme(theme));
        });
    }
    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        let window = window.clone();
        combo_language.connect_changed(move |combo| {
            if syncing.get() {
                return;
            }
            let language = combo
                .active_id()
                .map(|id| id.to_string())
                .unwrap_or_else(|| "system".to_string());
            let _ = tx.send(Command::SetLanguage(language));
            show_message(
                Some(window.upcast_ref()),
                gtk::MessageType::Info,
                &tr("Language"),
                &tr("Restart Ferramari to apply the language."),
            );
        });
    }
    {
        let tx = tx.clone();
        let syncing = Rc::clone(syncing);
        spin_blur.connect_value_changed(move |spin| {
            if !syncing.get() {
                let _ = tx.send(Command::SetBlurRadius(spin.value() as i64));
            }
        });
    }

    config_button.connect_clicked(|_| xdg_open(&paths::config_path()));
    {
        let tx = tx.clone();
        quit_button.connect_clicked(move |_| {
            let _ = tx.send(Command::Quit);
        });
    }
    {
        let window = window.clone();
        let version = version.to_string();
        about_button.connect_clicked(move |_| {
            show_about(window.upcast_ref::<gtk::Window>(), &version);
        });
    }

    if !paths::is_gnome() {
        for widget in [
            wallpaper_label.upcast_ref::<gtk::Widget>(),
            row_static.upcast_ref::<gtk::Widget>(),
            row_blur.upcast_ref::<gtk::Widget>(),
            wallpaper_separator.upcast_ref::<gtk::Widget>(),
        ] {
            widget.set_no_show_all(true);
            widget.hide();
        }
    }
    if paths::is_wayland() {
        for widget in [
            playback_label.upcast_ref::<gtk::Widget>(),
            row_pause_max.upcast_ref::<gtk::Widget>(),
            row_mute_max.upcast_ref::<gtk::Widget>(),
            playback_separator.upcast_ref::<gtk::Widget>(),
        ] {
            widget.set_no_show_all(true);
            widget.hide();
        }
    }

    SettingsPage {
        root,
        monitors_summary,
        combo_theme,
        combo_language,
        rotation_mode,
        rotation_interval,
        profile,
        switch_pause_battery,
        battery_status,
        switch_autostart,
        switch_static,
        switch_pause_max,
        switch_mute_max,
        spin_blur,
    }
}

fn build_video_page() -> VideoPage {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
    root.set_margin_start(26);
    root.set_margin_end(26);
    root.set_margin_top(26);
    root.set_margin_bottom(26);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let title = label(&tr("Local Video"), "page-title");
    let count = label("", "count-pill");
    count.set_valign(gtk::Align::Center);
    title_row.pack_start(&title, false, false, 0);
    title_row.pack_start(&count, false, false, 0);
    let subtitle = label(&tr("Videos located in Ferramari folder"), "page-subtitle");
    subtitle.set_ellipsize(pango::EllipsizeMode::End);
    titles.pack_start(&title_row, false, false, 0);
    titles.pack_start(&subtitle, false, false, 0);
    titles.set_hexpand(true);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_valign(gtk::Align::Center);
    let folder_button = labeled_icon_button(
        "folder-open-symbolic",
        &tr("Open folder"),
        &tr("Open Ferramari folder"),
    );
    let refresh_button =
        labeled_icon_button("view-refresh-symbolic", &tr("Refresh"), &tr("Refresh"));
    let new_collection = labeled_icon_button(
        "list-add-symbolic",
        &tr("New collection"),
        &tr("Create a collection"),
    );
    actions.pack_start(&new_collection, false, false, 0);
    actions.pack_start(&folder_button, false, false, 0);
    actions.pack_start(&refresh_button, false, false, 0);
    header.pack_start(&titles, true, true, 0);
    header.pack_end(&actions, false, false, 0);
    root.pack_start(&header, false, false, 0);

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    toolbar.style_context().add_class("glass-toolbar");
    toolbar.set_margin_start(12);
    toolbar.set_margin_end(12);
    toolbar.set_margin_top(11);
    toolbar.set_margin_bottom(11);
    let search = gtk::Entry::new();
    search.style_context().add_class("search-entry");
    search.set_placeholder_text(Some(&tr("Search videos")));
    search.set_input_purpose(gtk::InputPurpose::FreeForm);
    search.set_hexpand(true);
    search.set_icon_from_icon_name(gtk::EntryIconPosition::Primary, Some("edit-find-symbolic"));
    search.set_icon_tooltip_text(gtk::EntryIconPosition::Primary, Some(&tr("Search videos")));
    set_a11y_name(&search, &tr("Search videos"));
    toolbar.pack_start(&search, true, true, 0);

    let view = gtk::ComboBoxText::new();
    view.append(Some("all"), &tr("All videos"));
    view.append(Some("favorites"), &tr("Favorites"));
    view.append(Some("recent"), &tr("Recent"));
    view.set_active_id(Some("all"));
    view.set_size_request(160, -1);
    view.set_tooltip_text(Some(&tr("Library view")));
    set_a11y_name(&view, &tr("Library view"));
    toolbar.pack_start(&view, false, false, 0);

    let rename_collection = gtk::Button::with_label(&tr("Rename"));
    rename_collection.style_context().add_class("secondary");
    rename_collection.hide();
    rename_collection.set_no_show_all(true);
    let delete_collection = gtk::Button::with_label(&tr("Delete"));
    delete_collection.style_context().add_class("secondary");
    delete_collection.hide();
    delete_collection.set_no_show_all(true);
    toolbar.pack_start(&rename_collection, false, false, 0);
    toolbar.pack_start(&delete_collection, false, false, 0);
    root.pack_start(&toolbar, false, false, 0);

    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::Single);
    flow.set_homogeneous(true);
    flow.set_min_children_per_line(1);
    flow.set_max_children_per_line(6);
    flow.set_row_spacing(16);
    flow.set_column_spacing(16);
    flow.set_valign(gtk::Align::Start);
    flow.set_halign(gtk::Align::Fill);
    flow.style_context().add_class("video-grid");
    set_a11y_name(&flow, &tr("Video library"));

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .shadow_type(gtk::ShadowType::None)
        .vexpand(true)
        .build();
    scroller.add(&flow);

    let empty = gtk::Box::new(gtk::Orientation::Vertical, 10);
    empty.style_context().add_class("empty-state");
    empty.set_halign(gtk::Align::Center);
    empty.set_valign(gtk::Align::Center);
    empty.set_vexpand(true);
    let empty_icon = icon_image("video-x-generic-symbolic", 44);
    empty_icon.style_context().add_class("empty-icon");
    empty_icon.set_halign(gtk::Align::Center);
    let empty_title = label("", "empty-title");
    empty_title.set_halign(gtk::Align::Center);
    let empty_body = label("", "empty-body");
    empty_body.set_halign(gtk::Align::Center);
    empty_body.set_justify(gtk::Justification::Center);
    empty_body.set_line_wrap(true);
    empty_body.set_max_width_chars(44);
    let empty_folder = labeled_icon_button(
        "folder-open-symbolic",
        &tr("Open folder"),
        &tr("Open Ferramari folder"),
    );
    empty_folder.set_halign(gtk::Align::Center);
    empty_folder.set_margin_top(8);
    let empty_clear = labeled_icon_button(
        "edit-clear-symbolic",
        &tr("Clear search"),
        &tr("Clear search"),
    );
    empty_clear.set_halign(gtk::Align::Center);
    empty_clear.set_margin_top(8);
    empty_clear.hide();
    empty_folder.show();
    empty_folder.set_no_show_all(true);
    empty_clear.set_no_show_all(true);
    empty.pack_start(&empty_icon, false, false, 0);
    empty.pack_start(&empty_title, false, false, 0);
    empty.pack_start(&empty_body, false, false, 0);
    empty.pack_start(&empty_folder, false, false, 0);
    empty.pack_start(&empty_clear, false, false, 0);

    let library_stack = gtk::Stack::new();
    library_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    library_stack.set_transition_duration(180);
    library_stack.set_vexpand(true);
    library_stack.add_named(&scroller, "grid");
    library_stack.add_named(&empty, "empty");
    root.pack_start(&library_stack, true, true, 0);

    let selection_bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    selection_bar.style_context().add_class("selection-bar");
    selection_bar.set_margin_start(12);
    selection_bar.set_margin_end(12);
    selection_bar.set_margin_top(10);
    selection_bar.set_margin_bottom(11);
    let selection_label = label(&tr("Select a video to apply"), "field-hint");
    selection_label.set_xalign(0.0);
    selection_label.set_ellipsize(pango::EllipsizeMode::Middle);
    selection_label.set_hexpand(true);
    let apply_button = primary_button(&tr("Apply"));
    apply_button.set_sensitive(false);
    apply_button.set_tooltip_text(Some(&tr("Apply to wallpaper")));
    let add_to_collection = gtk::Button::with_label(&tr("Add to collection"));
    add_to_collection.style_context().add_class("secondary");
    add_to_collection.set_sensitive(false);
    let remove_from_collection = gtk::Button::with_label(&tr("Remove from collection"));
    remove_from_collection
        .style_context()
        .add_class("secondary");
    remove_from_collection.set_sensitive(false);
    remove_from_collection.hide();
    remove_from_collection.set_no_show_all(true);
    selection_bar.pack_start(&selection_label, true, true, 0);
    selection_bar.pack_end(&apply_button, false, false, 0);
    selection_bar.pack_end(&remove_from_collection, false, false, 0);
    selection_bar.pack_end(&add_to_collection, false, false, 0);
    root.pack_start(&selection_bar, false, false, 0);

    folder_button.connect_clicked(|_| xdg_open(&paths::video_wallpaper_dir()));

    VideoPage {
        root,
        refresh: refresh_button,
        flow,
        library_stack,
        empty_title,
        empty_body,
        count,
        selection_label,
        apply_button,
        empty_folder,
        empty_clear,
        search,
        view,
        view_syncing: Rc::new(Cell::new(false)),
        new_collection,
        rename_collection,
        delete_collection,
        add_to_collection,
        remove_from_collection,
    }
}

fn view_from_page(page: &VideoPage) -> LibraryView {
    match page.view.active_id().as_deref() {
        Some("favorites") => LibraryView::Favorites,
        Some("recent") => LibraryView::Recent,
        Some(id) if id.starts_with("collection:") => {
            LibraryView::Collection(id.trim_start_matches("collection:").to_string())
        }
        _ => LibraryView::All,
    }
}

fn videos_for_view(view: &LibraryView, library: &Library) -> Vec<PathBuf> {
    let all = || util::video_paths();
    match view {
        LibraryView::All => all(),
        LibraryView::Favorites => all()
            .into_iter()
            .filter(|path| library.is_favorite(&path.to_string_lossy()))
            .collect(),
        LibraryView::Recent => library
            .history()
            .iter()
            .filter_map(|entry| {
                let path = PathBuf::from(&entry.path);
                (path.is_file() && util::is_video(&path)).then_some(path)
            })
            .collect(),
        LibraryView::Collection(id) => library
            .collection(id)
            .map(|collection| {
                collection
                    .items
                    .iter()
                    .filter_map(|item| {
                        let path = PathBuf::from(item);
                        (path.is_file() && util::is_video(&path)).then_some(path)
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn refresh_collection_choices(page: &VideoPage, library: &Library) {
    let active = page
        .view
        .active_id()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "all".to_string());
    page.view_syncing.set(true);
    page.view.remove_all();
    page.view.append(Some("all"), &tr("All videos"));
    page.view.append(Some("favorites"), &tr("Favorites"));
    page.view.append(Some("recent"), &tr("Recent"));
    for collection in library.collections() {
        let id = format!("collection:{}", collection.id);
        page.view.append(Some(&id), &collection.name);
    }
    if !page.view.set_active_id(Some(&active)) {
        page.view.set_active_id(Some("all"));
    }
    page.view_syncing.set(false);
}

fn update_view_actions(page: &VideoPage, view: &LibraryView) {
    let collection = matches!(view, LibraryView::Collection(_));
    page.rename_collection.set_visible(collection);
    page.delete_collection.set_visible(collection);
    page.remove_from_collection.set_visible(collection);
}

fn set_star(button: &gtk::ToggleButton, active: bool) {
    button.set_active(active);
    button.set_image(Some(&icon_image(
        if active {
            "starred-symbolic"
        } else {
            "non-starred-symbolic"
        },
        16,
    )));
    button.set_tooltip_text(Some(&tr(if active {
        "Remove from favorites"
    } else {
        "Add to favorites"
    })));
}

fn prompt_text(
    parent: Option<&gtk::Window>,
    title: &str,
    label_text: &str,
    initial: &str,
) -> Option<String> {
    let dialog = gtk::Dialog::with_buttons(
        Some(title),
        parent,
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[
            (&tr("Cancel"), gtk::ResponseType::Cancel),
            (&tr("Save"), gtk::ResponseType::Ok),
        ],
    );
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);
    entry.set_margin_start(16);
    entry.set_margin_end(16);
    entry.set_margin_top(8);
    entry.set_margin_bottom(8);
    dialog.content_area().pack_start(&entry, false, false, 0);
    let hint = label(label_text, "field-hint");
    hint.set_margin_start(16);
    hint.set_margin_end(16);
    hint.set_margin_bottom(8);
    dialog.content_area().pack_start(&hint, false, false, 0);
    if let Some(button) = dialog.widget_for_response(gtk::ResponseType::Ok) {
        button.style_context().add_class("primary");
        dialog.set_default_response(gtk::ResponseType::Ok);
    }
    dialog.show_all();
    let response = dialog.run();
    let text = entry.text().trim().to_string();
    dialog.close();
    (response == gtk::ResponseType::Ok && !text.is_empty()).then_some(text)
}

fn confirm_dialog(parent: Option<&gtk::Window>, title: &str, text: &str) -> bool {
    let dialog = gtk::MessageDialog::new(
        parent,
        gtk::DialogFlags::MODAL,
        gtk::MessageType::Question,
        gtk::ButtonsType::YesNo,
        title,
    );
    dialog.set_secondary_text(Some(text));
    let response = dialog.run();
    dialog.close();
    response == gtk::ResponseType::Yes
}

fn wire_video_page(page: &VideoPage, shared: &Rc<Shared>, library: &Rc<RefCell<Library>>) {
    let query = Rc::new(RefCell::new(String::new()));

    {
        let query_for_filter = Rc::clone(&query);
        let shared = Rc::clone(shared);
        let library = Rc::clone(library);
        let page_for_filter = page.clone();
        page.flow.set_filter_func(Some(Box::new(move |child| {
            let index = child.index();
            if index < 0 {
                return false;
            }
            let videos = shared.videos.borrow();
            let Some(path) = videos.get(index as usize) else {
                return false;
            };
            let path_string = path.to_string_lossy();
            let allowed = match view_from_page(&page_for_filter) {
                LibraryView::All | LibraryView::Recent => true,
                LibraryView::Favorites => library.borrow().is_favorite(&path_string),
                LibraryView::Collection(id) => {
                    library.borrow().collection_contains(&id, &path_string)
                }
            };
            allowed
                && (query_for_filter.borrow().is_empty()
                    || path
                        .file_name()
                        .map(|name| {
                            name.to_string_lossy()
                                .to_lowercase()
                                .contains(&*query_for_filter.borrow())
                        })
                        .unwrap_or(false))
        })));
    }

    {
        let query = Rc::clone(&query);
        let shared = Rc::clone(shared);
        let page_view = page.clone();
        page.search.connect_changed(move |entry| {
            let text = entry.text().trim().to_lowercase();
            *query.borrow_mut() = text.clone();
            page_view.flow.invalidate_filter();
            update_library_state(&shared, &page_view, &text, &view_from_page(&page_view));
            update_selection(&page_view.flow, &shared, &page_view);

            if entry.text().is_empty() {
                entry.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, None);
            } else {
                entry.set_icon_from_icon_name(
                    gtk::EntryIconPosition::Secondary,
                    Some("edit-clear-symbolic"),
                );
                entry.set_icon_tooltip_text(
                    gtk::EntryIconPosition::Secondary,
                    Some(&tr("Clear search")),
                );
            }
        });
    }

    page.search.connect_icon_press(move |entry, position, _| {
        if position == gtk::EntryIconPosition::Secondary {
            entry.set_text("");
        }
    });

    page.empty_folder
        .connect_clicked(|_| xdg_open(&paths::video_wallpaper_dir()));
    {
        let search = page.search.clone();
        page.empty_clear
            .connect_clicked(move |_| search.set_text(""));
    }

    {
        let page_for_selection = page.clone();
        let shared_for_selection = Rc::clone(shared);
        page.flow.connect_selected_children_changed(move |_| {
            update_selection(
                &page_for_selection.flow,
                &shared_for_selection,
                &page_for_selection,
            );
        });
    }

    {
        let shared = Rc::clone(shared);
        let flow = page.flow.clone();
        page.apply_button.connect_clicked(move |button| {
            if let Some(path) = selected_video(&flow, &shared) {
                show_monitor_menu(&shared, button, None, path);
            }
        });
    }

    {
        let shared = Rc::clone(shared);
        let library = Rc::clone(library);
        let page = page.clone();
        let refresh = page.refresh.clone();
        refresh.connect_clicked(move |_| {
            let view = view_from_page(&page);
            fill_grid(&shared, &page, &library, &view);
        });
    }

    {
        let shared = Rc::clone(shared);
        let library = Rc::clone(library);
        let page = page.clone();
        let view_widget = page.view.clone();
        view_widget.connect_changed(move |_| {
            if page.view_syncing.get() {
                return;
            }
            let view = view_from_page(&page);
            update_view_actions(&page, &view);
            fill_grid(&shared, &page, &library, &view);
        });
    }

    {
        let library = Rc::clone(library);
        let shared = Rc::clone(shared);
        let page = page.clone();
        let button_widget = page.new_collection.clone();
        button_widget.connect_clicked(move |button| {
            let parent = button
                .toplevel()
                .and_then(|top| top.downcast::<gtk::Window>().ok());
            let Some(name) = prompt_text(
                parent.as_ref(),
                &tr("New collection"),
                &tr("Collection name"),
                "",
            ) else {
                return;
            };
            let Some(id) = library.borrow_mut().create_collection(&name) else {
                return;
            };
            let _ = library.borrow().save();
            refresh_collection_choices(&page, &library.borrow());
            let _ = shared.tx.send(Command::LibraryChanged);
            page.view.set_active_id(Some(&format!("collection:{id}")));
        });
    }

    {
        let shared = Rc::clone(shared);
        let library = Rc::clone(library);
        let page = page.clone();
        let button_widget = page.rename_collection.clone();
        button_widget.connect_clicked(move |button| {
            let LibraryView::Collection(id) = view_from_page(&page) else {
                return;
            };
            let current = library
                .borrow()
                .collection(&id)
                .map(|collection| collection.name.clone())
                .unwrap_or_default();
            let parent = button
                .toplevel()
                .and_then(|top| top.downcast::<gtk::Window>().ok());
            let Some(name) = prompt_text(
                parent.as_ref(),
                &tr("Rename collection"),
                &tr("Collection name"),
                &current,
            ) else {
                return;
            };
            if library.borrow_mut().rename_collection(&id, &name) {
                let _ = library.borrow().save();
                refresh_collection_choices(&page, &library.borrow());
                let _ = shared.tx.send(Command::LibraryChanged);
                let view = view_from_page(&page);
                fill_grid(&shared, &page, &library, &view);
            }
        });
    }

    {
        let shared = Rc::clone(shared);
        let library = Rc::clone(library);
        let page = page.clone();
        let button_widget = page.delete_collection.clone();
        button_widget.connect_clicked(move |button| {
            let LibraryView::Collection(id) = view_from_page(&page) else {
                return;
            };
            let name = library
                .borrow()
                .collection(&id)
                .map(|collection| collection.name.clone())
                .unwrap_or_default();
            let parent = button
                .toplevel()
                .and_then(|top| top.downcast::<gtk::Window>().ok());
            if !confirm_dialog(
                parent.as_ref(),
                &tr("Delete collection"),
                &format!("{} «{}»?", tr("Delete collection"), name),
            ) {
                return;
            }
            if library.borrow_mut().delete_collection(&id) {
                let _ = library.borrow().save();
                refresh_collection_choices(&page, &library.borrow());
                let _ = shared.tx.send(Command::LibraryChanged);
                page.view.set_active_id(Some("all"));
                let view = view_from_page(&page);
                fill_grid(&shared, &page, &library, &view);
            }
        });
    }

    {
        let shared = Rc::clone(shared);
        let library = Rc::clone(library);
        let page = page.clone();
        let button_widget = page.add_to_collection.clone();
        button_widget.connect_clicked(move |button| {
            let Some(path) = selected_video(&page.flow, &shared) else {
                return;
            };
            let menu = gtk::Menu::new();
            for collection in library.borrow().collections() {
                let item = gtk::MenuItem::with_label(&collection.name);
                let library = Rc::clone(&library);
                let page = page.clone();
                let shared = Rc::clone(&shared);
                let id = collection.id.clone();
                let path = path.clone();
                item.connect_activate(move |_| {
                    if library.borrow_mut().add_to_collection(&id, &path) {
                        let _ = library.borrow().save();
                        let _ = shared.tx.send(Command::LibraryChanged);
                        if view_from_page(&page) == LibraryView::Collection(id.clone()) {
                            fill_grid(
                                &shared,
                                &page,
                                &library,
                                &LibraryView::Collection(id.clone()),
                            );
                        }
                    }
                });
                menu.append(&item);
            }
            if !library.borrow().collections().is_empty() {
                menu.append(&gtk::SeparatorMenuItem::new());
            }
            let new_item = gtk::MenuItem::with_label(&tr("New collection…"));
            {
                let library = Rc::clone(&library);
                let shared = Rc::clone(&shared);
                let page = page.clone();
                let path = path.clone();
                let button = button.clone();
                new_item.connect_activate(move |_| {
                    let parent = button
                        .toplevel()
                        .and_then(|top| top.downcast::<gtk::Window>().ok());
                    let Some(name) = prompt_text(
                        parent.as_ref(),
                        &tr("New collection"),
                        &tr("Collection name"),
                        "",
                    ) else {
                        return;
                    };
                    let Some(id) = library.borrow_mut().create_collection(&name) else {
                        return;
                    };
                    let _ = library.borrow_mut().add_to_collection(&id, &path);
                    let _ = library.borrow().save();
                    refresh_collection_choices(&page, &library.borrow());
                    let _ = shared.tx.send(Command::LibraryChanged);
                });
            }
            menu.append(&new_item);
            menu.show_all();
            *shared.menu.borrow_mut() = Some(menu.clone());
            menu.popup_at_widget(button, gdk::Gravity::South, gdk::Gravity::North, None);
        });
    }

    {
        let shared = Rc::clone(shared);
        let library = Rc::clone(library);
        let page = page.clone();
        let button_widget = page.remove_from_collection.clone();
        button_widget.connect_clicked(move |_| {
            let LibraryView::Collection(id) = view_from_page(&page) else {
                return;
            };
            let Some(path) = selected_video(&page.flow, &shared) else {
                return;
            };
            if library.borrow_mut().remove_from_collection(&id, &path) {
                let _ = library.borrow().save();
                let _ = shared.tx.send(Command::LibraryChanged);
                let view = LibraryView::Collection(id);
                fill_grid(&shared, &page, &library, &view);
            }
        });
    }

    {
        let shared = Rc::clone(shared);
        page.flow.connect_button_press_event(move |flow, event| {
            if event.button() != 3 {
                return glib::Propagation::Proceed;
            }
            let (x, y) = event.position();
            let Some(child) = flow.child_at_pos(x as i32, y as i32) else {
                return glib::Propagation::Proceed;
            };
            flow.grab_focus();
            flow.select_child(&child);
            if let Some(path) = selected_video(flow, &shared) {
                show_monitor_menu(&shared, flow, Some(event), path);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
}

fn build_stream_page(tx: &crate::app::CommandSender) -> StreamPage {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_start(26);
    content.set_margin_end(26);
    content.set_margin_top(26);
    content.set_margin_bottom(26);

    content.pack_start(
        &page_heading(&tr("Streaming"), &tr("Videos from streaming services")),
        false,
        false,
        0,
    );

    let (card, inner) = glass_card(14);
    card.set_halign(gtk::Align::Center);
    card.set_size_request(560, -1);

    let form_title = label(&tr("Streaming URL"), "form-title");
    let form_copy = label(
        &tr("Paste a video link. Ferramari resolves it with yt-dlp and starts playback."),
        "form-copy",
    );
    form_copy.set_line_wrap(true);
    inner.pack_start(&form_title, false, false, 0);
    inner.pack_start(&form_copy, false, false, 0);

    let entry_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let entry = gtk::Entry::new();
    entry.style_context().add_class("search-entry");
    entry.set_placeholder_text(Some("https://www.youtube.com/watch?v=id"));
    entry.set_input_purpose(gtk::InputPurpose::Url);
    entry.set_hexpand(true);
    entry.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("go-next-symbolic"));
    set_a11y_name(&entry, &tr("Streaming URL"));
    let submit = primary_button(&tr("Play"));
    submit.set_image(Some(&icon_image("media-playback-start-symbolic", 16)));
    submit.set_always_show_image(true);
    entry_row.pack_start(&entry, true, true, 0);
    entry_row.pack_end(&submit, false, false, 0);
    inner.pack_start(&entry_row, false, false, 0);

    let error = label("", "danger-text");
    error.set_xalign(0.0);
    error.set_visible(false);
    error.set_no_show_all(true);
    inner.pack_start(&error, false, false, 0);

    let status_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let spinner = gtk::Spinner::new();
    spinner.set_size_request(16, 16);
    spinner.set_valign(gtk::Align::Center);
    let status = label("", "field-hint");
    status.set_xalign(0.0);
    status_box.pack_start(&spinner, false, false, 0);
    status_box.pack_start(&status, false, false, 0);
    status_box.show_all();
    status_box.set_no_show_all(true);
    status_box.hide();
    inner.pack_start(&status_box, false, false, 0);

    let hint = hint_expander(
        &tr("Hint"),
        &tr("Streaming is resolved with yt-dlp."),
        Some("https://github.com/yt-dlp/yt-dlp"),
    );
    inner.pack_start(&hint, false, false, 0);

    let submit_action: Rc<dyn Fn()> = {
        let entry = entry.clone();
        let error = error.clone();
        let tx = tx.clone();
        Rc::new(move || {
            let url = entry.text().trim().to_string();
            if url.is_empty() {
                entry.style_context().add_class("error");
                error.set_text(&tr("Enter a streaming URL first."));
                error.set_visible(true);
                return;
            }
            entry.style_context().remove_class("error");
            error.set_visible(false);
            let _ = tx.send(Command::Stream { url });
        })
    };
    {
        let submit_action = Rc::clone(&submit_action);
        entry.connect_activate(move |_| submit_action());
    }
    {
        let submit_action = Rc::clone(&submit_action);
        entry.connect_icon_press(move |_, position, _| {
            if position == gtk::EntryIconPosition::Secondary {
                submit_action();
            }
        });
    }
    {
        let error = error.clone();
        entry.connect_changed(move |entry| {
            entry.style_context().remove_class("error");
            error.set_visible(false);
        });
    }
    {
        let submit_action = Rc::clone(&submit_action);
        submit.connect_clicked(move |_| submit_action());
    }

    content.pack_start(&card, false, false, 8);
    let scroller = page_scroller(&content);

    StreamPage {
        root: scroller,
        entry,
        spinner,
        status_box,
        status,
    }
}

fn build_web_page(tx: &crate::app::CommandSender) -> WebPage {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_start(26);
    content.set_margin_end(26);
    content.set_margin_top(26);
    content.set_margin_bottom(26);

    content.pack_start(
        &page_heading(&tr("Web Page"), &tr("Set a web page as wallpaper")),
        false,
        false,
        0,
    );

    let (card, inner) = glass_card(14);
    card.set_halign(gtk::Align::Center);
    card.set_size_request(560, -1);
    let title = label(&tr("Web Page URL"), "form-title");
    let copy = label(
        &tr("Any website that renders in WebKitGTK can become your wallpaper."),
        "form-copy",
    );
    copy.set_line_wrap(true);
    inner.pack_start(&title, false, false, 0);
    inner.pack_start(&copy, false, false, 0);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let entry = gtk::Entry::new();
    entry.style_context().add_class("search-entry");
    entry.set_placeholder_text(Some("https://www.google.com/"));
    entry.set_input_purpose(gtk::InputPurpose::Url);
    entry.set_hexpand(true);
    entry.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("go-next-symbolic"));
    set_a11y_name(&entry, &tr("Web Page URL"));
    let submit = primary_button(&tr("Apply"));
    submit.set_image(Some(&icon_image("go-next-symbolic", 16)));
    submit.set_always_show_image(true);
    row.pack_start(&entry, true, true, 0);
    row.pack_end(&submit, false, false, 0);
    inner.pack_start(&row, false, false, 0);

    let error = label("", "danger-text");
    error.set_xalign(0.0);
    error.set_visible(false);
    error.set_no_show_all(true);
    inner.pack_start(&error, false, false, 0);

    let hint = hint_expander(
        &tr("Hint"),
        &tr("Check this website for some really amazing examples!"),
        Some("https://alteredqualia.com/"),
    );
    inner.pack_start(&hint, false, false, 0);

    let send_action: Rc<dyn Fn()> = {
        let entry = entry.clone();
        let error = error.clone();
        let tx = tx.clone();
        Rc::new(move || {
            let url = entry.text().trim().to_string();
            if url.is_empty() {
                entry.style_context().add_class("error");
                error.set_text(&tr("Enter a web page URL first."));
                error.set_visible(true);
                return;
            }
            entry.style_context().remove_class("error");
            error.set_visible(false);
            let _ = tx.send(Command::Webpage { url });
        })
    };
    {
        let send_action = Rc::clone(&send_action);
        entry.connect_activate(move |_| send_action());
    }
    {
        let send_action = Rc::clone(&send_action);
        entry.connect_icon_press(move |_, position, _| {
            if position == gtk::EntryIconPosition::Secondary {
                send_action();
            }
        });
    }
    {
        let error = error.clone();
        entry.connect_changed(move |entry| {
            entry.style_context().remove_class("error");
            error.set_visible(false);
        });
    }
    {
        let send_action = Rc::clone(&send_action);
        submit.connect_clicked(move |_| send_action());
    }

    content.pack_start(&card, false, false, 8);

    let or_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    or_row.set_halign(gtk::Align::Center);
    or_row.set_size_request(560, -1);
    let or_left = gtk::Separator::new(gtk::Orientation::Horizontal);
    let or_right = gtk::Separator::new(gtk::Orientation::Horizontal);
    or_left.set_hexpand(true);
    or_right.set_hexpand(true);
    let or = label(&tr("or"), "field-hint");
    or_row.pack_start(&or_left, true, true, 0);
    or_row.pack_start(&or, false, false, 0);
    or_row.pack_start(&or_right, true, true, 0);
    content.pack_start(&or_row, false, false, 0);

    let (local_card, local_inner) = glass_card(14);
    local_card.set_halign(gtk::Align::Center);
    local_card.set_size_request(560, -1);
    let local_title = label(&tr("Local Web Page"), "form-title");
    let local_copy = label(
        &tr("Alternately, you can choose a HTML file on your PC"),
        "form-copy",
    );
    local_copy.set_line_wrap(true);
    local_inner.pack_start(&local_title, false, false, 0);
    local_inner.pack_start(&local_copy, false, false, 0);

    let chosen: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let chooser_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let chooser_info = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    chooser_info.set_hexpand(true);
    let chooser = labeled_icon_button(
        "document-open-symbolic",
        &tr("Choose a HTML file"),
        &tr("Choose a HTML file"),
    );
    let chosen_label = label(&tr("No file selected"), "field-hint");
    chosen_label.set_ellipsize(pango::EllipsizeMode::Middle);
    chosen_label.set_max_width_chars(34);
    chooser_info.pack_start(&chooser, false, false, 0);
    chooser_info.pack_start(&chosen_label, true, true, 0);

    let apply = primary_button(&tr("Apply"));
    apply.set_image(Some(&icon_image("go-next-symbolic", 16)));
    apply.set_always_show_image(true);
    apply.set_sensitive(false);
    chooser_row.pack_start(&chooser_info, true, true, 0);
    chooser_row.pack_end(&apply, false, false, 0);
    local_inner.pack_start(&chooser_row, false, false, 0);

    let local_error = label("", "danger-text");
    local_error.set_xalign(0.0);
    local_error.set_visible(false);
    local_error.set_no_show_all(true);
    local_inner.pack_start(&local_error, false, false, 0);

    {
        let chosen = Rc::clone(&chosen);
        let chosen_label = chosen_label.clone();
        let apply_for_dialog = apply.clone();
        chooser.connect_clicked(move |button| {
            let parent = button
                .toplevel()
                .and_then(|widget| widget.downcast::<gtk::Window>().ok());
            let dialog = gtk::FileChooserDialog::with_buttons(
                Some(&tr("Choose a HTML file")),
                parent.as_ref(),
                gtk::FileChooserAction::Open,
                &[
                    (&tr("Cancel"), gtk::ResponseType::Cancel),
                    (&tr("Open"), gtk::ResponseType::Accept),
                ],
            );
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("HTML"));
            filter.add_mime_type("text/html");
            dialog.add_filter(filter);
            dialog.set_modal(true);
            dialog.set_default_response(gtk::ResponseType::Accept);
            dialog.set_transient_for(parent.as_ref());
            if let Some(button) = dialog.widget_for_response(gtk::ResponseType::Accept) {
                button.style_context().add_class("primary");
            }
            {
                let chosen = Rc::clone(&chosen);
                let chosen_label = chosen_label.clone();
                let apply = apply_for_dialog.clone();
                dialog.connect_response(move |dialog, response| {
                    if response == gtk::ResponseType::Accept {
                        if let Some(path) = dialog.filename() {
                            let name = path
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.to_string_lossy().into_owned());
                            chosen_label.set_text(&name);
                            chosen_label.set_tooltip_text(Some(&path.to_string_lossy()));
                            *chosen.borrow_mut() = Some(path);
                            apply.set_sensitive(true);
                        }
                    }
                    dialog.close();
                });
            }
            dialog.show_all();
        });
    }
    {
        let tx = tx.clone();
        let chosen = Rc::clone(&chosen);
        apply.connect_clicked(move |_| {
            if let Some(path) = chosen.borrow().as_ref() {
                local_error.set_visible(false);
                let _ = tx.send(Command::Webpage {
                    url: path.to_string_lossy().into_owned(),
                });
                return;
            }
            local_error.set_text(&tr("Choose an HTML file first."));
            local_error.set_visible(true);
        });
    }

    content.pack_start(&local_card, false, false, 0);
    WebPage {
        root: page_scroller(&content),
        entry,
    }
}

fn build_navigation(stack: &gtk::Stack) -> (gtk::Box, gtk::Box) {
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 12);
    sidebar.style_context().add_class("sidebar");
    sidebar.set_width_request(238);

    let browse = section_label(&tr("Browse"));
    browse.set_margin_start(8);
    browse.set_margin_top(6);
    sidebar.pack_start(&browse, false, false, 0);

    let nav_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    sidebar.pack_start(&nav_list, false, false, 0);

    let top_nav = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    top_nav.style_context().add_class("top-nav");
    top_nav.set_margin_bottom(0);
    let top_nav_inner = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    top_nav_inner.set_margin_start(6);
    top_nav_inner.set_margin_end(6);
    top_nav_inner.set_margin_top(6);
    top_nav_inner.set_margin_bottom(6);
    top_nav.pack_start(&top_nav_inner, true, true, 0);

    let specs = [
        ("home", "go-home-symbolic", tr("Home"), tr("Overview")),
        (
            "video",
            "video-x-generic-symbolic",
            tr("Local Video"),
            tr("Your own files"),
        ),
        (
            "online",
            "folder-download-symbolic",
            tr("Wallpapers"),
            tr("Preview before installing"),
        ),
        (
            "stream",
            "network-server-symbolic",
            tr("Streaming"),
            tr("Video links"),
        ),
        (
            "webpage",
            "web-browser-symbolic",
            tr("Web Page"),
            tr("URL or HTML"),
        ),
        (
            "settings",
            "preferences-system-symbolic",
            tr("Settings"),
            tr("App behaviour"),
        ),
    ];

    let mut buttons: Vec<(String, gtk::Button)> = Vec::new();
    for (name, icon, title, description) in specs {
        let sidebar_button = nav_button(icon, &title, Some(&description));
        nav_list.pack_start(&sidebar_button, false, false, 0);
        buttons.push((name.to_string(), sidebar_button));

        if name != "settings" {
            let top_button = nav_button_compact(icon, &title);
            top_nav_inner.pack_start(&top_button, true, true, 0);
            buttons.push((name.to_string(), top_button));
        }
    }

    let buttons = Rc::new(buttons);
    for (name, button) in buttons.iter() {
        let stack = stack.clone();
        let name = name.clone();
        button.connect_clicked(move |_| {
            stack.set_visible_child_name(&name);
        });
    }
    {
        let buttons = Rc::clone(&buttons);
        stack.connect_notify_local(Some("visible-child-name"), move |stack, _| {
            if let Some(name) = stack.visible_child_name() {
                sync_navigation(&buttons, &name);
            }
        });
    }
    sync_navigation(&buttons, "home");

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    sidebar.pack_start(&spacer, true, true, 0);

    top_nav.show_all();
    top_nav.set_no_show_all(true);
    top_nav.hide();

    (sidebar, top_nav)
}

fn sync_navigation(buttons: &[(String, gtk::Button)], active: &str) {
    for (name, button) in buttons {
        if name == active {
            button.style_context().add_class("active");
        } else {
            button.style_context().remove_class("active");
        }
    }
}

fn build_home_page(stack: &gtk::Stack, tx: &crate::app::CommandSender) -> HomePage {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_start(26);
    content.set_margin_end(26);
    content.set_margin_top(26);
    content.set_margin_bottom(26);

    content.pack_start(
        &page_heading(&tr("Home"), &tr("Your wallpapers at a glance")),
        false,
        false,
        0,
    );

    let (hero, hero_inner) = glass_card(6);
    hero_inner.pack_start(&section_label(&tr("Now playing")), false, false, 0);
    let now_title = label(&tr("No wallpaper set"), "setting-title");
    let now_source = label("", "field-hint");
    let now_status = label(&tr("No wallpaper set"), "status-text");
    now_source.set_ellipsize(pango::EllipsizeMode::End);
    now_source.set_max_width_chars(64);
    hero_inner.pack_start(&now_title, false, false, 0);
    hero_inner.pack_start(&now_source, false, false, 0);
    hero_inner.pack_start(&now_status, false, false, 0);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_margin_top(6);
    let library = gtk::Button::with_label(&tr("Open library"));
    library.style_context().add_class("primary");
    let catalogue = gtk::Button::with_label(&tr("Browse catalogue"));
    catalogue.style_context().add_class("secondary");
    let settings_button = gtk::Button::with_label(&tr("Settings"));
    settings_button.style_context().add_class("secondary");
    actions.pack_start(&library, false, false, 0);
    actions.pack_start(&catalogue, false, false, 0);
    actions.pack_start(&settings_button, false, false, 0);
    hero_inner.pack_start(&actions, false, false, 0);
    content.pack_start(&hero, false, false, 0);

    {
        let stack = stack.clone();
        library.connect_clicked(move |_| stack.set_visible_child_name("video"));
    }
    {
        let stack = stack.clone();
        catalogue.connect_clicked(move |_| stack.set_visible_child_name("online"));
    }
    {
        let stack = stack.clone();
        settings_button.connect_clicked(move |_| stack.set_visible_child_name("settings"));
    }

    let monitors_label = section_label(&tr("Monitors"));
    monitors_label.set_margin_top(6);
    content.pack_start(&monitors_label, false, false, 0);
    let monitors_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.pack_start(&monitors_box, false, false, 0);

    HomePage {
        root: page_scroller(&content),
        now_title,
        now_source,
        now_status,
        monitors_box,
        cards: RefCell::new(Vec::new()),
        videos: RefCell::new(Vec::new()),
        collections: RefCell::new(Vec::new()),
        syncing: Rc::new(Cell::new(false)),
        tx: tx.clone(),
    }
}

impl HomePage {
    fn update(
        &self,
        config: &Config,
        monitors: &[MonitorInfo],
        paused_by_user: bool,
        is_playing: bool,
        library: &Library,
    ) {
        let source = util::primary_source(config, monitors);
        let has_source = !matches!(config.mode, Mode::Null | Mode::Unknown) && !source.is_empty();
        if has_source {
            self.now_title.set_text(&mode_text(config.mode));
            self.now_source.set_text(&source_display(source));
        } else {
            self.now_title.set_text(&tr("No wallpaper set"));
            self.now_source.set_text("");
        }
        let status = if !has_source {
            tr("No wallpaper set")
        } else if paused_by_user || !is_playing {
            tr("Paused")
        } else {
            tr("Playing")
        };
        self.now_status.set_text(&status);

        let videos = util::video_paths();
        let videos_changed = *self.videos.borrow() != videos;
        if videos_changed {
            *self.videos.borrow_mut() = videos.clone();
        }
        let collections: Vec<(String, String)> = library
            .collections()
            .iter()
            .map(|collection| (collection.id.clone(), collection.name.clone()))
            .collect();
        let collections_changed = *self.collections.borrow() != collections;
        if collections_changed {
            *self.collections.borrow_mut() = collections;
        }
        let rebuild = {
            let cards = self.cards.borrow();
            cards.len() != monitors.len()
                || cards
                    .iter()
                    .zip(monitors)
                    .any(|(card, monitor)| card.name != monitor.name)
        };
        if rebuild || videos_changed || collections_changed {
            self.rebuild_cards(monitors, &videos, library);
        }

        self.syncing.set(true);
        {
            let cards = self.cards.borrow();
            for (card, monitor) in cards.iter().zip(monitors) {
                let own = config.source_for(&monitor.name);
                card.source.set_text(&source_display(own));
                let primary = if monitor.is_primary {
                    format!(" · {}", tr("Primary"))
                } else {
                    String::new()
                };
                card.meta
                    .set_text(&format!("{} × {}{primary}", monitor.width, monitor.height));
                let state = if matches!(config.mode, Mode::Null | Mode::Unknown) || own.is_empty() {
                    tr("No wallpaper set")
                } else if paused_by_user || !is_playing {
                    tr("Paused")
                } else {
                    tr("Playing")
                };
                card.state.set_text(&state);
                let active = if videos.iter().any(|path| path.to_string_lossy() == own) {
                    own
                } else {
                    "none"
                };
                if !card.combo.set_active_id(Some(active)) {
                    card.combo.set_active_id(Some("none"));
                }
                let rotation = config
                    .rotations
                    .get(&monitor.name)
                    .map(String::as_str)
                    .unwrap_or("off");
                if !card.rotation.set_active_id(Some(rotation)) {
                    card.rotation.set_active_id(Some("off"));
                }
            }
        }
        self.syncing.set(false);
    }

    fn rebuild_cards(&self, monitors: &[MonitorInfo], videos: &[PathBuf], library: &Library) {
        for child in self.monitors_box.children() {
            self.monitors_box.remove(&child);
        }
        let mut cards = Vec::new();
        for monitor in monitors {
            cards.push(self.build_card(monitor, videos, library));
        }
        *self.cards.borrow_mut() = cards;
        self.monitors_box.show_all();
    }

    fn build_card(
        &self,
        monitor: &MonitorInfo,
        videos: &[PathBuf],
        library: &Library,
    ) -> MonitorCard {
        let (card, inner) = glass_card(8);
        card.set_hexpand(true);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = label(&monitor.name, "setting-title");
        let source = label("", "setting-description");
        source.set_ellipsize(pango::EllipsizeMode::End);
        source.set_max_width_chars(48);
        let meta = label("", "field-hint");
        let state = label("", "status-text");
        text.pack_start(&title, false, false, 0);
        text.pack_start(&source, false, false, 0);
        text.pack_start(&meta, false, false, 0);
        text.pack_start(&state, false, false, 0);

        let combo = gtk::ComboBoxText::new();
        combo.append(Some("none"), &tr("Choose video…"));
        for video in videos {
            let name = video
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| tr("Untitled video"));
            let id = video.to_string_lossy().into_owned();
            combo.append(Some(&id), &name);
        }
        combo.set_size_request(230, -1);
        combo.set_valign(gtk::Align::Center);
        set_a11y_name(&combo, &format!("{} {}", monitor.name, tr("Wallpaper")));
        {
            let tx = self.tx.clone();
            let syncing = Rc::clone(&self.syncing);
            let name = monitor.name.clone();
            combo.connect_changed(move |combo| {
                if syncing.get() {
                    return;
                }
                let Some(path) = combo.active_id().map(|id| id.to_string()) else {
                    return;
                };
                if path == "none" {
                    return;
                }
                let _ = tx.send(Command::Video {
                    path: Some(path),
                    monitor: Some(name.clone()),
                });
            });
        }

        let rotation = gtk::ComboBoxText::new();
        rotation.append(Some("off"), &tr("No rotation"));
        for collection in library.collections() {
            rotation.append(Some(&collection.id), &collection.name);
        }
        rotation.set_size_request(230, -1);
        rotation.set_valign(gtk::Align::Center);
        rotation.set_tooltip_text(Some(&tr("Rotate this collection on this monitor")));
        set_a11y_name(&rotation, &format!("{} {}", monitor.name, tr("Rotation")));
        {
            let tx = self.tx.clone();
            let syncing = Rc::clone(&self.syncing);
            let name = monitor.name.clone();
            rotation.connect_changed(move |combo| {
                if syncing.get() {
                    return;
                }
                let collection = combo.active_id().map(|id| id.to_string());
                let collection = collection.filter(|id| id != "off");
                let _ = tx.send(Command::SetMonitorRotation {
                    monitor: name.clone(),
                    collection,
                });
            });
        }

        let controls = gtk::Box::new(gtk::Orientation::Vertical, 5);
        controls.pack_start(&combo, false, false, 0);
        controls.pack_start(&rotation, false, false, 0);
        row.pack_start(&text, true, true, 0);
        row.pack_end(&controls, false, false, 0);
        inner.pack_start(&row, false, false, 0);
        self.monitors_box.pack_start(&card, false, false, 0);
        MonitorCard {
            name: monitor.name.clone(),
            source,
            meta,
            state,
            combo,
            rotation,
        }
    }
}

struct NowPlaying {
    card: gtk::Box,
    dot: gtk::Box,
    status: gtk::Label,
    mode: gtk::Label,
    source: gtk::Label,
    monitor: gtk::Label,
}

fn build_now_playing_card() -> NowPlaying {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.style_context().add_class("now-card");
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 5);
    inner.set_margin_start(14);
    inner.set_margin_end(14);
    inner.set_margin_top(12);
    inner.set_margin_bottom(12);
    card.pack_start(&inner, true, true, 0);

    let heading = section_label(&tr("Now playing"));
    heading.set_margin_bottom(6);
    inner.pack_start(&heading, false, false, 0);

    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.style_context().add_class("status-dot");
    dot.set_size_request(8, 8);
    dot.set_valign(gtk::Align::Center);
    let status = label(&tr("Ready"), "status-text");
    status.set_xalign(0.0);
    status_row.pack_start(&dot, false, false, 0);
    status_row.pack_start(&status, false, false, 0);
    inner.pack_start(&status_row, false, false, 0);

    let mode = label(&tr("No wallpaper set"), "setting-title");
    mode.set_xalign(0.0);
    mode.set_margin_top(2);
    inner.pack_start(&mode, false, false, 0);

    let source = label("", "field-hint");
    source.set_xalign(0.0);
    source.set_ellipsize(pango::EllipsizeMode::Middle);
    source.set_max_width_chars(24);
    inner.pack_start(&source, false, false, 0);

    let monitor = label("", "field-hint");
    monitor.set_xalign(0.0);
    monitor.set_ellipsize(pango::EllipsizeMode::End);
    monitor.set_max_width_chars(24);
    inner.pack_start(&monitor, false, false, 0);

    NowPlaying {
        card,
        dot,
        status,
        mode,
        source,
        monitor,
    }
}

pub(crate) fn page_heading(title: &str, subtitle: &str) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let title = label(title, "page-title");
    let subtitle = label(subtitle, "page-subtitle");
    subtitle.set_ellipsize(pango::EllipsizeMode::End);
    box_.pack_start(&title, false, false, 0);
    box_.pack_start(&subtitle, false, false, 0);
    box_
}

fn page_scroller(content: &gtk::Box) -> gtk::ScrolledWindow {
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .shadow_type(gtk::ShadowType::None)
        .vexpand(true)
        .build();
    scroller.add(content);
    scroller
}

fn glass_card(spacing: i32) -> (gtk::Box, gtk::Box) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.style_context().add_class("glass-card");
    let inner = gtk::Box::new(gtk::Orientation::Vertical, spacing);
    inner.set_margin_start(20);
    inner.set_margin_end(20);
    inner.set_margin_top(20);
    inner.set_margin_bottom(20);
    card.pack_start(&inner, true, true, 0);
    (card, inner)
}

fn switch_row(title: &str, description: &str, switch: &gtk::Switch) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.style_context().add_class("setting-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let title_label = label(title, "setting-title");
    let description = if description.is_empty() {
        None
    } else {
        Some(label(description, "setting-description"))
    };
    if let Some(description) = &description {
        description.set_line_wrap(true);
        description.set_max_width_chars(38);
    }
    text.pack_start(&title_label, false, false, 0);
    if let Some(description) = &description {
        text.pack_start(description, false, false, 0);
    }
    row.pack_start(&text, true, true, 0);
    row.pack_end(switch, false, false, 0);
    set_a11y_name(switch, title);
    row
}

pub(crate) fn labeled_icon_button(icon: &str, text: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.style_context().add_class("secondary");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    row.pack_start(&icon_image(icon, 16), false, false, 0);
    let text_label = label(text, "button-label");
    row.pack_start(&text_label, false, false, 0);
    button.add(&row);
    button.set_tooltip_text(Some(tooltip));
    set_a11y_name(&button, text);
    button
}

fn menu_action_button(icon: &str, text: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.style_context().add_class("ghost");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    row.pack_start(&icon_image(icon, 16), false, false, 0);
    let label = label(text, "button-label");
    label.set_xalign(0.0);
    row.pack_start(&label, true, true, 0);
    button.add(&row);
    set_a11y_name(&button, text);
    button
}

pub(crate) fn primary_button(text: &str) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.style_context().add_class("primary");
    set_a11y_name(&button, text);
    button
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.style_context().add_class("icon-button");
    button.set_image(Some(&icon_image(icon, 16)));
    button.set_tooltip_text(Some(tooltip));
    set_a11y_name(&button, tooltip);
    button
}

fn nav_button(icon: &str, title: &str, description: Option<&str>) -> gtk::Button {
    let button = nav_button_base(icon, title);
    let row = button
        .child()
        .and_then(|child| child.downcast::<gtk::Box>().ok())
        .expect("nav button content is a box");
    let text = row
        .children()
        .get(1)
        .and_then(|child| child.clone().downcast::<gtk::Box>().ok())
        .expect("nav button text box");
    if let Some(description) = description {
        let desc = label(description, "nav-desc");
        desc.set_ellipsize(pango::EllipsizeMode::End);
        text.pack_start(&desc, false, false, 0);
    }
    button
}

fn nav_button_compact(icon: &str, title: &str) -> gtk::Button {
    nav_button_base(icon, title)
}

fn nav_button_base(icon: &str, title: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.style_context().add_class("nav-item");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let image = icon_image(icon, 18);
    image.style_context().add_class("nav-icon");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let title_label = label(title, "nav-title");
    title_label.set_xalign(0.0);
    text.pack_start(&title_label, false, false, 0);
    row.pack_start(&image, false, false, 0);
    row.pack_start(&text, true, true, 0);
    button.add(&row);
    button.set_tooltip_text(Some(title));
    button.set_focus_on_click(false);
    set_a11y_name(&button, title);
    button
}

fn fill_grid(
    shared: &Rc<Shared>,
    page: &VideoPage,
    library: &Rc<RefCell<Library>>,
    view: &LibraryView,
) {
    if let Some(timer) = shared.thumb_timer.borrow_mut().take() {
        timer.cancel();
    }
    if let Some(cancel) = shared.thumb_cancel.borrow_mut().take() {
        cancel.store(true, Ordering::Relaxed);
    }
    let previous: Vec<gtk::Widget> = page.flow.children();
    for child in previous {
        page.flow.remove(&child);
    }
    page.flow.unselect_all();

    let videos = videos_for_view(view, &library.borrow());
    *shared.videos.borrow_mut() = videos.clone();

    let (sender, receiver) = mpsc::channel::<(usize, PathBuf)>();
    let cancel = Arc::new(AtomicBool::new(false));
    *shared.thumb_cancel.borrow_mut() = Some(Arc::clone(&cancel));
    let (task_sender, task_receiver) = mpsc::channel::<(usize, PathBuf)>();
    let task_receiver = Arc::new(Mutex::new(task_receiver));
    for _ in 0..2 {
        let task_receiver = Arc::clone(&task_receiver);
        let sender = sender.clone();
        let cancel = Arc::clone(&cancel);
        std::thread::spawn(move || loop {
            let task = task_receiver
                .lock()
                .ok()
                .and_then(|receiver| receiver.recv().ok());
            let Some((index, path)) = task else {
                return;
            };
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            if let Some(thumbnail) = thumbnails::generate(&path) {
                if sender.send((index, thumbnail)).is_err() {
                    return;
                }
            }
        });
    }
    let states: ThumbStates = Rc::new(RefCell::new(Vec::new()));

    for (index, path) in videos.iter().enumerate() {
        let slot = Rc::new(RefCell::new(None));
        let area = gtk::DrawingArea::new();
        area.set_size_request(-1, 118);
        area.set_hexpand(true);
        area.set_valign(gtk::Align::Fill);
        area.style_context().add_class("video-image");
        {
            let slot = Rc::clone(&slot);
            area.connect_draw(move |area, cr| {
                draw_video_frame(cr, area, &slot);
                glib::Propagation::Proceed
            });
        }
        states
            .borrow_mut()
            .push(Some((Rc::clone(&slot), area.clone())));

        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| tr("Untitled video"));
        let (card, star) = video_card(&name, &area);
        let path_string = path.to_string_lossy().into_owned();
        set_star(&star, library.borrow().is_favorite(&path_string));
        {
            let library = Rc::clone(library);
            let flow = page.flow.clone();
            let path = path_string.clone();
            let star_for_update = star.clone();
            let page_for_star = page.clone();
            let shared_for_star = Rc::clone(shared);
            let view_for_star = view.clone();
            star.connect_toggled(move |button| {
                let active = button.is_active();
                let mut state = library.borrow_mut();
                if state.is_favorite(&path) != active {
                    state.toggle_favorite(&path);
                    let _ = state.save();
                }
                drop(state);
                update_star_icon(&star_for_update, active);
                if view_for_star == LibraryView::Favorites && !active {
                    let library = Rc::clone(&library);
                    let page = page_for_star.clone();
                    let shared = Rc::clone(&shared_for_star);
                    glib::idle_add_local_once(move || {
                        fill_grid(&shared, &page, &library, &LibraryView::Favorites);
                    });
                } else {
                    flow.invalidate_filter();
                }
            });
        }
        let child = gtk::FlowBoxChild::new();
        child.set_size_request(210, -1);
        child.add(&card);
        child.set_tooltip_text(Some(&name));
        set_a11y_name(&child, &name);
        page.flow.insert(&child, -1);
        child.show_all();
        update_star_icon(&star, library.borrow().is_favorite(&path_string));

        let _ = task_sender.send((index, path.clone()));
    }
    drop(task_sender);
    drop(sender);

    let states_for_timer = Rc::clone(&states);
    let done = Rc::new(Cell::new(false));
    let finished = Rc::clone(&done);
    let id = glib::timeout_add_local(Duration::from_millis(80), move || loop {
        match receiver.try_recv() {
            Ok((index, path)) => {
                if let Ok(pixbuf) = Pixbuf::from_file(&path) {
                    if let Some(Some((slot, area))) = states_for_timer.borrow().get(index) {
                        *slot.borrow_mut() = Some(pixbuf);
                        area.queue_draw();
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                finished.set(true);
                return glib::ControlFlow::Break;
            }
        }
    });
    *shared.thumb_timer.borrow_mut() = Some(PendingTimer { id, done });

    let query = page.search.text().trim().to_lowercase();
    page.flow.invalidate_filter();
    update_library_state(shared, page, &query, view);
    update_selection(&page.flow, shared, page);
    update_view_actions(page, view);
}

fn video_card(name: &str, image: &gtk::DrawingArea) -> (gtk::Box, gtk::ToggleButton) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.style_context().add_class("video-card");

    let frame = gtk::AspectFrame::new(None, 0.5, 0.5, 16.0 / 9.0, false);
    frame.set_halign(gtk::Align::Fill);
    frame.set_valign(gtk::Align::Start);
    frame.set_hexpand(true);
    frame.add(image);
    card.pack_start(&frame, false, false, 0);

    let info = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    info.style_context().add_class("video-info");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let name_label = label(name, "video-name");
    name_label.set_ellipsize(pango::EllipsizeMode::End);
    name_label.set_max_width_chars(24);
    let meta = label(&tr("Video"), "video-meta");
    text.pack_start(&name_label, false, false, 0);
    text.pack_start(&meta, false, false, 0);
    let star = gtk::ToggleButton::new();
    star.style_context().add_class("icon-button");
    star.set_valign(gtk::Align::Start);
    star.set_relief(gtk::ReliefStyle::None);
    update_star_icon(&star, false);
    info.pack_start(&text, true, true, 0);
    info.pack_end(&star, false, false, 0);
    card.pack_start(&info, true, true, 0);
    (card, star)
}

fn update_star_icon(button: &gtk::ToggleButton, active: bool) {
    let action = tr(if active {
        "Remove from favorites"
    } else {
        "Add to favorites"
    });
    button.set_image(Some(&icon_image(
        if active {
            "starred-symbolic"
        } else {
            "non-starred-symbolic"
        },
        16,
    )));
    button.set_tooltip_text(Some(&action));
    set_a11y_name(button, &action);
}

pub(crate) fn draw_video_frame(
    cr: &gtk::cairo::Context,
    area: &gtk::DrawingArea,
    slot: &Rc<RefCell<Option<Pixbuf>>>,
) {
    let width = area.allocated_width() as f64;
    let height = area.allocated_height() as f64;
    if width <= 0.0 || height <= 0.0 {
        return;
    }

    let _ = cr.save();
    rounded_rect_top(cr, 0.0, 0.0, width, height, 19.0);
    cr.clip();

    if let Some(pixbuf) = slot.borrow().as_ref() {
        let pw = pixbuf.width() as f64;
        let ph = pixbuf.height() as f64;
        let scale = (width / pw).max(height / ph);
        let draw_width = pw * scale;
        let draw_height = ph * scale;
        cr.translate((width - draw_width) / 2.0, (height - draw_height) / 2.0);
        cr.scale(scale, scale);
        cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
        let _ = cr.paint();
    } else {
        let gradient = gtk::cairo::LinearGradient::new(0.0, 0.0, width, height);
        gradient.add_color_stop_rgba(0.0, 0.95, 0.79, 0.48, 0.26);
        gradient.add_color_stop_rgba(1.0, 0.37, 0.56, 1.0, 0.20);
        let _ = cr.set_source(&gradient);
        let _ = cr.paint();

        let cx = width / 2.0;
        let cy = height / 2.0;
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.30);
        cr.arc(cx, cy, 17.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        cr.set_source_rgba(0.12, 0.09, 0.04, 0.72);
        cr.move_to(cx - 4.5, cy - 7.0);
        cr.line_to(cx + 8.0, cy);
        cr.line_to(cx - 4.5, cy + 7.0);
        cr.close_path();
        let _ = cr.fill();
    }
    let _ = cr.restore();

    // A hairline at the bottom of the image separates it from the info panel.
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.12);
    cr.rectangle(0.0, height - 1.0, width, 1.0);
    let _ = cr.fill();
}

fn rounded_rect_top(
    cr: &gtk::cairo::Context,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    cr.move_to(x + radius, y);
    cr.line_to(x + width - radius, y);
    cr.arc(
        x + width - radius,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    cr.line_to(x + width, y + height);
    cr.line_to(x, y + height);
    cr.line_to(x, y + radius);
    cr.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        1.5 * std::f64::consts::PI,
    );
    cr.close_path();
}

fn update_library_state(shared: &Rc<Shared>, page: &VideoPage, query: &str, view: &LibraryView) {
    let videos = shared.videos.borrow();
    let total = videos.len();
    let matches = if query.is_empty() {
        total
    } else {
        videos
            .iter()
            .filter(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_lowercase().contains(query))
                    .unwrap_or(false)
            })
            .count()
    };
    drop(videos);

    if total == 0 {
        let (title, body, show_folder) = match view {
            LibraryView::All => (
                tr("No videos yet"),
                tr("Add videos to your Ferramari folder, then refresh the list."),
                true,
            ),
            LibraryView::Favorites => (
                tr("No favorite videos"),
                tr("Mark a video with the star to see it here."),
                false,
            ),
            LibraryView::Recent => (
                tr("No recently used videos"),
                tr("Apply a video and it will appear here."),
                false,
            ),
            LibraryView::Collection(_) => (
                tr("Collection is empty"),
                tr("Add videos to this collection."),
                false,
            ),
        };
        page.empty_title.set_text(&title);
        page.empty_body.set_text(&body);
        page.library_stack.set_visible_child_name("empty");
        page.empty_folder.set_visible(show_folder);
        page.empty_clear.set_visible(false);
        page.count.set_text(&tr("No videos"));
        page.count.set_tooltip_text(Some(&body));
    } else if matches == 0 {
        page.empty_title
            .set_text(&tr("No videos match your search"));
        page.empty_body
            .set_text(&tr("Try a different name or clear the search."));
        page.library_stack.set_visible_child_name("empty");
        page.empty_folder.set_visible(false);
        page.empty_clear.set_visible(true);
        page.count
            .set_text(&format!("0 {}", tr_n("match", "matches", 0)));
        page.count
            .set_tooltip_text(Some(&tr("No videos match your search")));
    } else {
        page.library_stack.set_visible_child_name("grid");
        page.empty_folder.set_visible(false);
        page.empty_clear.set_visible(false);
        if query.is_empty() {
            page.count.set_text(&video_count(total));
            page.count.set_tooltip_text(None);
        } else {
            page.count.set_text(&format!("{matches} / {total}"));
            page.count.set_tooltip_text(Some(&format!(
                "{} {}",
                matches,
                tr_n("match", "matches", matches as u32)
            )));
        }
    }

    update_view_actions(page, view);
    if page
        .flow
        .selected_children()
        .iter()
        .any(|child| !child.is_child_visible())
    {
        page.flow.unselect_all();
    }
}

fn update_selection(flow: &gtk::FlowBox, shared: &Shared, page: &VideoPage) {
    let selected = selected_video(flow, shared);
    let collection = matches!(view_from_page(page), LibraryView::Collection(_));
    match selected {
        Some(path) => {
            let name = Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or(path);
            page.selection_label.set_text(&name);
            page.selection_label.set_tooltip_text(Some(&name));
            page.apply_button.set_sensitive(true);
            page.add_to_collection.set_sensitive(true);
            page.remove_from_collection.set_sensitive(collection);
        }
        None => {
            page.selection_label
                .set_text(&tr("Select a video to apply"));
            page.selection_label.set_tooltip_text(None);
            page.apply_button.set_sensitive(false);
            page.add_to_collection.set_sensitive(false);
            page.remove_from_collection.set_sensitive(false);
        }
    }
}

fn selected_video(flow: &gtk::FlowBox, shared: &Shared) -> Option<String> {
    let selected = flow.selected_children();
    let child = selected.iter().find(|child| child.is_child_visible())?;
    let index = child.index();
    if index < 0 {
        return None;
    }
    let videos = shared.videos.borrow();
    videos
        .get(index as usize)
        .map(|path| path.to_string_lossy().into_owned())
}

fn video_count(total: usize) -> String {
    format!("{total} {}", tr_n("video", "videos", total as u32))
}

fn mode_text(mode: Mode) -> String {
    match mode {
        Mode::Video => tr("Local Video"),
        Mode::Stream => tr("Streaming"),
        Mode::Webpage => tr("Web Page"),
        Mode::Null | Mode::Unknown => tr("No wallpaper set"),
    }
}

fn source_display(source: &str) -> String {
    if source.is_empty() {
        return tr("No wallpaper set");
    }
    if let Some(path) = source.strip_prefix("file://") {
        return Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.to_string());
    }
    if source.starts_with('/') {
        return Path::new(source)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.to_string());
    }
    source.to_string()
}

fn monitor_text(monitors: &[MonitorInfo]) -> String {
    match monitors.first() {
        None => tr("No displays detected"),
        Some(first) if monitors.len() == 1 => {
            format!("{} · {} × {}", first.name, first.width, first.height)
        }
        Some(first) => format!(
            "{} + {} · {} × {}",
            first.name,
            monitors.len() - 1,
            first.width,
            first.height
        ),
    }
}

fn set_dot_state(dot: &gtk::Box, state: &str) {
    let context = dot.style_context();
    for class in ["playing", "paused", "busy"] {
        context.remove_class(class);
    }
    context.add_class(state);
}

pub(crate) fn set_a11y_name(widget: &impl IsA<gtk::Widget>, name: &str) {
    if let Some(object) = widget.accessible() {
        object.set_accessible_name(Some(name));
    }
}

pub(crate) fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.style_context().add_class(class);
    label.set_xalign(0.0);
    label
}

fn section_label(text: &str) -> gtk::Label {
    let label = label(text, "section-label");
    label.set_margin_top(8);
    label.set_margin_bottom(2);
    label
}

pub(crate) fn icon_image(icon: &str, pixels: i32) -> gtk::Image {
    let image = gtk::Image::from_icon_name(Some(icon), gtk::IconSize::Button);
    image.set_pixel_size(pixels);
    image
}

fn hint_expander(title: &str, hint: &str, link: Option<&str>) -> gtk::Expander {
    let expander = gtk::Expander::new(Some(title));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let label = gtk::Label::new(Some(hint));
    label.set_xalign(0.0);
    label.set_line_wrap(true);
    label.set_max_width_chars(62);
    content.pack_start(&label, false, false, 0);
    if let Some(link) = link {
        let link_label = gtk::Label::new(None);
        link_label.set_markup(&format!("<a href=\"{link}\">{link}</a>"));
        link_label.set_xalign(0.0);
        link_label.set_line_wrap(true);
        content.pack_start(&link_label, false, false, 0);
    }
    expander.add(&content);
    expander
}

fn show_monitor_menu(
    shared: &Shared,
    anchor: &impl IsA<gtk::Widget>,
    event: Option<&gdk::Event>,
    video_path: String,
) {
    let menu = gtk::Menu::new();
    let monitors = shared.monitors.borrow().clone();
    let heading = gtk::MenuItem::with_label(&tr("Apply to"));
    heading.set_sensitive(false);
    menu.append(&heading);
    menu.append(&gtk::SeparatorMenuItem::new());
    for monitor in monitors {
        let item = gtk::MenuItem::with_label(&format!("{} {}", tr("Set For"), monitor.name));
        let tx = shared.tx.clone();
        let path = video_path.clone();
        let name = monitor.name.clone();
        item.connect_activate(move |_| {
            let _ = tx.send(Command::Video {
                path: Some(path.clone()),
                monitor: Some(name.clone()),
            });
        });
        menu.append(&item);
    }
    menu.append(&gtk::SeparatorMenuItem::new());
    let all = gtk::MenuItem::with_label(&tr("Set For All"));
    {
        let tx = shared.tx.clone();
        let path = video_path;
        all.connect_activate(move |_| {
            let _ = tx.send(Command::Video {
                path: Some(path.clone()),
                monitor: Some("all".to_string()),
            });
        });
    }
    menu.append(&all);
    menu.show_all();
    *shared.menu.borrow_mut() = Some(menu.clone());
    match event {
        Some(event) => menu.popup_at_pointer(Some(event)),
        None => menu.popup_at_widget(anchor, gdk::Gravity::South, gdk::Gravity::North, None),
    }
}

fn show_message(
    parent: Option<&gtk::Window>,
    message_type: gtk::MessageType,
    title: &str,
    text: &str,
) {
    let dialog = gtk::MessageDialog::new(
        parent,
        gtk::DialogFlags::MODAL,
        message_type,
        gtk::ButtonsType::Ok,
        title,
    );
    dialog.set_secondary_text(Some(text));
    if let Some(button) = dialog.widget_for_response(gtk::ResponseType::Ok) {
        button.style_context().add_class("primary");
    }
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.show_all();
}

fn show_about(parent: &gtk::Window, version: &str) {
    let about = gtk::AboutDialog::new();
    about.set_transient_for(Some(parent));
    about.set_modal(true);
    about.set_program_name("Ferramari");
    about.set_version(Some(version));
    about.set_comments(Some(&tr("Video wallpaper for Linux. Written in Rust. 🦀")));
    about.set_copyright(Some("Copyright © 2026 Ferramari contributors"));
    about.set_website(Some("https://github.com/ferramari/ferramari"));
    about.set_website_label(Some(&tr("Project website")));
    about.set_authors(&["Ferramari contributors"]);
    about.set_artists(&["Ferramari icon: original artwork"]);
    about.set_logo_icon_name(Some(paths::APP_ID));
    about.connect_response(|about, _| about.close());
    about.show_all();
}

fn xdg_open(path: &Path) {
    if let Err(err) = std::process::Command::new("xdg-open").arg(path).spawn() {
        log::warn!("[GUI] xdg-open {} failed: {err}", path.display());
    }
}
