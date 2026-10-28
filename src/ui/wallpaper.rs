//! Wallpaper windows: one X11 toplevel per monitor.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use webkit2gtk::WebViewExt;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

use crate::fade::Fade;
use crate::monitors::MonitorInfo;
use crate::mpv::{Mpv, Options as MpvOptions};
use crate::ui::menu;
use crate::ytdlp::StreamInfo;

pub struct WallpaperWindow {
    window: gtk::ApplicationWindow,
    content: Content,
    fade: Fade,
    primary: bool,
    key: WindowKey,
    // Kept alive for the lifetime of the window.
    _menu: gtk::Menu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    Video,
    Stream,
    Web,
}

/// What a wallpaper window shows: kind, monitor and source. Two windows are
/// interchangeable only when all three match, which lets a change on one
/// monitor keep the other players running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowKey {
    pub kind: WindowKind,
    pub monitor: String,
    pub source: String,
}

enum Content {
    Video { mpv: Rc<RefCell<Mpv>> },
    Web { webview: webkit2gtk::WebView },
}

impl WallpaperWindow {
    pub fn video(
        app: &gtk::Application,
        monitor: &MonitorInfo,
        source: &str,
        volume: i64,
        muted: bool,
    ) -> anyhow::Result<Self> {
        let primary = monitor.is_primary;
        let (window, xid) = base_window(app, monitor)?;
        let mpv = match Mpv::spawn(&MpvOptions {
            wid: Some(xid),
            source: source.to_string(),
            audio_file: None,
            no_audio: !primary,
            volume: if primary { volume } else { 0 },
            muted,
            paused: true,
            loop_file: true,
        }) {
            Ok(mpv) => Rc::new(RefCell::new(mpv)),
            Err(err) => {
                // GTK adds the toplevel to the application and keeps its own
                // reference: dropping our wrapper alone would leave a blank
                // desktop window mapped forever.
                window.close();
                return Err(err);
            }
        };
        let menu = attach_menu(app, &window);
        Ok(Self {
            window,
            content: Content::Video { mpv },
            fade: Fade::default(),
            primary,
            key: WindowKey {
                kind: WindowKind::Video,
                monitor: monitor.name.clone(),
                source: source.to_string(),
            },
            _menu: menu,
        })
    }

    pub fn stream(
        app: &gtk::Application,
        monitor: &MonitorInfo,
        stream: &StreamInfo,
        volume: i64,
        muted: bool,
    ) -> anyhow::Result<Self> {
        let primary = monitor.is_primary;
        let (window, xid) = base_window(app, monitor)?;
        let mpv = match Mpv::spawn(&MpvOptions {
            wid: Some(xid),
            source: stream.video_url.clone(),
            audio_file: if primary {
                stream.audio_url.clone()
            } else {
                None
            },
            no_audio: !primary,
            volume: if primary { volume } else { 0 },
            muted,
            paused: true,
            loop_file: true,
        }) {
            Ok(mpv) => Rc::new(RefCell::new(mpv)),
            Err(err) => {
                window.close();
                return Err(err);
            }
        };
        let menu = attach_menu(app, &window);
        Ok(Self {
            window,
            content: Content::Video { mpv },
            fade: Fade::default(),
            primary,
            key: WindowKey {
                kind: WindowKind::Stream,
                monitor: monitor.name.clone(),
                source: stream.video_url.clone(),
            },
            _menu: menu,
        })
    }

    pub fn web(
        app: &gtk::Application,
        monitor: &MonitorInfo,
        uri: &str,
        muted: bool,
    ) -> anyhow::Result<Self> {
        let primary = monitor.is_primary;
        let (window, _) = base_window(app, monitor)?;

        let webview = webkit2gtk::WebView::new();
        webview.set_is_muted(muted || !primary);
        window.add(&webview);
        webview.show();
        webview.load_uri(uri);

        let menu = attach_menu(app, &window);
        let menu_for_context = menu.clone();
        let window_for_context = window.clone();
        let _ = &window_for_context;
        webview.connect_context_menu(move |_, _, event, _| {
            menu_for_context.popup_at_pointer(Some(event));
            true
        });

        Ok(Self {
            window,
            content: Content::Web { webview },
            fade: Fade::default(),
            primary,
            key: WindowKey {
                kind: WindowKind::Web,
                monitor: monitor.name.clone(),
                source: uri.to_string(),
            },
            _menu: menu,
        })
    }

    pub fn key(&self) -> &WindowKey {
        &self.key
    }

    pub fn is_primary(&self) -> bool {
        self.primary
    }

    pub fn play_fade(&mut self, target: i64, duration: f64, interval: f64) {
        match &self.content {
            Content::Video { mpv } => {
                let _ = mpv.borrow_mut().set_pause(false);
                if self.primary {
                    let mpv = Rc::clone(mpv);
                    self.fade.start(
                        0.0,
                        target as f64,
                        duration,
                        interval,
                        move |volume| {
                            let _ = mpv.borrow_mut().set_volume(volume as i64);
                        },
                        || {},
                    );
                }
            }
            Content::Web { .. } => {}
        }
    }

    pub fn pause_fade(&mut self, duration: f64, interval: f64) {
        match &self.content {
            Content::Video { mpv } => {
                if self.primary {
                    let from = mpv.borrow().volume as f64;
                    let mpv_for_volume = Rc::clone(mpv);
                    let mpv_for_pause = Rc::clone(mpv);
                    self.fade.start(
                        from,
                        0.0,
                        duration,
                        interval,
                        move |volume| {
                            let _ = mpv_for_volume.borrow_mut().set_volume(volume as i64);
                        },
                        move || {
                            let _ = mpv_for_pause.borrow_mut().set_pause(true);
                        },
                    );
                } else {
                    let _ = mpv.borrow_mut().set_pause(true);
                }
            }
            Content::Web { .. } => {}
        }
    }

    pub fn volume_fade(&mut self, target: i64, duration: f64, interval: f64) {
        if let Content::Video { mpv } = &self.content {
            let from = mpv.borrow().volume as f64;
            let mpv = Rc::clone(mpv);
            self.fade.start(
                from,
                target as f64,
                duration,
                interval,
                move |volume| {
                    let _ = mpv.borrow_mut().set_volume(volume as i64);
                },
                || {},
            );
        }
    }

    pub fn set_volume(&mut self, volume: i64) {
        if let Content::Video { mpv } = &self.content {
            let _ = mpv.borrow_mut().set_volume(volume);
        }
    }

    pub fn set_mute(&mut self, muted: bool) {
        match &self.content {
            Content::Video { mpv } => {
                let _ = mpv.borrow_mut().set_mute(muted);
            }
            Content::Web { webview } => webview.set_is_muted(muted),
        }
    }
}

impl Drop for WallpaperWindow {
    fn drop(&mut self) {
        self.fade.cancel();
        self.window.close();
    }
}

fn base_window(
    app: &gtk::Application,
    monitor: &MonitorInfo,
) -> anyhow::Result<(gtk::ApplicationWindow, u64)> {
    let window = gtk::ApplicationWindow::new(app);
    window.set_decorated(false);
    window.set_resizable(false);
    window.set_type_hint(gdk::WindowTypeHint::Desktop);
    window.set_skip_taskbar_hint(true);
    window.set_skip_pager_hint(true);
    window.set_accept_focus(false);
    window.set_can_focus(false);
    window.set_default_size(monitor.width, monitor.height);
    window.set_size_request(monitor.width, monitor.height);
    window.move_(monitor.x, monitor.y);
    window.show_all();
    window.resize(monitor.width, monitor.height);

    let gdk_window = window
        .window()
        .ok_or_else(|| anyhow::anyhow!("wallpaper window was not realized"))?;
    let x11 = gdk_window
        .downcast::<gdkx11::X11Window>()
        .map_err(|_| anyhow::anyhow!("GDK is not using X11; cannot embed the video surface"))?;
    let xid = x11.xid();

    // Desktop-icons extensions keep their own window at the bottom of the
    // DESKTOP layer. A freshly mapped wallpaper window sits above it and hides
    // the icons, so put ourselves below it now and again after the icons
    // extension has settled.
    lower_x11_window(xid);
    let _ = glib::timeout_add_local(Duration::from_millis(2500), move || {
        lower_x11_window(xid);
        glib::ControlFlow::Break
    });

    Ok((window, xid))
}

fn attach_menu(app: &gtk::Application, window: &gtk::ApplicationWindow) -> gtk::Menu {
    let menu = menu::wallpaper_menu(app);
    window.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
    let popup = menu.clone();
    window.connect_button_press_event(move |_, event| {
        if event.button() == 3 {
            popup.popup_at_pointer(Some(event));
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    menu
}

/// Put an X11 window below other windows in the same layer. Used to keep the
/// wallpaper underneath the desktop icons.
fn lower_x11_window(xid: u64) {
    let Ok((connection, _)) = x11rb::connect(None) else {
        return;
    };
    use x11rb::protocol::xproto::{ConfigureWindowAux, StackMode};
    let _ = connection.configure_window(
        xid as u32,
        &ConfigureWindowAux::new().stack_mode(StackMode::BELOW),
    );
    let _ = connection.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(kind: WindowKind, monitor: &str, source: &str) -> WindowKey {
        WindowKey {
            kind,
            monitor: monitor.to_string(),
            source: source.to_string(),
        }
    }

    #[test]
    fn keys_match_only_when_all_fields_do() {
        let original = key(WindowKind::Video, "eDP-1", "/videos/a.mp4");
        assert!(original == original.clone());
        // Same monitor and file, different mode: a new window is needed.
        assert_ne!(original, key(WindowKind::Stream, "eDP-1", "/videos/a.mp4"));
        // Another monitor or another file: rebuild only that window.
        assert_ne!(original, key(WindowKind::Video, "HDMI-1", "/videos/a.mp4"));
        assert_ne!(original, key(WindowKind::Video, "eDP-1", "/videos/b.mp4"));
    }

    #[test]
    fn unchanged_monitors_survive_a_sync_plan() {
        // Mirrors App::sync_windows: a window is kept when its key is still
        // in the desired set, so only the changed monitor is rebuilt.
        let desired = [
            key(WindowKind::Video, "eDP-1", "/videos/a.mp4"),
            key(WindowKind::Video, "HDMI-1", "/videos/new.mp4"),
        ];
        let existing = [
            key(WindowKind::Video, "eDP-1", "/videos/a.mp4"),
            key(WindowKind::Video, "HDMI-1", "/videos/old.mp4"),
        ];
        let kept: Vec<&WindowKey> = existing
            .iter()
            .filter(|window| desired.contains(window))
            .collect();
        assert_eq!(kept, vec![&existing[0]]);
    }
}
