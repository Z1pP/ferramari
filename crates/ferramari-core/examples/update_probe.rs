//! Prints what the settings row would show after a real release check.
//!
//! The check is the same function the control surface calls; this only makes
//! it runnable from a terminal, where the answer can be read without opening
//! a window and clicking through a settings page.

fn main() {
    let status = ferramari_core::update::check();
    println!("current:  {}", ferramari_core::update::current_version());
    println!("status:   {}", status.text());
    let record = ferramari_core::update::Record::load();
    println!("checked:  {:?}", record.checked_on());
    println!(
        "tag:      {}",
        if record.version.is_empty() {
            "-"
        } else {
            &record.version
        }
    );
    println!(
        "link:     {}",
        if record.url.is_empty() {
            "-"
        } else {
            &record.url
        }
    );
}
