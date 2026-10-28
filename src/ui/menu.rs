//! Context menu shared by wallpaper windows.

use gtk::prelude::*;

use crate::util::tr;

pub fn wallpaper_menu(app: &gtk::Application) -> gtk::Menu {
    let menu = gtk::Menu::new();
    // A GtkMenu is a separate popup toplevel, not a descendant of the
    // application window, so it cannot find `app.*` actions on its own.
    // Without this every item stays insensitive (greyed out, unclickable).
    menu.insert_action_group("app", Some(app));
    for (label, action) in [
        ("Show Ferramari", "app.show"),
        ("Toggle Mute Audio", "app.toggle-mute"),
        ("Toggle Play/Pause", "app.toggle-pause"),
        ("Reload", "app.reload"),
        ("I'm Feeling Lucky", "app.lucky"),
        ("Quit Ferramari", "app.quit"),
    ] {
        let item = gtk::MenuItem::with_label(&tr(label));
        item.set_action_name(Some(action));
        menu.append(&item);
    }
    menu.show_all();
    menu
}
