//! The online wallpaper catalogue: browse it, preview a wallpaper,
//! download it on request and install it through the normal monitor menu.
//!
//! Network work runs on plain threads; results are marshalled back to the GTK
//! thread through a channel that a GTK timeout polls, the same pattern
//! the local video grid uses for thumbnails. Nothing is fetched until the page
//! is opened or the user asks for another page.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use gdk_pixbuf::Pixbuf;
use gtk::gdk;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;

use crate::catalogue::{self, Category, DownloadInfo, Downloaded, Listing, Probe, Wallpaper};
use crate::ui::control::{
    draw_video_frame, icon_image, label, labeled_icon_button, page_heading, primary_button,
    set_a11y_name, PendingTimer, ThumbSlot,
};
use crate::util::{tr, tr_n};

pub type ApplyFn = Rc<dyn Fn(&gtk::Button, String)>;
pub type RefreshFn = Rc<dyn Fn()>;

/// What the preview dialog can do to the rest of the application.
#[derive(Clone)]
pub struct Context {
    /// Pop the existing "Apply to <monitor> / Set For All" menu.
    pub apply: ApplyFn,
    /// Re-read the local video library after installing a wallpaper.
    pub refresh_library: RefreshFn,
}

#[derive(Clone)]
pub struct OnlinePage {
    pub root: gtk::Box,
    flow: gtk::FlowBox,
    stack: gtk::Stack,
    spinner: gtk::Spinner,
    loading_label: gtk::Label,
    error_label: gtk::Label,
    empty_title: gtk::Label,
    empty_body: gtk::Label,
    retry: gtk::Button,
    show_more: gtk::Button,
    count: gtk::Label,
    search: gtk::Entry,
    category: gtk::ComboBoxText,
    refresh: gtk::Button,
    footer: gtk::Label,
    syncing: Rc<Cell<bool>>,
    state: Rc<CatalogState>,
    context: Context,
}

struct CatalogState {
    items: RefCell<Vec<Wallpaper>>,
    next: RefCell<Option<String>>,
    categories: RefCell<Vec<Category>>,
    thumbs: RefCell<Vec<Option<(ThumbSlot, gtk::DrawingArea)>>>,
    thumb_timer: RefCell<Option<PendingTimer>>,
    thumb_cancel: RefCell<Option<Arc<AtomicBool>>>,
    list_generation: Cell<u64>,
    thumb_generation: Cell<u64>,
    loading: Cell<bool>,
    query: RefCell<String>,
    category_url: RefCell<Option<String>>,
}

#[derive(Clone)]
enum Request {
    Listing(String),
    Search(String),
}

pub fn build(context: Context) -> Rc<OnlinePage> {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
    root.set_margin_start(26);
    root.set_margin_end(26);
    root.set_margin_top(26);
    root.set_margin_bottom(26);

    let header = page_heading(
        &tr("Wallpaper catalogue"),
        &tr("Preview live wallpapers before installing them"),
    );
    root.pack_start(&header, false, false, 0);

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    toolbar.style_context().add_class("glass-toolbar");
    toolbar.set_margin_start(10);
    toolbar.set_margin_end(10);
    toolbar.set_margin_top(10);
    toolbar.set_margin_bottom(10);

    let category = gtk::ComboBoxText::new();
    category.append(Some("all"), &tr("All wallpapers"));
    category.set_active_id(Some("all"));
    category.set_size_request(190, -1);
    category.set_tooltip_text(Some(&tr("Filter by category")));
    set_a11y_name(&category, &tr("Wallpaper category"));
    toolbar.pack_start(&category, false, false, 0);

    let search = gtk::Entry::new();
    search.set_placeholder_text(Some(&tr("Search wallpapers…")));
    search.style_context().add_class("search-entry");
    search.set_hexpand(true);
    search.set_icon_from_icon_name(gtk::EntryIconPosition::Primary, Some("edit-find-symbolic"));
    search.set_icon_tooltip_text(
        gtk::EntryIconPosition::Primary,
        Some(&tr("Search wallpapers")),
    );
    set_a11y_name(&search, &tr("Search wallpapers"));
    toolbar.pack_start(&search, true, true, 0);

    let refresh = gtk::Button::new();
    refresh.style_context().add_class("icon-button");
    refresh.set_image(Some(&icon_image("view-refresh-symbolic", 16)));
    refresh.set_tooltip_text(Some(&tr("Reload the catalogue")));
    set_a11y_name(&refresh, &tr("Reload the catalogue"));
    toolbar.pack_end(&refresh, false, false, 0);

    let count = label("", "count-pill");
    count.set_valign(gtk::Align::Center);
    count.set_margin_end(4);
    toolbar.pack_end(&count, false, false, 0);
    root.pack_start(&toolbar, false, false, 0);

    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_min_children_per_line(1);
    flow.set_max_children_per_line(6);
    flow.set_row_spacing(16);
    flow.set_column_spacing(16);
    flow.set_valign(gtk::Align::Start);
    flow.set_halign(gtk::Align::Fill);
    flow.style_context().add_class("video-grid");
    flow.set_activate_on_single_click(true);
    set_a11y_name(&flow, &tr("Online wallpapers"));

    let grid_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .shadow_type(gtk::ShadowType::None)
        .vexpand(true)
        .build();
    grid_scroller.add(&flow);

    let loading = gtk::Box::new(gtk::Orientation::Vertical, 12);
    loading.style_context().add_class("empty-state");
    loading.set_halign(gtk::Align::Center);
    loading.set_valign(gtk::Align::Center);
    let spinner = gtk::Spinner::new();
    spinner.set_size_request(34, 34);
    spinner.set_halign(gtk::Align::Center);
    let loading_label = label(&tr("Loading wallpapers…"), "empty-title");
    loading_label.set_halign(gtk::Align::Center);
    loading.pack_start(&spinner, false, false, 0);
    loading.pack_start(&loading_label, false, false, 0);

    let empty = gtk::Box::new(gtk::Orientation::Vertical, 10);
    empty.style_context().add_class("empty-state");
    empty.set_halign(gtk::Align::Center);
    empty.set_valign(gtk::Align::Center);
    empty.set_vexpand(true);
    let empty_icon = icon_image("folder-download-symbolic", 44);
    empty_icon.style_context().add_class("empty-icon");
    empty_icon.set_halign(gtk::Align::Center);
    let empty_title = label("", "empty-title");
    empty_title.set_halign(gtk::Align::Center);
    let empty_body = label("", "empty-body");
    empty_body.set_halign(gtk::Align::Center);
    empty_body.set_justify(gtk::Justification::Center);
    empty_body.set_line_wrap(true);
    empty_body.set_max_width_chars(48);
    empty.pack_start(&empty_icon, false, false, 0);
    empty.pack_start(&empty_title, false, false, 0);
    empty.pack_start(&empty_body, false, false, 0);

    let error = gtk::Box::new(gtk::Orientation::Vertical, 10);
    error.style_context().add_class("empty-state");
    error.set_halign(gtk::Align::Center);
    error.set_valign(gtk::Align::Center);
    error.set_vexpand(true);
    let error_icon = icon_image("dialog-warning-symbolic", 44);
    error_icon.style_context().add_class("empty-icon");
    error_icon.set_halign(gtk::Align::Center);
    let error_title = label(&tr("Could not load wallpapers"), "empty-title");
    error_title.set_halign(gtk::Align::Center);
    let error_label = label("", "empty-body");
    error_label.set_halign(gtk::Align::Center);
    error_label.set_line_wrap(true);
    error_label.set_max_width_chars(52);
    let retry = primary_button(&tr("Try again"));
    retry.set_halign(gtk::Align::Center);
    retry.set_margin_top(8);
    error.pack_start(&error_icon, false, false, 0);
    error.pack_start(&error_title, false, false, 0);
    error.pack_start(&error_label, false, false, 0);
    error.pack_start(&retry, false, false, 0);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(160);
    stack.set_vexpand(true);
    stack.add_named(&grid_scroller, "grid");
    stack.add_named(&loading, "loading");
    stack.add_named(&empty, "empty");
    stack.add_named(&error, "error");
    root.pack_start(&stack, true, true, 0);

    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let footer = label("", "field-hint");
    footer.set_ellipsize(pango::EllipsizeMode::End);
    footer.set_hexpand(true);
    bottom.pack_start(&footer, true, true, 0);
    let show_more = gtk::Button::with_label(&tr("Show more"));
    show_more.style_context().add_class("secondary");
    show_more.set_tooltip_text(Some(&tr("Load the next page")));
    show_more.hide();
    show_more.set_no_show_all(true);
    bottom.pack_end(&show_more, false, false, 0);
    root.pack_start(&bottom, false, false, 0);

    let page = Rc::new(OnlinePage {
        root,
        flow: flow.clone(),
        stack: stack.clone(),
        spinner: spinner.clone(),
        loading_label,
        error_label,
        empty_title: empty_title.clone(),
        empty_body: empty_body.clone(),
        retry: retry.clone(),
        show_more: show_more.clone(),
        count,
        search: search.clone(),
        category: category.clone(),
        refresh: refresh.clone(),
        footer,
        syncing: Rc::new(Cell::new(false)),
        state: Rc::new(CatalogState {
            items: RefCell::new(Vec::new()),
            next: RefCell::new(None),
            categories: RefCell::new(Vec::new()),
            thumbs: RefCell::new(Vec::new()),
            thumb_timer: RefCell::new(None),
            thumb_cancel: RefCell::new(None),
            list_generation: Cell::new(0),
            thumb_generation: Cell::new(0),
            loading: Cell::new(false),
            query: RefCell::new(String::new()),
            category_url: RefCell::new(None),
        }),
        context,
    });

    wire_page(&page);
    // Reclaim old downloads in the background; the first request is not
    // blocked by cache maintenance.
    std::thread::spawn(catalogue::cleanup_cache);

    page
}

/// Start the first request when the user opens the page. Re-visits keep the
/// existing catalogue, including search and scroll position.
pub fn show(page: &Rc<OnlinePage>) {
    if page.state.list_generation.get() == 0 && !page.state.loading.get() {
        load_page(
            page,
            Request::Listing(catalogue::listing_url(None, 1)),
            false,
        );
    }
}

fn wire_page(page: &Rc<OnlinePage>) {
    {
        let page = Rc::clone(page);
        let category = page.category.clone();
        category.connect_changed(move |combo| {
            if page.syncing.get() {
                return;
            }
            let active = combo.active_id().map(|id| id.to_string());
            let url = active.filter(|id| id != "all");
            *page.state.category_url.borrow_mut() = url;
            if !page.state.query.borrow().is_empty() {
                clear_search(&page);
            }
            load_current(&page, false);
        });
    }

    {
        let page = Rc::clone(page);
        let search = page.search.clone();
        search.connect_activate(move |entry| {
            let query = entry.text().trim().to_string();
            *page.state.query.borrow_mut() = query.clone();
            page.category.set_sensitive(query.is_empty());
            if query.is_empty() {
                load_current(&page, false);
            } else {
                load_page(&page, Request::Search(query), false);
            }
            update_search_icon(entry);
        });
    }

    {
        let page = Rc::clone(page);
        let search = page.search.clone();
        search.connect_icon_press(move |entry, position, _| {
            if position != gtk::EntryIconPosition::Secondary {
                return;
            }
            entry.set_text("");
            *page.state.query.borrow_mut() = String::new();
            page.category.set_sensitive(true);
            update_search_icon(entry);
            load_current(&page, false);
        });
    }

    {
        let page = Rc::clone(page);
        let refresh = page.refresh.clone();
        refresh.connect_clicked(move |_| load_current(&page, false));
    }

    {
        let page = Rc::clone(page);
        let retry = page.retry.clone();
        retry.connect_clicked(move |_| load_current(&page, false));
    }

    {
        let page = Rc::clone(page);
        let show_more = page.show_more.clone();
        show_more.connect_clicked(move |_| {
            if page.state.loading.get() {
                return;
            }
            let next = page.state.next.borrow().clone();
            if let Some(url) = next {
                load_page(&page, Request::Listing(url), true);
            }
        });
    }

    {
        let page = Rc::clone(page);
        let flow = page.flow.clone();
        flow.connect_child_activated(move |_, child| {
            let index = child.index();
            if index < 0 {
                return;
            }
            let item = page.state.items.borrow().get(index as usize).cloned();
            if let Some(item) = item {
                if let Some(window) = page
                    .root
                    .toplevel()
                    .and_then(|top| top.downcast::<gtk::Window>().ok())
                {
                    open_preview(&window, &page.context, item);
                }
            }
        });
    }
}

fn clear_search(page: &OnlinePage) {
    page.search.set_text("");
    page.search
        .set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, None);
    page.category.set_sensitive(true);
    *page.state.query.borrow_mut() = String::new();
}

fn update_search_icon(entry: &gtk::Entry) {
    if entry.text().is_empty() {
        entry.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, None);
    } else {
        entry.set_icon_from_icon_name(
            gtk::EntryIconPosition::Secondary,
            Some("edit-clear-symbolic"),
        );
        entry.set_icon_tooltip_text(gtk::EntryIconPosition::Secondary, Some(&tr("Clear search")));
    }
}

fn load_current(page: &Rc<OnlinePage>, append: bool) {
    let query = page.state.query.borrow().clone();
    if query.is_empty() {
        let url = catalogue::listing_url(page.state.category_url.borrow().as_deref(), 1);
        load_page(page, Request::Listing(url), append);
    } else {
        load_page(page, Request::Search(query), append);
    }
}

/// Poll a worker channel on the GTK thread and hand the first value to
/// `deliver`. GTK objects never leave the main thread, so worker threads only
/// send plain data through the channel.
fn poll_once<T: 'static>(
    receiver: mpsc::Receiver<T>,
    interval: Duration,
    mut deliver: impl FnMut(T) + 'static,
) -> PendingTimer {
    let done = Rc::new(Cell::new(false));
    let finished = Rc::clone(&done);
    let id = glib::timeout_add_local(interval, move || match receiver.try_recv() {
        Ok(value) => {
            deliver(value);
            finished.set(true);
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            finished.set(true);
            glib::ControlFlow::Break
        }
    });
    PendingTimer { id, done }
}

fn load_page(page: &Rc<OnlinePage>, request: Request, append: bool) {
    let generation = page.state.list_generation.get() + 1;
    page.state.list_generation.set(generation);
    page.state.loading.set(true);
    page.footer.set_text("");
    page.show_more.set_sensitive(false);

    if !append {
        page.spinner.start();
        page.loading_label.set_text(&tr("Loading wallpapers…"));
        page.stack.set_visible_child_name("loading");
        clear_items(page);
    } else {
        page.show_more.set_label(&tr("Loading…"));
    }

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = match request {
            Request::Listing(url) => catalogue::load_listing(&url),
            Request::Search(query) => catalogue::search(&query),
        };
        let _ = sender.send(result);
    });
    let page = Rc::clone(page);
    poll_once(receiver, Duration::from_millis(50), move |result| {
        apply_page(&page, result, append, generation);
    });
}

fn clear_items(page: &OnlinePage) {
    if let Some(cancel) = page.state.thumb_cancel.borrow_mut().take() {
        cancel.store(true, Ordering::Relaxed);
    }
    if let Some(timer) = page.state.thumb_timer.borrow_mut().take() {
        timer.cancel();
    }
    page.state
        .thumb_generation
        .set(page.state.thumb_generation.get() + 1);
    let previous: Vec<gtk::Widget> = page.flow.children();
    for child in previous {
        page.flow.remove(&child);
    }
    page.state.items.borrow_mut().clear();
    page.state.thumbs.borrow_mut().clear();
    *page.state.next.borrow_mut() = None;
    page.show_more.hide();
    page.count.set_text("");
}

fn apply_page(
    page: &Rc<OnlinePage>,
    result: anyhow::Result<Listing>,
    _append: bool,
    generation: u64,
) {
    if page.state.list_generation.get() != generation {
        return;
    }
    page.state.loading.set(false);
    page.spinner.stop();
    page.show_more.set_label(&tr("Show more"));

    let listing = match result {
        Ok(listing) => listing,
        Err(err) => {
            if page.state.items.borrow().is_empty() {
                page.error_label.set_text(&err.to_string());
                page.stack.set_visible_child_name("error");
                page.count.set_text(&tr("Unavailable"));
                page.show_more.hide();
            } else {
                page.footer
                    .set_text(&format!("{}: {err}", tr("Could not load more")));
                page.stack.set_visible_child_name("grid");
                page.show_more
                    .set_visible(page.state.next.borrow().is_some());
                page.show_more.set_sensitive(true);
            }
            return;
        }
    };

    if !listing.categories.is_empty() {
        populate_categories(page, &listing.categories);
    }
    *page.state.next.borrow_mut() = listing.next.clone();

    let searching = !page.state.query.borrow().is_empty();
    let mut known: HashSet<String> = page
        .state
        .items
        .borrow()
        .iter()
        .map(|item| item.id.clone())
        .collect();
    let start;
    {
        let mut items = page.state.items.borrow_mut();
        start = items.len();
        for item in listing.items {
            if searching && !item.category.starts_with("Живые обои") {
                continue;
            }
            if known.insert(item.id.clone()) {
                items.push(item);
            }
        }
    }

    let items: Vec<Wallpaper> = page.state.items.borrow()[start..].to_vec();
    for item in items {
        add_card(page, item);
    }

    let total = page.state.items.borrow().len();
    if total == 0 {
        if searching {
            page.count
                .set_text(&format!("0 {}", tr_n("result", "results", 0)));
            set_empty(
                page,
                &tr("No wallpapers found"),
                &tr("Try a different search, or clear it to browse the catalogue."),
            );
        } else {
            page.count
                .set_text(&format!("0 {}", tr_n("wallpaper", "wallpapers", 0)));
            set_empty(
                page,
                &tr("No wallpapers in this category"),
                &tr("Try another category or reload the catalogue."),
            );
        }
        page.show_more.hide();
        return;
    }

    page.stack.set_visible_child_name("grid");
    if searching {
        page.count.set_text(&format!(
            "{total} {}",
            tr_n("result", "results", total as u32)
        ));
    } else {
        page.count.set_text(&format!(
            "{total} {}",
            tr_n("wallpaper", "wallpapers", total as u32)
        ));
    }
    page.show_more
        .set_visible(page.state.next.borrow().is_some());
    page.show_more.set_sensitive(true);
    start_thumbnails(page);
}

fn set_empty(page: &OnlinePage, title: &str, body: &str) {
    page.empty_title.set_text(title);
    page.empty_body.set_text(body);
    page.stack.set_visible_child_name("empty");
}

fn populate_categories(page: &OnlinePage, categories: &[Category]) {
    let current = page.state.category_url.borrow().clone();
    let current = current.filter(|url| categories.iter().any(|category| &category.url == url));
    *page.state.category_url.borrow_mut() = current.clone();
    page.syncing.set(true);
    page.category.remove_all();
    page.category.append(Some("all"), &tr("All wallpapers"));
    for category in categories {
        page.category.append(Some(&category.url), &category.name);
    }
    match current.as_deref() {
        Some(url) => {
            if !page.category.set_active_id(Some(url)) {
                page.category.set_active_id(Some("all"));
                *page.state.category_url.borrow_mut() = None;
            }
        }
        None => {
            page.category.set_active_id(Some("all"));
        }
    }
    page.syncing.set(false);
    *page.state.categories.borrow_mut() = categories.to_vec();
}

fn add_card(page: &OnlinePage, item: Wallpaper) {
    let slot: ThumbSlot = Rc::new(RefCell::new(None));
    let area = gtk::DrawingArea::new();
    area.set_size_request(-1, 118);
    area.set_hexpand(true);
    area.set_valign(gtk::Align::Fill);
    {
        let slot = Rc::clone(&slot);
        area.connect_draw(move |area, cr| {
            draw_video_frame(cr, area, &slot);
            glib::Propagation::Proceed
        });
    }

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.style_context().add_class("video-card");
    let frame = gtk::AspectFrame::new(None, 0.5, 0.5, 16.0 / 9.0, false);
    frame.set_halign(gtk::Align::Fill);
    frame.set_valign(gtk::Align::Start);
    frame.set_hexpand(true);
    frame.add(&area);
    card.pack_start(&frame, false, false, 0);

    let info = gtk::Box::new(gtk::Orientation::Vertical, 3);
    info.style_context().add_class("video-info");
    let name = label(&item.title, "video-name");
    name.set_ellipsize(pango::EllipsizeMode::End);
    name.set_max_width_chars(24);
    let category = label(&short_category(&item.category), "video-meta");
    category.set_ellipsize(pango::EllipsizeMode::End);
    category.set_max_width_chars(24);
    info.pack_start(&name, false, false, 0);
    info.pack_start(&category, false, false, 0);
    card.pack_start(&info, true, true, 0);

    let child = gtk::FlowBoxChild::new();
    child.set_size_request(210, -1);
    child.add(&card);
    child.set_tooltip_text(Some(&format!("{}\n{}", item.title, item.category)));
    set_a11y_name(&child, &item.title);
    page.flow.insert(&child, -1);
    child.show_all();
    page.state
        .thumbs
        .borrow_mut()
        .push(Some((slot, area.clone())));
}

fn short_category(category: &str) -> String {
    category
        .strip_prefix("Живые обои - ")
        .unwrap_or(category)
        .to_string()
}

fn start_thumbnails(page: &Rc<OnlinePage>) {
    if let Some(cancel) = page.state.thumb_cancel.borrow_mut().take() {
        cancel.store(true, Ordering::Relaxed);
    }
    if let Some(timer) = page.state.thumb_timer.borrow_mut().take() {
        timer.cancel();
    }

    let generation = page.state.thumb_generation.get();
    let items = page.state.items.borrow().clone();
    if items.is_empty() {
        return;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *page.state.thumb_cancel.borrow_mut() = Some(Arc::clone(&cancel));
    let (sender, receiver) = mpsc::channel::<(usize, PathBuf)>();
    std::thread::spawn(move || {
        for (index, item) in items.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            if let Ok(path) = catalogue::ensure_preview(&item.thumb_url) {
                if sender.send((index, path)).is_err() {
                    return;
                }
            }
        }
    });

    let done = Rc::new(Cell::new(false));
    let finished = Rc::clone(&done);
    let timer_page = Rc::clone(page);
    let id = glib::timeout_add_local(Duration::from_millis(80), move || {
        if timer_page.state.thumb_generation.get() != generation {
            finished.set(true);
            return glib::ControlFlow::Break;
        }
        loop {
            match receiver.try_recv() {
                Ok((index, path)) => {
                    if let Some(Some((slot, area))) = timer_page.state.thumbs.borrow().get(index) {
                        if let Ok(pixbuf) = Pixbuf::from_file(path) {
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
        }
    });
    *page.state.thumb_timer.borrow_mut() = Some(PendingTimer { id, done });
}

// ------------------------------------------------------------------ preview

#[derive(Clone)]
struct PreviewState {
    video: Rc<RefCell<Option<PathBuf>>>,
    installed: Rc<RefCell<Option<PathBuf>>>,
    original_name: Rc<RefCell<Option<String>>>,
    info: Rc<RefCell<Option<DownloadInfo>>>,
    still: Rc<RefCell<Option<Pixbuf>>>,
    has_frames: Rc<Cell<bool>>,
    downloading: Rc<Cell<bool>>,
    cancel: Rc<RefCell<Arc<AtomicBool>>>,
    timer: Rc<RefCell<Option<PendingTimer>>>,
    aux_timers: Rc<RefCell<Vec<PendingTimer>>>,
}

#[derive(Clone)]
struct PreviewWidgets {
    download: gtk::Button,
    install: gtk::Button,
    progress: gtk::ProgressBar,
    status: gtk::Label,
    info: gtk::Label,
    cache: gtk::Button,
    slot: ThumbSlot,
    area: gtk::DrawingArea,
}

enum DownloadEvent {
    Progress {
        done: u64,
        total: Option<u64>,
    },
    Done {
        downloaded: Downloaded,
        probe: Option<Probe>,
        sheet: Option<PathBuf>,
    },
    Error(String),
    Cancelled,
}

fn open_preview(parent: &gtk::Window, context: &Context, item: Wallpaper) {
    let dialog = gtk::Dialog::new();
    dialog.set_transient_for(Some(parent));
    dialog.set_modal(true);
    dialog.set_title(&item.title);
    dialog.set_default_size(860, 760);
    dialog.style_context().add_class("ferramari-window");

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .shadow_type(gtk::ShadowType::None)
        .vexpand(true)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_start(20);
    content.set_margin_end(20);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    scroller.add(&content);
    dialog.content_area().pack_start(&scroller, true, true, 0);

    let title = label(&item.title, "form-title");
    title.set_line_wrap(true);
    title.set_max_width_chars(72);
    content.pack_start(&title, false, false, 0);
    let category = label(&short_category(&item.category), "field-hint");
    content.pack_start(&category, false, false, 0);

    let slot: ThumbSlot = Rc::new(RefCell::new(None));
    let area = gtk::DrawingArea::new();
    {
        let slot = Rc::clone(&slot);
        area.connect_draw(move |area, cr| {
            draw_video_frame(cr, area, &slot);
            glib::Propagation::Proceed
        });
    }
    let frame = gtk::AspectFrame::new(None, 0.5, 0.5, 16.0 / 9.0, false);
    frame.set_size_request(680, 383);
    frame.set_hexpand(true);
    frame.add(&area);
    content.pack_start(&frame, false, false, 0);

    {
        let slot = Rc::clone(&slot);
        let parent = parent.clone();
        let title = item.title.clone();
        area.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
        area.connect_button_press_event(move |_, event| {
            if event.button() == 1 {
                if let Some(pixbuf) = slot.borrow().clone() {
                    show_large_image(&parent, &title, &pixbuf);
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
    }

    let info = label(&tr("Fetching details…"), "field-hint");
    info.set_line_wrap(true);
    content.pack_start(&info, false, false, 0);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    progress.set_no_show_all(true);
    progress.hide();
    content.pack_start(&progress, false, false, 0);

    let status = label("", "field-hint");
    status.set_line_wrap(true);
    content.pack_start(&status, false, false, 0);

    let links = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let cache = labeled_icon_button(
        "edit-delete-symbolic",
        &tr("Remove download"),
        &tr("Delete the cached video and frames"),
    );
    cache.hide();
    cache.set_no_show_all(true);
    links.pack_end(&cache, false, false, 0);
    content.pack_start(&links, false, false, 0);

    let close = gtk::Button::with_label(&tr("Close"));
    close.style_context().add_class("secondary");
    dialog.add_action_widget(&close, gtk::ResponseType::Close);
    let download = gtk::Button::with_label(&tr("Download"));
    download.style_context().add_class("primary");
    dialog.add_action_widget(&download, gtk::ResponseType::Other(1));
    let install = gtk::Button::with_label(&tr("Install…"));
    install.set_sensitive(false);
    dialog.add_action_widget(&install, gtk::ResponseType::Other(2));

    let state = PreviewState {
        video: Rc::new(RefCell::new(None)),
        installed: Rc::new(RefCell::new(None)),
        original_name: Rc::new(RefCell::new(None)),
        info: Rc::new(RefCell::new(None)),
        still: Rc::new(RefCell::new(None)),
        has_frames: Rc::new(Cell::new(false)),
        downloading: Rc::new(Cell::new(false)),
        cancel: Rc::new(RefCell::new(Arc::new(AtomicBool::new(false)))),
        timer: Rc::new(RefCell::new(None)),
        aux_timers: Rc::new(RefCell::new(Vec::new())),
    };
    let widgets = PreviewWidgets {
        download: download.clone(),
        install: install.clone(),
        progress: progress.clone(),
        status: status.clone(),
        info: info.clone(),
        cache: cache.clone(),
        slot: Rc::clone(&slot),
        area: area.clone(),
    };

    // Still preview and download details are fetched in parallel; frames
    // replace the still once the video is available.
    {
        let (sender, receiver) = mpsc::channel::<PathBuf>();
        let preview_url = item.preview_url.clone();
        std::thread::spawn(move || {
            if let Ok(path) = catalogue::ensure_preview(&preview_url) {
                let _ = sender.send(path);
            }
        });
        let slot = Rc::clone(&slot);
        let still = Rc::clone(&state.still);
        let has_frames = Rc::clone(&state.has_frames);
        let area = area.clone();
        let timer = poll_once(receiver, Duration::from_millis(80), move |path| {
            if has_frames.get() {
                return;
            }
            if let Ok(pixbuf) = Pixbuf::from_file(path) {
                *still.borrow_mut() = Some(pixbuf.clone());
                *slot.borrow_mut() = Some(pixbuf);
                area.queue_draw();
            }
        });
        state.aux_timers.borrow_mut().push(timer);
    }
    {
        let (sender, receiver) = mpsc::channel();
        let id = item.id.clone();
        std::thread::spawn(move || {
            let _ = sender.send(catalogue::fetch_download_info(&id));
        });
        let info = info.clone();
        let state = state.clone();
        let download = download.clone();
        let timer = poll_once(
            receiver,
            Duration::from_millis(80),
            move |result| match result {
                Ok(details) => {
                    let size = details.size.map(format_bytes).unwrap_or_else(|| {
                        details
                            .size_text
                            .split('/')
                            .next()
                            .unwrap_or(&details.size_text)
                            .trim()
                            .to_string()
                    });
                    info.set_text(&format!(
                        "{} · {}",
                        fallback_file_name(&details.file_name),
                        size
                    ));
                    if state.video.borrow().is_none() {
                        download.set_sensitive(true);
                    }
                    *state.info.borrow_mut() = Some(details);
                }
                Err(err) => {
                    info.set_text(&format!("{}: {err}", tr("Could not read file details")));
                }
            },
        );
        state.aux_timers.borrow_mut().push(timer);
    }

    if let Some(video) = catalogue::cached_video(&item.id) {
        *state.video.borrow_mut() = Some(video.clone());
        *state.original_name.borrow_mut() = catalogue::cached_original_name(&item.id);
        install.set_sensitive(true);
        download.set_label(&tr("Downloaded"));
        download.set_sensitive(false);
        show_cache(&cache);
        load_cached_frames(
            &state,
            &widgets,
            video,
            item.id.clone(),
            &tr("Loading frames…"),
        );
    }

    {
        let state = state.clone();
        let widgets = widgets.clone();
        let id = item.id.clone();
        download.connect_clicked(move |_| {
            if state.downloading.get() {
                state.cancel.borrow().store(true, Ordering::Relaxed);
                widgets.download.set_sensitive(false);
                widgets.download.set_label(&tr("Cancelling…"));
                widgets.status.set_text(&tr("Cancelling…"));
                return;
            }
            start_download(&state, &widgets, &id);
        });
    }

    {
        let state = state.clone();
        let widgets = widgets.clone();
        let context = context.clone();
        let id = item.id.clone();
        install.connect_clicked(move |button| {
            if let Some(path) = state.installed.borrow().clone() {
                (context.apply)(button, path.to_string_lossy().into_owned());
                return;
            }
            let Some(video) = state.video.borrow().clone() else {
                return;
            };
            let original = state
                .original_name
                .borrow()
                .clone()
                .or_else(|| {
                    state
                        .info
                        .borrow()
                        .as_ref()
                        .map(|info| info.file_name.clone())
                })
                .unwrap_or_else(|| format!("wallpaper-{id}.mp4"));
            let extension = video
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("mp4");
            let name = catalogue::library_name(&original, &id, extension);
            match catalogue::install(&video, &name) {
                Ok(path) => {
                    (context.refresh_library)();
                    *state.installed.borrow_mut() = Some(path.clone());
                    button.set_label(&tr("Apply…"));
                    widgets
                        .status
                        .set_text(&format!("{} {}", tr("Installed to"), path.display()));
                    (context.apply)(button, path.to_string_lossy().into_owned());
                }
                Err(err) => widgets
                    .status
                    .set_text(&format!("{}: {err}", tr("Could not install"))),
            }
        });
    }

    {
        let state = state.clone();
        let widgets = widgets.clone();
        let id = item.id.clone();
        cache.connect_clicked(move |_| {
            catalogue::clear_cached(&id);
            *state.video.borrow_mut() = None;
            *state.original_name.borrow_mut() = None;
            state.has_frames.set(false);
            if let Some(still) = state.still.borrow().clone() {
                *widgets.slot.borrow_mut() = Some(still);
                widgets.area.queue_draw();
            }
            widgets
                .info
                .set_text(&tr("Download removed from the cache."));
            widgets.cache.hide();
            widgets.progress.hide();
            widgets.status.set_text(&tr(
                "The cached video and frames were deleted. Installed wallpapers are not touched.",
            ));
            widgets.download.set_label(&tr("Download"));
            widgets.download.set_sensitive(true);
            if state.installed.borrow().is_some() {
                widgets.install.set_label(&tr("Apply…"));
                widgets.install.set_sensitive(true);
            } else {
                widgets.install.set_label(&tr("Install…"));
                widgets.install.set_sensitive(false);
            }
        });
    }

    {
        let state = state.clone();
        dialog.connect_destroy(move |_| {
            state.cancel.borrow().store(true, Ordering::Relaxed);
            if let Some(timer) = state.timer.borrow_mut().take() {
                timer.cancel();
            }
            for timer in state.aux_timers.borrow_mut().drain(..) {
                timer.cancel();
            }
        });
    }
    dialog.connect_response(|dialog, response| {
        if response == gtk::ResponseType::Close || response == gtk::ResponseType::DeleteEvent {
            dialog.close();
        }
    });

    dialog.show_all();
    // `show_all` does not reveal widgets marked no-show-all, but explicit
    // show calls are honoured.
    progress.hide();
    if state.video.borrow().is_some() {
        show_cache(&cache);
    }
}

/// Reveal a button that was created hidden with `no_show_all`; its children
/// would otherwise stay invisible because `show_all` skips them.
fn show_cache(button: &gtk::Button) {
    button.show();
    button.foreach(|child| child.show_all());
}

fn fallback_file_name(name: &str) -> String {
    if name.trim().is_empty() {
        tr("Wallpaper archive")
    } else {
        name.to_string()
    }
}

fn load_cached_frames(
    state: &PreviewState,
    widgets: &PreviewWidgets,
    video: PathBuf,
    id: String,
    status: &str,
) {
    widgets.status.set_text(status);
    let (sender, receiver) = mpsc::channel::<(Option<Probe>, Option<PathBuf>, Option<u64>)>();
    std::thread::spawn(move || {
        let probe = catalogue::probe(&video);
        let sheet = catalogue::ensure_sheet(&video, &id);
        let size = std::fs::metadata(&video)
            .ok()
            .map(|metadata| metadata.len());
        let _ = sender.send((probe, sheet, size));
    });
    let state = state.clone();
    let widgets = widgets.clone();
    let timer = poll_once(
        receiver,
        Duration::from_millis(100),
        move |(probe, sheet, size)| {
            widgets.info.set_text(&probe_text(probe.as_ref(), size));
            if let Some(sheet) = sheet {
                if let Ok(pixbuf) = Pixbuf::from_file(sheet) {
                    state.has_frames.set(true);
                    *widgets.slot.borrow_mut() = Some(pixbuf);
                    widgets.area.queue_draw();
                }
            }
            widgets
                .status
                .set_text(&tr("Downloaded. Check the frames, then install."));
        },
    );
    state.aux_timers.borrow_mut().push(timer);
}

fn start_download(state: &PreviewState, widgets: &PreviewWidgets, id: &str) {
    if state.downloading.get() {
        return;
    }
    if let Some(timer) = state.timer.borrow_mut().take() {
        timer.cancel();
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *state.cancel.borrow_mut() = Arc::clone(&cancel);
    state.downloading.set(true);
    widgets.download.set_label(&tr("Cancel"));
    widgets.download.set_sensitive(true);
    widgets.install.set_sensitive(false);
    widgets.progress.set_fraction(0.0);
    widgets.progress.set_text(Some(&tr("Starting…")));
    widgets.progress.show();
    widgets.status.set_text(&tr("Downloading the wallpaper…"));
    widgets.cache.set_sensitive(false);

    let (sender, receiver) = mpsc::channel();
    let thread_id = id.to_string();
    std::thread::spawn(move || {
        let mut progress = |done, total| {
            let _ = sender.send(DownloadEvent::Progress { done, total });
        };
        match catalogue::download_video(&thread_id, &cancel, &mut progress) {
            Ok(downloaded) => {
                let probe = catalogue::probe(&downloaded.video);
                let sheet = catalogue::ensure_sheet(&downloaded.video, &thread_id);
                let _ = sender.send(DownloadEvent::Done {
                    downloaded,
                    probe,
                    sheet,
                });
            }
            Err(err) if err.to_string().contains(catalogue::CANCELLED) => {
                let _ = sender.send(DownloadEvent::Cancelled);
            }
            Err(err) => {
                let _ = sender.send(DownloadEvent::Error(err.to_string()));
            }
        }
    });

    let state = state.clone();
    let widgets = widgets.clone();
    let done = Rc::new(Cell::new(false));
    let finished = Rc::clone(&done);
    let id = glib::timeout_add_local(Duration::from_millis(100), move || loop {
        match receiver.try_recv() {
            Ok(DownloadEvent::Progress { done, total }) => {
                if let Some(total) = total.filter(|total| *total > 0) {
                    widgets
                        .progress
                        .set_fraction((done as f64 / total as f64).clamp(0.0, 1.0));
                    widgets.progress.set_text(Some(&format!(
                        "{} / {}",
                        format_bytes(done),
                        format_bytes(total)
                    )));
                } else {
                    widgets.progress.pulse();
                    widgets.progress.set_text(Some(&format_bytes(done)));
                }
            }
            Ok(DownloadEvent::Done {
                downloaded,
                probe,
                sheet,
            }) => {
                state.downloading.set(false);
                state.cancel.borrow_mut().store(true, Ordering::Relaxed);
                let size = std::fs::metadata(&downloaded.video)
                    .ok()
                    .map(|metadata| metadata.len());
                *state.video.borrow_mut() = Some(downloaded.video.clone());
                *state.original_name.borrow_mut() = Some(downloaded.original_name);
                widgets.info.set_text(&probe_text(probe.as_ref(), size));
                widgets.progress.hide();
                widgets.download.set_label(&tr("Downloaded"));
                widgets.download.set_sensitive(false);
                widgets.install.set_label(&tr("Install…"));
                widgets.install.set_sensitive(true);
                widgets.cache.set_sensitive(true);
                show_cache(&widgets.cache);
                widgets
                    .status
                    .set_text(&tr("Downloaded. Check the frames, then install."));
                if let Some(sheet) = sheet {
                    if let Ok(pixbuf) = Pixbuf::from_file(sheet) {
                        state.has_frames.set(true);
                        *widgets.slot.borrow_mut() = Some(pixbuf);
                        widgets.area.queue_draw();
                    }
                } else {
                    widgets.status.set_text(&tr(
                        "Downloaded, but the preview frames could not be generated.",
                    ));
                }
                finished.set(true);
                return glib::ControlFlow::Break;
            }
            Ok(DownloadEvent::Error(err)) => {
                state.downloading.set(false);
                state.cancel.borrow_mut().store(true, Ordering::Relaxed);
                widgets.progress.hide();
                widgets.download.set_label(&tr("Retry download"));
                widgets.download.set_sensitive(true);
                widgets.cache.set_sensitive(true);
                widgets
                    .status
                    .set_text(&format!("{}: {err}", tr("Download failed")));
                finished.set(true);
                return glib::ControlFlow::Break;
            }
            Ok(DownloadEvent::Cancelled) => {
                state.downloading.set(false);
                widgets.progress.hide();
                widgets.download.set_label(&tr("Download"));
                widgets.download.set_sensitive(true);
                widgets.cache.set_sensitive(true);
                widgets.status.set_text(&tr("Download cancelled."));
                finished.set(true);
                return glib::ControlFlow::Break;
            }
            Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                state.downloading.set(false);
                finished.set(true);
                return glib::ControlFlow::Break;
            }
        }
    });
    *state.timer.borrow_mut() = Some(PendingTimer { id, done });
}

fn probe_text(probe: Option<&Probe>, size: Option<u64>) -> String {
    let mut parts = Vec::new();
    if let Some(probe) = probe {
        parts.push(format!("{}×{}", probe.width, probe.height));
        if let Some(fps) = probe.fps.filter(|fps| *fps > 0.0) {
            if (fps - fps.round()).abs() < 0.05 {
                parts.push(format!("{:.0} fps", fps));
            } else {
                parts.push(format!("{fps:.2} fps"));
            }
        }
        if let Some(duration) = probe.duration.filter(|duration| *duration > 0.0) {
            parts.push(format!("{duration:.1} s"));
        }
    }
    if let Some(size) = size.filter(|size| *size > 0) {
        parts.push(format_bytes(size));
    }
    if parts.is_empty() {
        tr("Video downloaded")
    } else {
        parts.join(" · ")
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.0} KB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

fn show_large_image(parent: &gtk::Window, title: &str, pixbuf: &Pixbuf) {
    let dialog = gtk::Dialog::new();
    dialog.set_transient_for(Some(parent));
    dialog.set_modal(true);
    dialog.set_title(title);
    dialog.set_default_size(1180, 720);
    dialog.style_context().add_class("ferramari-window");

    let slot: ThumbSlot = Rc::new(RefCell::new(Some(pixbuf.clone())));
    let area = gtk::DrawingArea::new();
    area.connect_draw(move |area, cr| {
        draw_video_frame(cr, area, &slot);
        glib::Propagation::Proceed
    });
    let frame = gtk::AspectFrame::new(None, 0.5, 0.5, 16.0 / 9.0, false);
    frame.set_margin_start(18);
    frame.set_margin_end(18);
    frame.set_margin_top(18);
    frame.set_margin_bottom(18);
    frame.add(&area);
    dialog.content_area().pack_start(&frame, true, true, 0);
    dialog.add_button(&tr("Close"), gtk::ResponseType::Close);
    dialog.connect_response(|dialog, response| {
        if response == gtk::ResponseType::Close || response == gtk::ResponseType::DeleteEvent {
            dialog.close();
        }
    });
    dialog.show_all();
}
