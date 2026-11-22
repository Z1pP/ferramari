//! Test-only idle source for the benchmark harness.
//!
//! Ferramari polls `org.gnome.Mutter.IdleMonitor.GetIdletime` on the session
//! bus. The Island benchmark bus has no Mutter, so this tiny service owns the
//! name and reports a fixed idle time. It is only built on Linux; the shipped
//! server never uses it.
//!
//! ```sh
//! cargo build --release -p ferramari-platform --example idle_stub
//! DBUS_SESSION_BUS_ADDRESS=... target/release/examples/idle_stub --idle-ms 3600000
//! ```

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use zbus::blocking::connection::Builder;

    struct IdleMonitor {
        idle_ms: Arc<AtomicU64>,
    }

    #[zbus::interface(name = "org.gnome.Mutter.IdleMonitor")]
    impl IdleMonitor {
        #[zbus(name = "GetIdletime")]
        fn get_idletime(&self) -> u64 {
            self.idle_ms.load(Ordering::Relaxed)
        }

        /// A test hook: `gdbus call ... SetIdle 0` makes the desktop active
        /// again without moving the real pointer.
        #[zbus(name = "SetIdle")]
        fn set_idle(&self, millis: u64) {
            self.idle_ms.store(millis, Ordering::Relaxed);
        }
    }

    pub fn run() -> anyhow::Result<()> {
        let idle_ms = std::env::args()
            .skip(1)
            .collect::<Vec<_>>()
            .windows(2)
            .find(|pair| pair[0] == "--idle-ms")
            .and_then(|pair| pair[1].parse::<u64>().ok())
            .unwrap_or(3_600_000);
        let monitor = IdleMonitor {
            idle_ms: Arc::new(AtomicU64::new(idle_ms)),
        };
        let _connection = Builder::session()?
            .name("org.gnome.Mutter.IdleMonitor")?
            .serve_at("/org/gnome/Mutter/IdleMonitor/Core", monitor)?
            .build()?;
        eprintln!("[idle-stub] reporting {idle_ms} ms idle");
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("idle_stub is a Linux benchmark helper");
}
