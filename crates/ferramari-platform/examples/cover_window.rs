//! Test-only fullscreen cover for the benchmark harness.
//!
//! The Eco Engine pauses a monitor when a normal window covers it. This helper
//! creates one such window on a chosen monitor and keeps it alive until the
//! harness terminates the process. It is not part of the shipped server; build
//! it only on a Linux benchmark host:
//!
//! ```sh
//! cargo build --release -p ferramari-platform --example cover_window
//! ```

#[cfg(target_os = "linux")]
mod linux {
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt as _, CreateWindowAux,
        EventMask, PropMode, WindowClass,
    };

    pub fn run() -> anyhow::Result<()> {
        let index = std::env::args()
            .skip(1)
            .collect::<Vec<_>>()
            .windows(2)
            .find(|pair| pair[0] == "--monitor")
            .and_then(|pair| pair[1].parse::<usize>().ok())
            .unwrap_or(0);
        let (connection, screen_number) = x11rb::connect(None)?;
        let screen = &connection.setup().roots[screen_number];
        let monitors = connection
            .randr_get_monitors(screen.root, true)?
            .reply()?
            .monitors;
        let (x, y, width, height) = monitors
            .get(index)
            .map(|monitor| (monitor.x, monitor.y, monitor.width, monitor.height))
            .unwrap_or((0, 0, screen.width_in_pixels, screen.height_in_pixels));

        let window = connection.generate_id()?;
        let aux = CreateWindowAux::new()
            .background_pixel(0)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY);
        connection.create_window(
            0, // CopyFromParent depth
            window,
            screen.root,
            x,
            y,
            width,
            height,
            0,
            WindowClass::INPUT_OUTPUT,
            0, // CopyFromParent visual
            &aux,
        )?;
        let wm_state = connection
            .intern_atom(false, b"_NET_WM_STATE")?
            .reply()?
            .atom;
        let fullscreen = connection
            .intern_atom(false, b"_NET_WM_STATE_FULLSCREEN")?
            .reply()?
            .atom;
        let wm_type = connection
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE")?
            .reply()?
            .atom;
        let normal = connection
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE_NORMAL")?
            .reply()?
            .atom;
        // x11rb 0.14 takes the raw property bytes; format 32 with one unit is
        // the atom-valued form of `_NET_WM_WINDOW_TYPE`.
        let normal_bytes = normal.to_ne_bytes();
        connection.change_property(
            PropMode::REPLACE,
            window,
            wm_type,
            AtomEnum::ATOM,
            32,
            1,
            &normal_bytes,
        )?;
        // EWMH fullscreen request: _NET_WM_STATE_ADD (1), fullscreen, no
        // second property, source indication 1 (a normal application).
        let event = ClientMessageEvent::new(
            32,
            window,
            wm_state,
            ClientMessageData::from([1u32, fullscreen, 0, 1, 0]),
        );
        connection.send_event(
            false,
            screen.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )?;
        connection.map_window(window)?;
        connection.flush()?;
        eprintln!("[cover] monitor {index} {width}x{height}+{x},{y} mapped");
        // The harness stops this process with SIGTERM. Ignore X events until
        // the connection closes so the window stays mapped.
        loop {
            match connection.wait_for_event() {
                Ok(_) => {}
                Err(_) => break,
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("cover_window is a Linux benchmark helper");
}
