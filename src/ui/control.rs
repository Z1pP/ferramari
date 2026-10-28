//! The control window: local videos, streaming URLs, web pages and quick
//! toggles. Built in code (no GtkBuilder XML) so widget names are checked by
//! the compiler.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Sender;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

use crate::app::Command;
use crate::config::Config;
use crate::monitors::MonitorInfo;
use crate::paths;
use crate::thumbnails;
use crate::util::{self, tr};

struct Shared {
    tx: Sender<Command>,
    videos: RefCell<Vec<PathBuf>>,
    monitors: RefCell<Vec<MonitorInfo>>,
    menu: RefCell<Option<gtk::Menu>>,
}

pub struct ControlWindow {
    pub window: gtk::ApplicationWindow,
    shared: Rc<Shared>,
    list_store: gtk::ListStore,
    scale_volume: gtk::Scale,
    toggle_mute: gtk::ToggleButton,
    volume_button: gtk::MenuButton,
    volume_label: gtk::Label,
    spin_blur: gtk::SpinButton,
    check_autostart: gtk::CheckButton,
    check_static: gtk::CheckButton,
    check_pause_max: gtk::CheckButton,
    check_mute_max: gtk::CheckButton,
    play_button: gtk::Button,
    syncing: Rc<Cell<bool>>,
}

impl ControlWindow {
    pub fn new(app: &gtk::Application, tx: Sender<Command>, version: &str) -> Self {
        let window = gtk::ApplicationWindow::new(app);
        window.set_title("Ferramari");
        window.set_default_size(1024, 576);
        window.set_position(gtk::WindowPosition::Center);
        window.set_icon_name(Some(paths::APP_ID));

        let shared = Rc::new(Shared {
            tx: tx.clone(),
            videos: RefCell::new(Vec::new()),
            monitors: RefCell::new(Vec::new()),
            menu: RefCell::new(None),
        });
        let syncing = Rc::new(Cell::new(false));

        let list_store =
            gtk::ListStore::new(&[gdk_pixbuf::Pixbuf::static_type(), glib::Type::STRING]);
        let icon_view = gtk::IconView::builder()
            .model(&list_store)
            .pixbuf_column(0)
            .text_column(1)
            .item_width(96)
            .row_spacing(8)
            .column_spacing(8)
            .item_padding(8)
            .margin(6)
            .build();

        // ---- header bar -------------------------------------------------
        let stack = gtk::Stack::new();
        let switcher = gtk::StackSwitcher::new();
        switcher.set_stack(Some(&stack));

        let header = gtk::HeaderBar::new();
        header.set_show_close_button(true);
        header.set_custom_title(Some(&switcher));

        let play_button = icon_button("media-playback-pause-symbolic", &tr("Toggle Play/Pause"));
        header.pack_start(&play_button);

        let lucky_button = gtk::Button::with_label("🎲");
        lucky_button.set_tooltip_text(Some(&tr("I'm Feeling Lucky")));
        header.pack_start(&lucky_button);

        // ---- local video page -------------------------------------------
        let video_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
        page_margins(&video_page);

        let video_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let video_title = gtk::Label::new(None);
        video_title.set_markup(&format!(
            "<span size=\"x-large\"><b>{}</b></span>\n{}",
            tr("Local Video"),
            tr("Videos located in Ferramari folder")
        ));
        video_title.set_xalign(0.0);
        video_header.pack_start(&video_title, true, true, 0);

        let folder_button = icon_button("folder-open-symbolic", &tr("Open Ferramari folder"));
        let refresh_button = icon_button("view-refresh-symbolic", &tr("Refresh"));
        video_header.pack_end(&refresh_button, false, false, 0);
        video_header.pack_end(&folder_button, false, false, 0);
        video_page.pack_start(&video_header, false, false, 0);

        let scrolled = gtk::ScrolledWindow::builder()
            .shadow_type(gtk::ShadowType::In)
            .vexpand(true)
            .build();
        scrolled.add(&icon_view);
        video_page.pack_start(&scrolled, true, true, 0);

        let apply_button = gtk::Button::with_label(&tr("Apply"));
        apply_button.style_context().add_class("suggested-action");
        apply_button.set_halign(gtk::Align::End);
        video_page.pack_end(&apply_button, false, false, 0);
        stack.add_titled(&video_page, "video", &tr("Local Video"));

        // ---- streaming page ---------------------------------------------
        let stream_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
        page_margins(&stream_page);
        stream_page.pack_start(
            &page_title(&tr("Streaming"), &tr("Videos from streaming services")),
            false,
            false,
            0,
        );
        stream_page.pack_start(
            &gtk::Separator::new(gtk::Orientation::Horizontal),
            false,
            false,
            0,
        );

        let stream_form = gtk::Box::new(gtk::Orientation::Vertical, 8);
        stream_form.set_width_request(512);
        stream_form.set_halign(gtk::Align::Center);
        stream_form.pack_start(&bold_label(&tr("Streaming URL")), false, false, 0);
        let stream_entry = gtk::Entry::new();
        stream_entry.set_placeholder_text(Some("https://www.youtube.com/watch?v=id"));
        stream_entry.set_input_purpose(gtk::InputPurpose::Url);
        stream_entry
            .set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("go-next-symbolic"));
        stream_form.pack_start(&stream_entry, false, false, 0);
        stream_form.pack_start(
            &hint_expander(
                &tr("Hint"),
                &format!("💡\t{} <i>yt-dlp</i>", tr("This feature uses as backend")),
                Some(&tr("😉\tTry playing any YouTube video!")),
            ),
            false,
            false,
            0,
        );
        stream_page.pack_start(&stream_form, false, false, 8);
        stack.add_titled(&stream_page, "stream", &tr("Streaming"));

        // ---- web page ---------------------------------------------------
        let web_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
        page_margins(&web_page);
        web_page.pack_start(
            &page_title(&tr("Web Page"), &tr("Set a web page as wallpaper")),
            false,
            false,
            0,
        );
        web_page.pack_start(
            &gtk::Separator::new(gtk::Orientation::Horizontal),
            false,
            false,
            0,
        );

        let web_form = gtk::Box::new(gtk::Orientation::Vertical, 8);
        web_form.set_width_request(512);
        web_form.set_halign(gtk::Align::Center);
        web_form.pack_start(&bold_label(&tr("Web Page URL")), false, false, 0);
        let web_entry = gtk::Entry::new();
        web_entry.set_placeholder_text(Some("https://www.google.com/"));
        web_entry.set_input_purpose(gtk::InputPurpose::Url);
        web_entry
            .set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("go-next-symbolic"));
        web_form.pack_start(&web_entry, false, false, 0);
        web_form.pack_start(
            &hint_expander(
                &tr("Hint"),
                &tr("Check this website for some really amazing examples!"),
                Some("https://alteredqualia.com/"),
            ),
            false,
            false,
            0,
        );
        web_form.pack_start(&bold_label(&tr("Local Web Page")), false, false, 0);
        web_form.pack_start(
            &gtk::Label::new(Some(&tr(
                "Alternately, you can choose a HTML file on your PC",
            ))),
            false,
            false,
            0,
        );

        let chooser_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let chooser =
            gtk::FileChooserButton::new(&tr("Choose a HTML file"), gtk::FileChooserAction::Open);
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("HTML"));
        filter.add_mime_type("text/html");
        chooser.add_filter(filter);
        chooser.set_hexpand(true);
        let web_apply = gtk::Button::with_label(&tr("Apply"));
        web_apply.style_context().add_class("suggested-action");
        chooser_row.pack_start(&chooser, true, true, 0);
        chooser_row.pack_end(&web_apply, false, false, 0);
        web_form.pack_start(&chooser_row, false, false, 0);
        web_page.pack_start(&web_form, false, false, 8);
        stack.add_titled(&web_page, "webpage", &tr("Web Page"));

        // ---- menu popover -----------------------------------------------
        let popover = gtk::Popover::builder().width_request(300).build();
        let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        menu_box.set_margin_start(10);
        menu_box.set_margin_end(10);
        menu_box.set_margin_top(10);
        menu_box.set_margin_bottom(10);

        menu_box.pack_start(&dim_label(&tr("Quick toggles")), false, false, 0);
        let check_autostart = gtk::CheckButton::with_label(&tr("Autostart after login"));
        let check_static = gtk::CheckButton::with_label(&tr("Apply static wallpaper"));
        check_static.set_tooltip_text(Some(&tr(
            "Video frame can be applied as system wallpaper, look great in GNOME",
        )));
        let check_pause_max = gtk::CheckButton::with_label(&tr("Pause when maximized window"));
        check_pause_max.set_tooltip_text(Some(&tr(
            "Automatically pauses playback when maximized window or full screen mode is detected",
        )));
        let check_mute_max = gtk::CheckButton::with_label(&tr("Mute when maximized window"));
        check_mute_max.set_tooltip_text(Some(&tr(
            "Automatically mutes playback when maximized window or full screen mode is detected",
        )));
        menu_box.pack_start(&check_autostart, false, false, 0);
        menu_box.pack_start(&check_static, false, false, 0);
        menu_box.pack_start(&check_pause_max, false, false, 0);
        menu_box.pack_start(&check_mute_max, false, false, 0);

        let label_blur = dim_label(&tr("Static wallpaper blur radius"));
        menu_box.pack_start(&label_blur, false, false, 0);
        let spin_blur = gtk::SpinButton::with_range(0.0, 20.0, 1.0);
        menu_box.pack_start(&spin_blur, false, false, 0);

        menu_box.pack_start(
            &gtk::Separator::new(gtk::Orientation::Horizontal),
            false,
            false,
            0,
        );
        let config_button = flat_button(&tr("Open config file"));
        let about_button = flat_button(&tr("About Ferramari"));
        let quit_button = flat_button(&tr("Quit Ferramari"));
        menu_box.pack_start(&config_button, false, false, 0);
        menu_box.pack_start(&about_button, false, false, 0);
        menu_box.pack_start(&quit_button, false, false, 0);
        menu_box.show_all();
        popover.add(&menu_box);

        // Dedicated volume control, always one click away from the header.
        let volume_button = gtk::MenuButton::new();
        volume_button.set_tooltip_text(Some(&tr("Volume")));
        volume_button.set_image(Some(&gtk::Image::from_icon_name(
            Some("audio-volume-high-symbolic"),
            gtk::IconSize::Button,
        )));
        let volume_popover = gtk::Popover::builder().width_request(250).build();
        let volume_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        volume_box.set_margin_start(10);
        volume_box.set_margin_end(10);
        volume_box.set_margin_top(10);
        volume_box.set_margin_bottom(10);
        let volume_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let volume_title = gtk::Label::new(Some(&tr("Volume")));
        volume_title.set_xalign(0.0);
        volume_title.set_hexpand(true);
        let volume_label = gtk::Label::new(Some("50%"));
        volume_label.set_xalign(1.0);
        volume_header.pack_start(&volume_title, true, true, 0);
        volume_header.pack_end(&volume_label, false, false, 0);
        volume_box.pack_start(&volume_header, false, false, 0);

        let volume_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let scale_volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        scale_volume.set_draw_value(false);
        scale_volume.set_hexpand(true);
        let toggle_mute = gtk::ToggleButton::new();
        toggle_mute.set_tooltip_text(Some(&tr("Mute")));
        toggle_mute.set_image(Some(&gtk::Image::from_icon_name(
            Some("audio-volume-high-symbolic"),
            gtk::IconSize::Button,
        )));
        toggle_mute.set_always_show_image(true);
        volume_row.pack_start(&scale_volume, true, true, 0);
        volume_row.pack_end(&toggle_mute, false, false, 0);
        volume_box.pack_start(&volume_row, false, false, 0);
        volume_box.show_all();
        volume_popover.add(&volume_box);
        volume_button.set_popover(Some(&volume_popover));
        header.pack_end(&volume_button);

        let menu_button = gtk::MenuButton::new();
        menu_button.set_image(Some(&gtk::Image::from_icon_name(
            Some("open-menu-symbolic"),
            gtk::IconSize::Button,
        )));
        menu_button.set_popover(Some(&popover));
        header.pack_end(&menu_button);

        window.set_titlebar(Some(&header));
        window.add(&stack);

        // ---- behaviour ---------------------------------------------------
        {
            let tx = tx.clone();
            play_button.connect_clicked(move |_| {
                let _ = tx.send(Command::TogglePause);
            });
        }
        {
            let tx = tx.clone();
            lucky_button.connect_clicked(move |_| {
                let _ = tx.send(Command::FeelingLucky);
            });
        }
        refresh_button.connect_clicked({
            let shared = Rc::clone(&shared);
            let store = list_store.clone();
            move |_| fill_store(&shared, &store)
        });
        folder_button.connect_clicked(|_| xdg_open(&paths::video_wallpaper_dir()));

        apply_button.connect_clicked({
            let shared = Rc::clone(&shared);
            let icon_view = icon_view.clone();
            move |button| {
                if let Some(path) = selected_video(&icon_view, &shared) {
                    show_monitor_menu(&shared, button, None, path);
                } else {
                    let parent = button
                        .toplevel()
                        .and_then(|widget| widget.downcast::<gtk::Window>().ok());
                    show_message(
                        parent.as_ref(),
                        gtk::MessageType::Info,
                        &tr("No Video Selected"),
                        &tr("There are no video selected.\nPlease choose one first."),
                    );
                }
            }
        });
        icon_view.connect_button_press_event({
            let shared = Rc::clone(&shared);
            let icon_view = icon_view.clone();
            move |view, event| {
                if event.button() != 3 {
                    return glib::Propagation::Proceed;
                }
                if let Some(path) =
                    view.path_at_pos(event.position().0 as i32, event.position().1 as i32)
                {
                    view.grab_focus();
                    view.select_path(&path);
                    if let Some(path) = selected_video(&icon_view, &shared) {
                        show_monitor_menu(&shared, view, Some(event), path);
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        });

        {
            let tx = tx.clone();
            stream_entry.connect_activate(move |entry| send_entry(&tx, entry, true));
        }
        {
            let tx = tx.clone();
            stream_entry.connect_icon_press(move |entry, position, _| {
                if position == gtk::EntryIconPosition::Secondary {
                    send_entry(&tx, entry, true);
                }
            });
        }
        {
            let tx = tx.clone();
            web_entry.connect_activate(move |entry| send_entry(&tx, entry, false));
        }
        {
            let tx = tx.clone();
            web_entry.connect_icon_press(move |entry, position, _| {
                if position == gtk::EntryIconPosition::Secondary {
                    send_entry(&tx, entry, false);
                }
            });
        }
        {
            let tx = tx.clone();
            web_apply.connect_clicked(move |_| {
                if let Some(file) = chooser.file() {
                    if let Some(path) = file.path() {
                        let _ = tx.send(Command::Webpage {
                            url: path.to_string_lossy().into_owned(),
                        });
                    }
                }
            });
        }

        {
            let tx = tx.clone();
            let syncing = Rc::clone(&syncing);
            let volume_label = volume_label.clone();
            scale_volume.connect_value_changed(move |scale| {
                volume_label.set_text(&format!("{}%", scale.value() as i32));
                if syncing.get() {
                    return;
                }
                let _ = tx.send(Command::SetVolume(scale.value() as i64));
            });
        }
        {
            let tx = tx.clone();
            let syncing = Rc::clone(&syncing);
            toggle_mute.connect_toggled(move |button| {
                if syncing.get() {
                    return;
                }
                let _ = tx.send(Command::SetMute(button.is_active()));
            });
        }
        {
            let tx = tx.clone();
            let syncing = Rc::clone(&syncing);
            spin_blur.connect_value_changed(move |spin| {
                if syncing.get() {
                    return;
                }
                let _ = tx.send(Command::SetBlurRadius(spin.value() as i64));
            });
        }
        {
            let tx = tx.clone();
            let syncing = Rc::clone(&syncing);
            check_autostart.connect_toggled(move |check| {
                if syncing.get() {
                    return;
                }
                let _ = tx.send(Command::SetAutostart(check.is_active()));
            });
        }
        {
            let tx = tx.clone();
            let syncing = Rc::clone(&syncing);
            check_static.connect_toggled(move |check| {
                if syncing.get() {
                    return;
                }
                let _ = tx.send(Command::SetStaticWallpaper(check.is_active()));
            });
        }
        {
            let tx = tx.clone();
            let syncing = Rc::clone(&syncing);
            check_pause_max.connect_toggled(move |check| {
                if syncing.get() {
                    return;
                }
                let _ = tx.send(Command::SetPauseWhenMaximized(check.is_active()));
            });
        }
        {
            let tx = tx.clone();
            let syncing = Rc::clone(&syncing);
            check_mute_max.connect_toggled(move |check| {
                if syncing.get() {
                    return;
                }
                let _ = tx.send(Command::SetMuteWhenMaximized(check.is_active()));
            });
        }
        config_button.connect_clicked(|_| xdg_open(&paths::config_path()));
        {
            let tx = tx.clone();
            quit_button.connect_clicked(move |_| {
                let _ = tx.send(Command::Quit);
            });
        }
        about_button.connect_clicked({
            let window = window.clone();
            let version = version.to_string();
            move |_| show_about(window.upcast_ref::<gtk::Window>(), &version)
        });

        window.connect_delete_event(|window, _| {
            window.hide();
            glib::Propagation::Stop
        });

        if paths::is_wayland() {
            check_pause_max.set_visible(false);
            check_mute_max.set_visible(false);
        }
        if !paths::is_gnome() {
            check_static.set_visible(false);
            label_blur.set_visible(false);
            spin_blur.set_visible(false);
        }

        let control = Self {
            window,
            shared,
            list_store,
            scale_volume,
            toggle_mute,
            volume_button,
            volume_label,
            spin_blur,
            check_autostart,
            check_static,
            check_pause_max,
            check_mute_max,
            play_button,
            syncing,
        };
        fill_store(&control.shared, &control.list_store);
        control
    }

    pub fn present(&self) {
        self.window.show_all();
        self.window.present();
    }

    pub fn refresh(
        &self,
        config: &Config,
        monitors: &[MonitorInfo],
        autostart_enabled: bool,
        paused_by_user: bool,
    ) {
        self.syncing.set(true);
        *self.shared.monitors.borrow_mut() = monitors.to_vec();

        self.check_autostart.set_active(autostart_enabled);
        self.check_static.set_active(config.is_static_wallpaper);
        self.check_pause_max
            .set_active(config.is_pause_when_maximized);
        self.check_mute_max
            .set_active(config.is_mute_when_maximized);
        self.scale_volume.set_value(config.volume as f64);
        self.volume_label.set_text(&format!("{}%", config.volume));
        self.spin_blur.set_value(config.blur_radius as f64);
        self.toggle_mute.set_active(config.is_mute);
        self.update_mute_icon(config.volume, config.is_mute);
        self.scale_volume.set_sensitive(!config.is_mute);
        self.spin_blur.set_sensitive(config.is_static_wallpaper);
        self.play_button.set_image(Some(&gtk::Image::from_icon_name(
            Some(if paused_by_user {
                "media-playback-start-symbolic"
            } else {
                "media-playback-pause-symbolic"
            }),
            gtk::IconSize::Button,
        )));
        self.syncing.set(false);
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
        self.toggle_mute.set_image(Some(&gtk::Image::from_icon_name(
            Some(icon),
            gtk::IconSize::Button,
        )));
        self.volume_button
            .set_image(Some(&gtk::Image::from_icon_name(
                Some(icon),
                gtk::IconSize::Button,
            )));
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

fn fill_store(shared: &Shared, store: &gtk::ListStore) {
    let videos = util::video_paths();
    store.clear();
    *shared.videos.borrow_mut() = videos.clone();

    let generic_icon = gtk::IconTheme::default().and_then(|theme| {
        theme
            .load_icon("video-x-generic", 96, gtk::IconLookupFlags::empty())
            .ok()
    });

    let (sender, receiver) = std::sync::mpsc::channel::<(u32, PathBuf)>();
    let store_for_thumbs = store.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || loop {
        match receiver.try_recv() {
            Ok((index, path)) => {
                if let Ok(pixbuf) = gdk_pixbuf::Pixbuf::from_file_at_size(&path, -1, 96) {
                    if let Some(iter) = store_for_thumbs.iter_nth_child(None, index as i32) {
                        store_for_thumbs.set(&iter, &[(0, &pixbuf as &dyn ToValue)]);
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return glib::ControlFlow::Break,
        }
    });

    for (index, path) in videos.iter().enumerate() {
        let iter = store.append();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match &generic_icon {
            Some(pixbuf) => store.set(
                &iter,
                &[(0, pixbuf as &dyn ToValue), (1, &name as &dyn ToValue)],
            ),
            None => store.set(&iter, &[(1, &name as &dyn ToValue)]),
        }
        let sender = sender.clone();
        let path = path.clone();
        std::thread::spawn(move || {
            if let Some(thumbnail) = thumbnails::generate(&path) {
                let _ = sender.send((index as u32, thumbnail));
            }
        });
    }
    drop(sender);
}

fn selected_video(icon_view: &gtk::IconView, shared: &Shared) -> Option<String> {
    let selected = icon_view.selected_items();
    let path = selected.first()?;
    let index = *path.indices().first()? as usize;
    let videos = shared.videos.borrow();
    videos
        .get(index)
        .map(|path| path.to_string_lossy().into_owned())
}

fn show_monitor_menu(
    shared: &Shared,
    anchor: &impl IsA<gtk::Widget>,
    event: Option<&gdk::Event>,
    video_path: String,
) {
    let menu = gtk::Menu::new();
    let monitors = shared.monitors.borrow().clone();
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

fn send_entry(tx: &Sender<Command>, entry: &gtk::Entry, stream: bool) {
    let url = entry.text().to_string();
    if url.is_empty() {
        return;
    }
    let command = if stream {
        Command::Stream { url }
    } else {
        Command::Webpage { url }
    };
    let _ = tx.send(command);
}

fn page_title(title: &str, subtitle: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_markup(&format!(
        "<span size=\"x-large\"><b>{title}</b></span>\n{subtitle}"
    ));
    label.set_xalign(0.0);
    label
}

fn bold_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_markup(&format!("<b>{text}</b>"));
    label.set_halign(gtk::Align::Start);
    label
}

fn dim_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_sensitive(false);
    label.set_halign(gtk::Align::Start);
    label
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(Some(icon), gtk::IconSize::Button);
    button.set_tooltip_text(Some(tooltip));
    button
}

fn flat_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.set_relief(gtk::ReliefStyle::None);
    button.set_halign(gtk::Align::Fill);
    button
}

fn page_margins(widget: &impl IsA<gtk::Widget>) {
    widget.set_margin_start(16);
    widget.set_margin_end(16);
    widget.set_margin_top(16);
    widget.set_margin_bottom(16);
}

fn hint_expander(title: &str, hint: &str, link: Option<&str>) -> gtk::Expander {
    let expander = gtk::Expander::new(Some(title));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let label = gtk::Label::new(None);
    label.set_use_markup(true);
    label.set_markup(hint);
    label.set_xalign(0.0);
    label.set_line_wrap(true);
    content.pack_start(&label, false, false, 0);
    if let Some(link) = link {
        let link_label = gtk::Label::new(None);
        link_label.set_markup(&format!("<a href=\"{link}\">{link}</a>"));
        link_label.set_xalign(0.0);
        content.pack_start(&link_label, false, false, 0);
    }
    expander.add(&content);
    expander
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
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.show_all();
}

fn show_about(parent: &gtk::Window, version: &str) {
    let about = gtk::AboutDialog::new();
    about.set_transient_for(Some(parent));
    about.set_modal(true);
    about.set_program_name("Ferramari");
    about.set_version(Some(version));
    about.set_comments(Some(
        "Video wallpaper for Linux. Written in Rust. 🦀\n\n\
         Ferramari — a Rust rewrite of Hidamari (日溜まり, \"sunny spot\")\n\
         by Jeff Shee.",
    ));
    about.set_copyright(Some(
        "Copyright © 2022 Jeff Shee (original Hidamari)\n\
         Copyright © 2026 Ferramari contributors",
    ));
    about.set_website(Some("https://github.com/jeffshee/hidamari"));
    about.set_website_label(Some(&tr("Original Hidamari project")));
    about.set_authors(&[
        "Jeff Shee (original Hidamari) https://github.com/jeffshee",
        "Ferramari contributors",
    ]);
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
