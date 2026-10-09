use crate::config::Appearance;
use crate::content::ClipContent;
use crate::picker::css;
use crate::picker::paste::{self, Target};
use crate::picker::preview::{self, Kind, MarkupStyle};
use crate::storage::{self, ClipEntry};
use crate::theme::{self, Palette};

use gdk4::gdk_pixbuf::Pixbuf;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

/// Thumbnail size on screen (rendered at 2× for HiDPI).
const THUMB_W: i32 = 60;
const THUMB_H: i32 = 44;

pub fn run(auto_paste: bool, target_is_terminal: bool, target: Target, appearance: Appearance) {
    let app = gtk4::Application::builder()
        .application_id("com.copyninja.picker")
        .build();

    let picker: Rc<RefCell<Option<Rc<Picker>>>> = Rc::default();
    app.connect_activate(move |app| {
        // Pressing the hotkey again activates this running instance: show the
        // existing window instead of building a second one.
        if let Some(existing) = picker.borrow().as_ref() {
            if !existing.is_pasting.get() {
                existing.window.present();
                existing.search.grab_focus();
            }
            return;
        }
        let built = Picker::build(
            app,
            auto_paste,
            target_is_terminal,
            target.clone(),
            &appearance,
        );
        *picker.borrow_mut() = Some(built);
    });
    app.run_with_args::<String>(&[]);
}

/// A label whose markup is re-rendered as the search changes.
struct Highlighted {
    label: gtk4::Label,
    source: String,
    /// Text clip previews jump to a match further in; titles don't.
    snippet: bool,
}

impl Highlighted {
    fn render(&self, terms: &[String], style: &MarkupStyle) {
        let markup = if self.snippet {
            preview::markup(&self.source, terms, style)
        } else {
            preview::highlight(&self.source, terms, style)
        };
        self.label.set_markup(&markup);
    }
}

/// One clip in the list.
struct Row {
    entry: ClipEntry,
    row: gtk4::ListBoxRow,
    /// Lowercased full text (or image description) that search matches against.
    search_key: String,
    labels: Vec<Highlighted>,
}

struct Picker {
    app: gtk4::Application,
    window: gtk4::ApplicationWindow,
    search: gtk4::SearchEntry,
    list: gtk4::ListBox,
    scrolled: gtk4::ScrolledWindow,
    empty_title: gtk4::Label,
    empty_body: gtk4::Label,
    hints: gtk4::Box,
    status: gtk4::Label,
    clear_btn: gtk4::Button,
    palette: Palette,
    style: MarkupStyle,

    rows: RefCell<HashMap<String, Row>>,
    /// Lowercased search terms; a clip must contain all of them.
    terms: RefCell<Vec<String>>,
    count: Cell<usize>,
    /// Bumped on every click so stale timers don't reset a newer confirmation.
    clear_generation: Cell<u32>,
    clear_pending: Cell<bool>,
    status_generation: Cell<u32>,
    /// Set while pasting, so focus loss doesn't quit mid-paste.
    is_pasting: Cell<bool>,

    auto_paste: bool,
    target_is_terminal: bool,
    target: Target,
}

impl Picker {
    fn build(
        app: &gtk4::Application,
        auto_paste: bool,
        target_is_terminal: bool,
        target: Target,
        appearance: &Appearance,
    ) -> Rc<Self> {
        let palette = install_theme(appearance);

        let window = gtk4::ApplicationWindow::builder()
            .application(app)
            .title("Clipboard")
            .default_width(appearance.width as i32)
            .default_height(appearance.height as i32)
            .resizable(false)
            .build();
        window.add_css_class("copyninja");

        // -- Search: it doubles as the title bar (drag its edge to move the window) --
        let search = gtk4::SearchEntry::new();
        search.add_css_class("search-field");
        search.set_hexpand(true);
        let topbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        topbar.add_css_class("topbar");
        topbar.append(&search);
        let handle = gtk4::WindowHandle::new();
        handle.set_child(Some(&topbar));
        window.set_titlebar(Some(&handle));

        // -- Clip list, with a placeholder for "empty" and "no matches" --
        let list = gtk4::ListBox::new();
        list.add_css_class("clips");
        list.set_selection_mode(gtk4::SelectionMode::Single);

        let empty_icon = gtk4::Image::from_icon_name("edit-paste-symbolic");
        empty_icon.set_pixel_size(40);
        empty_icon.add_css_class("empty-icon");
        let empty_title = gtk4::Label::new(None);
        empty_title.add_css_class("empty-title");
        empty_title.set_wrap(true);
        empty_title.set_justify(gtk4::Justification::Center);
        let empty_body = gtk4::Label::new(None);
        empty_body.add_css_class("empty-body");
        empty_body.set_wrap(true);
        empty_body.set_max_width_chars(36);
        empty_body.set_justify(gtk4::Justification::Center);
        let placeholder = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        placeholder.set_margin_top(110);
        placeholder.set_margin_start(24);
        placeholder.set_margin_end(24);
        placeholder.append(&empty_icon);
        placeholder.append(&empty_title);
        placeholder.append(&empty_body);
        list.set_placeholder(Some(&placeholder));

        let scrolled = gtk4::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .build();

        // -- Footer: keyboard hints (or a short status message) and "Clear history" --
        let hints = gtk4::Box::new(gtk4::Orientation::Horizontal, 14);
        for (key, action) in [("Enter", "Paste"), ("Ctrl+P", "Pin"), ("Ctrl+D", "Delete")] {
            let pair = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
            let key_label = gtk4::Label::new(Some(key));
            key_label.add_css_class("key");
            let action_label = gtk4::Label::new(Some(action));
            action_label.add_css_class("hint");
            pair.append(&key_label);
            pair.append(&action_label);
            hints.append(&pair);
        }
        let status = gtk4::Label::new(None);
        status.add_css_class("status");
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.set_hexpand(true);
        status.set_visible(false);
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let clear_btn = gtk4::Button::with_label("Clear history");
        clear_btn.add_css_class("clear");
        clear_btn.set_tooltip_text(Some("Removes every clip except pinned ones (Ctrl+L)"));
        clear_btn.set_focus_on_click(false);

        let footer = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        footer.add_css_class("footer");
        footer.append(&hints);
        footer.append(&status);
        footer.append(&spacer);
        footer.append(&clear_btn);

        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        vbox.append(&scrolled);
        vbox.append(&footer);
        window.set_child(Some(&vbox));

        let style = MarkupStyle::new(&palette.muted, &palette.accent);
        let picker = Rc::new(Self {
            app: app.clone(),
            window,
            search,
            list,
            scrolled,
            empty_title,
            empty_body,
            hints,
            status,
            clear_btn,
            palette,
            style,
            rows: RefCell::default(),
            terms: RefCell::default(),
            count: Cell::new(0),
            clear_generation: Cell::new(0),
            clear_pending: Cell::new(false),
            status_generation: Cell::new(0),
            is_pasting: Cell::new(false),
            auto_paste,
            target_is_terminal,
            target,
        });
        picker.connect_signals();
        picker.populate(None, None);

        picker.window.present();
        picker.search.grab_focus();
        picker
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.list
            .set_filter_func(move |row| weak.upgrade().is_none_or(|p| p.row_visible(row)));

        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.on_search_changed();
            }
        });

        // Click (or Enter on a row): copy + paste
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let (Some(p), Some(hash)) = (weak.upgrade(), row_hash(row)) {
                p.paste(&hash);
            }
        });

        let weak = Rc::downgrade(self);
        self.clear_btn.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.on_clear_clicked();
            }
        });

        // Keyboard: handled before the search field sees the keys, so typing
        // always searches while arrows, Enter and shortcuts drive the list.
        let keys = gtk4::EventControllerKey::new();
        keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| match weak.upgrade() {
            Some(p) => p.on_key(key, state),
            None => glib::Propagation::Proceed,
        });
        self.window.add_controller(keys);

        // -- Focus-loss close (with 500ms grace period) --
        let focus_close_enabled = Rc::new(Cell::new(false));
        let enable = focus_close_enabled.clone();
        glib::timeout_add_local_once(Duration::from_millis(500), move || enable.set(true));

        let weak = Rc::downgrade(self);
        self.window.connect_is_active_notify(move |win| {
            let Some(p) = weak.upgrade() else { return };
            if !win.is_active() && focus_close_enabled.get() && !p.is_pasting.get() {
                let win = win.clone();
                let app = p.app.clone();
                glib::timeout_add_local_once(Duration::from_millis(150), move || {
                    if !win.is_active() {
                        app.quit();
                    }
                });
            }
        });
    }

    // ── List ──

    /// Rebuild the list from the history file. Selects the clip with
    /// `keep_hash`, else the visible clip at `keep_index`, else the first one.
    fn populate(self: &Rc<Self>, keep_hash: Option<&str>, keep_index: Option<usize>) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }

        let history = storage::load_history();
        self.count.set(history.len());
        let placeholder = match history.len() {
            0 => "Search clips".to_string(),
            1 => "Search 1 clip".to_string(),
            n => format!("Search {} clips", n),
        };
        self.search.set_placeholder_text(Some(&placeholder));

        let now = storage::now();
        let (pinned, recent): (Vec<_>, Vec<_>) = history.into_iter().partition(|e| e.pinned);
        let mut rows = HashMap::new();
        let show_sections = !pinned.is_empty();
        for (title, group) in [("Pinned", pinned), ("Recent", recent)] {
            if show_sections && !group.is_empty() {
                self.list.append(&section_row(title));
            }
            for entry in group {
                let row = self.build_row(entry, now);
                self.list.append(&row.row);
                rows.insert(row.entry.hash.clone(), row);
            }
        }
        *self.rows.borrow_mut() = rows;

        self.list.invalidate_filter();
        self.update_empty_state();

        let kept = keep_hash
            .and_then(|hash| self.rows.borrow().get(hash).map(|r| r.row.clone()))
            .filter(|row| row.is_child_visible())
            .or_else(|| {
                let visible = self.visible_rows();
                keep_index
                    .and_then(|i| visible.get(i.min(visible.len().saturating_sub(1))).cloned())
            });
        match kept {
            Some(row) => self.list.select_row(Some(&row)),
            None => self.select_first(),
        }
    }

    fn build_row(self: &Rc<Self>, entry: ClipEntry, now: f64) -> Row {
        let row = gtk4::ListBoxRow::new();
        row.set_widget_name(&format!("entry:{}", entry.hash));
        row.add_css_class("clip");
        // Keep keyboard focus in the search field; the list is driven from there.
        row.set_focusable(false);

        let body = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        body.add_css_class("clip-body");
        let main = gtk4::Box::new(gtk4::Orientation::Vertical, 3);
        main.set_hexpand(true);
        main.set_valign(gtk4::Align::Center);

        let mut labels = Vec::new();
        let mut add_label = |parent: &gtk4::Box, class: &str, source: String, snippet: bool| {
            let label = gtk4::Label::new(None);
            label.add_css_class(class);
            label.set_xalign(0.0);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_max_width_chars(46);
            parent.append(&label);
            let highlighted = Highlighted {
                label: label.clone(),
                source,
                snippet,
            };
            highlighted.render(&self.terms.borrow(), &self.style);
            labels.push(highlighted);
            label
        };

        let search_key = match entry.resolved_content() {
            ClipContent::Text { text, .. } => {
                match preview::classify(&text) {
                    // A link reads like a bookmark: the site, then the full address.
                    Kind::Link { host } => {
                        add_label(&main, "clip-title", host, false);
                        add_label(&main, "clip-sub", text.trim().to_string(), false);
                    }
                    // A color code gets a swatch of the color itself.
                    Kind::Color { rgb } => {
                        let line = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
                        line.append(&swatch(rgb, theme::rgb(&self.palette.surface_strong)));
                        let texts = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
                        texts.set_valign(gtk4::Align::Center);
                        add_label(&texts, "clip-text", text.trim().to_string(), false);
                        let (r, g, b) = rgb;
                        texts.append(&small_label(
                            &format!("rgb({}, {}, {})", r, g, b),
                            "clip-meta",
                        ));
                        line.append(&texts);
                        main.append(&line);
                    }
                    // File paths: what the files are called, then where they are.
                    Kind::Files { names, folder } => {
                        let title = match names.len() {
                            1..=3 => names.join(", "),
                            n => format!("{} and {} more", names[..2].join(", "), n - 2),
                        };
                        add_label(&main, "clip-title", title, false);
                        add_label(&main, "clip-sub", folder, false);
                    }
                    Kind::Text => {
                        let label = add_label(&main, "clip-text", text.clone(), true);
                        label.set_wrap(true);
                        label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
                        label.set_lines(2);
                        if let Some(meta) = preview::text_meta(&text) {
                            main.append(&small_label(&meta, "clip-meta"));
                        }
                    }
                }
                text.to_lowercase()
            }
            ClipContent::Image { path, mime } => {
                body.append(&thumbnail_widget(&entry.hash, &path));
                let (title, meta) = image_labels(&path, &mime);
                main.append(&small_label(&title, "clip-title"));
                main.append(&small_label(&meta, "clip-meta"));
                format!("image {} {}", title, meta).to_lowercase()
            }
        };
        body.append(&main);
        body.append(&self.build_side(&row, &entry, now));

        row.set_child(Some(&body));
        Row {
            entry,
            row,
            search_key,
            labels,
        }
    }

    /// The right side of a row: when it was copied (and a pin mark), swapped
    /// for the pin and delete buttons while the pointer is over the row.
    fn build_side(
        self: &Rc<Self>,
        row: &gtk4::ListBoxRow,
        entry: &ClipEntry,
        now: f64,
    ) -> gtk4::Stack {
        let info = gtk4::Box::new(gtk4::Orientation::Horizontal, 5);
        info.set_halign(gtk4::Align::End);
        info.set_valign(gtk4::Align::Center);
        if entry.pinned {
            let mark = gtk4::Image::from_icon_name("view-pin-symbolic");
            mark.set_pixel_size(12);
            mark.add_css_class("pin-mark");
            info.append(&mark);
        }
        let time = small_label(&preview::relative_time(entry.time, now), "clip-time");
        if let Ok(when) = glib::DateTime::from_unix_local(entry.time as i64)
            .and_then(|d| d.format("Copied %e %b %Y, %H:%M"))
        {
            info.set_tooltip_text(Some(when.trim()));
        }
        info.append(&time);

        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
        actions.set_halign(gtk4::Align::End);
        actions.set_valign(gtk4::Align::Center);
        let (pin_tip, pin_class) = if entry.pinned {
            ("Unpin (Ctrl+P)", Some("pinned"))
        } else {
            ("Pin (Ctrl+P)", None)
        };
        let pin_btn = action_button("view-pin-symbolic", pin_tip, pin_class);
        let delete_btn = action_button("user-trash-symbolic", "Delete (Ctrl+D)", Some("delete"));
        let (weak, hash) = (Rc::downgrade(self), entry.hash.clone());
        pin_btn.connect_clicked(move |_| with(&weak, |p| p.toggle_pin(&hash)));
        let (weak, hash) = (Rc::downgrade(self), entry.hash.clone());
        delete_btn.connect_clicked(move |_| with(&weak, |p| p.delete(&hash)));
        actions.append(&pin_btn);
        actions.append(&delete_btn);

        let side = gtk4::Stack::new();
        side.set_transition_type(gtk4::StackTransitionType::Crossfade);
        side.set_transition_duration(120);
        side.set_valign(gtk4::Align::Center);
        side.add_named(&info, Some("info"));
        side.add_named(&actions, Some("actions"));

        let hover = gtk4::EventControllerMotion::new();
        let stack = side.clone();
        hover.connect_enter(move |_, _, _| stack.set_visible_child_name("actions"));
        let stack = side.clone();
        hover.connect_leave(move |_| stack.set_visible_child_name("info"));
        row.add_controller(hover);
        side
    }

    fn row_visible(&self, row: &gtk4::ListBoxRow) -> bool {
        let terms = self.terms.borrow();
        if terms.is_empty() {
            return true;
        }
        // Section headers hide while searching.
        let Some(hash) = row_hash(row) else {
            return false;
        };
        self.rows
            .borrow()
            .get(&hash)
            .is_some_and(|r| terms.iter().all(|t| r.search_key.contains(t.as_str())))
    }

    /// Clip rows currently shown, in order.
    fn visible_rows(&self) -> Vec<gtk4::ListBoxRow> {
        let mut rows = Vec::new();
        let mut i = 0;
        while let Some(row) = self.list.row_at_index(i) {
            if row.is_selectable() && row.is_child_visible() {
                rows.push(row);
            }
            i += 1;
        }
        rows
    }

    fn selected_hash(&self) -> Option<String> {
        self.list
            .selected_row()
            .filter(|row| row.is_child_visible())
            .and_then(|row| row_hash(&row))
    }

    fn select_first(&self) {
        let first = self.visible_rows().into_iter().next();
        self.list.select_row(first.as_ref());
    }

    fn move_selection(&self, delta: i32) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return;
        }
        let current = self
            .list
            .selected_row()
            .and_then(|sel| rows.iter().position(|r| *r == sel));
        let next = match current {
            Some(i) => (i as i32 + delta).clamp(0, rows.len() as i32 - 1) as usize,
            None => 0,
        };
        self.list.select_row(Some(&rows[next]));
        self.scroll_to(&rows[next]);
    }

    fn scroll_to(&self, row: &gtk4::ListBoxRow) {
        let Some(bounds) = row.compute_bounds(&self.list) else {
            return;
        };
        let adj = self.scrolled.vadjustment();
        let (top, bottom) = (bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
        if top < adj.value() {
            adj.set_value(top);
        } else if bottom > adj.value() + adj.page_size() {
            adj.set_value(bottom - adj.page_size());
        }
    }

    // ── Search ──

    fn on_search_changed(&self) {
        *self.terms.borrow_mut() = self
            .search
            .text()
            .to_lowercase()
            .split_whitespace()
            .map(str::to_string)
            .collect();

        // Re-render previews so matches are highlighted (and in view).
        let terms = self.terms.borrow();
        for row in self.rows.borrow().values() {
            for label in &row.labels {
                label.render(&terms, &self.style);
            }
        }
        drop(terms);

        self.list.invalidate_filter();
        self.update_empty_state();
        self.select_first();
        self.scrolled.vadjustment().set_value(0.0);
    }

    fn update_empty_state(&self) {
        if self.count.get() == 0 {
            self.empty_title.set_text("Nothing copied yet");
            self.empty_body
                .set_text("Copy some text or an image and it will show up here.");
        } else {
            let query = self.search.text();
            self.empty_title
                .set_text(&format!("No clips match \u{201c}{}\u{201d}", query.trim()));
            self.empty_body
                .set_text("Check the spelling, or search for fewer words.");
        }
    }

    // ── Keyboard ──

    fn on_key(self: &Rc<Self>, key: gdk4::Key, state: gdk4::ModifierType) -> glib::Propagation {
        use gdk4::Key;
        let ctrl = state.contains(gdk4::ModifierType::CONTROL_MASK);
        match key {
            Key::Escape => {
                if self.search.text().is_empty() {
                    self.app.quit();
                } else {
                    self.search.set_text("");
                }
            }
            Key::Down | Key::KP_Down => self.move_selection(1),
            Key::Up | Key::KP_Up => self.move_selection(-1),
            Key::Page_Down | Key::KP_Page_Down => self.move_selection(5),
            Key::Page_Up | Key::KP_Page_Up => self.move_selection(-5),
            Key::Return | Key::KP_Enter | Key::ISO_Enter => {
                // Let a focused button (Tab to "Clear history") handle its own Enter.
                if gtk4::prelude::GtkWindowExt::focus(&self.window)
                    .is_some_and(|w| w.is::<gtk4::Button>())
                {
                    return glib::Propagation::Proceed;
                }
                if let Some(hash) = self.selected_hash() {
                    self.paste(&hash);
                }
            }
            Key::p | Key::P if ctrl => {
                if let Some(hash) = self.selected_hash() {
                    self.toggle_pin(&hash);
                }
            }
            Key::d | Key::D if ctrl => {
                if let Some(hash) = self.selected_hash() {
                    self.delete(&hash);
                }
            }
            Key::l | Key::L if ctrl => self.on_clear_clicked(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    // ── Actions ──

    fn paste(self: &Rc<Self>, hash: &str) {
        let Some(entry) = self.rows.borrow().get(hash).map(|r| r.entry.clone()) else {
            return;
        };

        // Write to clipboard based on content type.
        // For images, force non-terminal paste mode: Ctrl+Shift+V never pastes
        // images in any common app (terminals don't accept images, GTK apps
        // treat it as Unicode entry, image editors ignore it). Ctrl+V is the
        // only shortcut that actually pastes image data.
        let (text_for_paste, paste_as_terminal) = match entry.resolved_content() {
            ClipContent::Text { text, .. } => {
                if !paste::write_clipboard_sync(&text) {
                    if let Some(display) = gdk4::Display::default() {
                        display.clipboard().set_text(&text);
                    }
                }
                (text, self.target_is_terminal)
            }
            ClipContent::Image { path, mime } => {
                // Pasting now would paste whatever was on the clipboard before.
                if !paste::write_image_clipboard_sync(&path, &mime) {
                    self.show_status(
                        "This image's file is missing. Delete the clip and copy the image again.",
                    );
                    return;
                }
                (String::new(), false)
            }
        };

        self.is_pasting.set(true);
        self.window.set_visible(false);

        if !self.auto_paste {
            // Auto-paste disabled — just quit after copying to clipboard
            self.app.quit();
            return;
        }

        // Hold the application alive while we paste (guard dropped on quit)
        let guard = self.app.upcast_ref::<gio::Application>().hold();

        // Paste in a background thread — simulate_paste polls for focus
        // loss internally, so no fixed delay needed here.
        let target = self.target.clone();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            paste::simulate_paste(&text_for_paste, paste_as_terminal, &target);
            let _ = tx.send(());
        });
        // Poll from GLib main loop until paste thread completes (or dies)
        let app = self.app.clone();
        let mut guard = Some(guard);
        glib::timeout_add_local(Duration::from_millis(50), move || {
            if let Err(TryRecvError::Empty) = rx.try_recv() {
                return glib::ControlFlow::Continue;
            }
            let app = app.clone();
            let guard = guard.take();
            glib::timeout_add_local_once(Duration::from_millis(200), move || {
                drop(guard);
                app.quit();
            });
            glib::ControlFlow::Break
        });
    }

    fn toggle_pin(self: &Rc<Self>, hash: &str) {
        let pinned = storage::global().toggle_pin(hash);
        self.populate(Some(hash), None);
        if let Some(pinned) = pinned {
            self.show_status(if pinned {
                "Pinned. Pinned clips are never cleared."
            } else {
                "Unpinned."
            });
        }
    }

    fn delete(self: &Rc<Self>, hash: &str) {
        let index = self
            .visible_rows()
            .iter()
            .position(|r| row_hash(r).as_deref() == Some(hash));
        storage::global().delete(hash);
        self.populate(None, index);
    }

    /// "Clear history" asks for a second click within 3 seconds.
    fn on_clear_clicked(self: &Rc<Self>) {
        let unpinned = self
            .rows
            .borrow()
            .values()
            .filter(|r| !r.entry.pinned)
            .count();
        if unpinned == 0 {
            self.show_status("Nothing to clear. Pinned clips are kept.");
            return;
        }

        let generation = self.clear_generation.get() + 1;
        self.clear_generation.set(generation);

        if self.clear_pending.get() {
            self.clear_pending.set(false);
            self.reset_clear_button();
            storage::global().clear_unpinned();
            self.populate(None, None);
            return;
        }

        self.clear_pending.set(true);
        self.clear_btn.set_label(&match unpinned {
            1 => "Clear 1 clip".to_string(),
            n => format!("Clear {} clips", n),
        });
        self.clear_btn.add_css_class("confirm");
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(3), move || {
            with(&weak, |p| {
                if p.clear_generation.get() == generation && p.clear_pending.get() {
                    p.clear_pending.set(false);
                    p.reset_clear_button();
                }
            })
        });
    }

    fn reset_clear_button(&self) {
        self.clear_btn.set_label("Clear history");
        self.clear_btn.remove_css_class("confirm");
    }

    /// Replace the footer hints with a message for a few seconds.
    fn show_status(self: &Rc<Self>, message: &str) {
        let generation = self.status_generation.get() + 1;
        self.status_generation.set(generation);
        self.status.set_text(message);
        self.status.set_visible(true);
        self.hints.set_visible(false);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(3), move || {
            with(&weak, |p| {
                if p.status_generation.get() == generation {
                    p.status.set_visible(false);
                    p.hints.set_visible(true);
                }
            })
        });
    }
}

// ── Theme ──

/// Load the configured theme, then the user's own `style.css` on top of it.
fn install_theme(appearance: &Appearance) -> Palette {
    let palette = theme::resolve(appearance, system_prefers_dark);
    let display = gdk4::Display::default().expect("Could not get default display");

    let provider = gtk4::CssProvider::new();
    provider.load_from_data(&css::stylesheet(&palette, appearance));
    gtk4::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let user_css = dirs::config_dir().map(|d| d.join("copyninja").join("style.css"));
    if let Some(path) = user_css.filter(|p| p.exists()) {
        let user = gtk4::CssProvider::new();
        user.connect_parsing_error(|_, section, error| {
            log::warn!("style.css: {} ({})", error, section.to_str());
        });
        user.load_from_path(&path);
        gtk4::style_context_add_provider_for_display(
            &display,
            &user,
            gtk4::STYLE_PROVIDER_PRIORITY_USER,
        );
    }
    palette
}

/// The desktop's light/dark preference, for `theme = "auto"`.
fn system_prefers_dark() -> Option<bool> {
    portal_color_scheme()
        .or_else(gtk_prefers_dark)
        .or_else(gnome_color_scheme)
}

/// `color-scheme` from the XDG settings portal: 1 means dark, 2 light.
fn portal_color_scheme() -> Option<bool> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
    let read = |method: &str| {
        bus.call_sync(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
            method,
            Some(&("org.freedesktop.appearance", "color-scheme").to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            300,
            gio::Cancellable::NONE,
        )
        .ok()
    };
    let reply = read("ReadOne").or_else(|| read("Read"))?;
    // ReadOne returns (v); the older Read wraps the value in one more variant.
    let mut value = reply.child_value(0);
    while value.type_() == glib::VariantTy::VARIANT {
        value = value.as_variant()?;
    }
    match value.get::<u32>()? {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

fn gtk_prefers_dark() -> Option<bool> {
    let settings = gtk4::Settings::default()?;
    let dark_theme = settings
        .gtk_theme_name()
        .is_some_and(|name| name.to_lowercase().contains("dark"));
    (settings.is_gtk_application_prefer_dark_theme() || dark_theme).then_some(true)
}

fn gnome_color_scheme() -> Option<bool> {
    let schema =
        gio::SettingsSchemaSource::default()?.lookup("org.gnome.desktop.interface", true)?;
    if !schema.has_key("color-scheme") {
        return None;
    }
    let on_gnome =
        std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_lowercase().contains("gnome"));
    match gio::Settings::new("org.gnome.desktop.interface")
        .string("color-scheme")
        .as_str()
    {
        "prefer-dark" => Some(true),
        "prefer-light" => Some(false),
        // GNOME shows "default" as light.
        _ if on_gnome => Some(false),
        _ => None,
    }
}

// ── Widgets ──

fn with(weak: &Weak<Picker>, f: impl FnOnce(&Rc<Picker>)) {
    if let Some(picker) = weak.upgrade() {
        f(&picker);
    }
}

fn row_hash(row: &gtk4::ListBoxRow) -> Option<String> {
    row.widget_name().strip_prefix("entry:").map(str::to_string)
}

fn section_row(title: &str) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.set_widget_name("header");
    row.add_css_class("section");
    row.set_selectable(false);
    row.set_activatable(false);
    row.set_focusable(false);

    let label = gtk4::Label::new(Some(title));
    label.add_css_class("section-label");
    label.set_xalign(0.0);
    row.set_child(Some(&label));
    row
}

fn small_label(text: &str, class: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class(class);
    label.set_xalign(0.0);
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    label
}

fn action_button(icon: &str, tooltip: &str, class: Option<&str>) -> gtk4::Button {
    let button = gtk4::Button::from_icon_name(icon);
    button.add_css_class("row-action");
    if let Some(class) = class {
        button.add_css_class(class);
    }
    button.set_tooltip_text(Some(tooltip));
    button.set_focusable(false);
    button.set_valign(gtk4::Align::Center);
    button
}

/// A rounded square filled with a copied color.
fn swatch((r, g, b): (u8, u8, u8), outline: (u8, u8, u8)) -> gtk4::DrawingArea {
    let area = gtk4::DrawingArea::new();
    area.set_content_width(24);
    area.set_content_height(24);
    area.set_valign(gtk4::Align::Center);
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h, radius) = (w as f64 - 1.0, h as f64 - 1.0, 6.0);
        cr.new_sub_path();
        cr.arc(w - radius + 0.5, radius + 0.5, radius, -FRAC_PI_2, 0.0);
        cr.arc(w - radius + 0.5, h - radius + 0.5, radius, 0.0, FRAC_PI_2);
        cr.arc(radius + 0.5, h - radius + 0.5, radius, FRAC_PI_2, PI);
        cr.arc(radius + 0.5, radius + 0.5, radius, PI, 1.5 * PI);
        cr.close_path();
        let channel = |c: u8| c as f64 / 255.0;
        cr.set_source_rgb(channel(r), channel(g), channel(b));
        let _ = cr.fill_preserve();
        cr.set_source_rgb(channel(outline.0), channel(outline.1), channel(outline.2));
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    area
}

/// Title ("1920 × 1080") and detail ("PNG image, 240 KB") for an image clip.
fn image_labels(path: &Path, mime: &str) -> (String, String) {
    let kind = mime.strip_prefix("image/").unwrap_or(mime).to_uppercase();
    let title = Pixbuf::file_info(path)
        .map(|(_, w, h)| format!("{} \u{d7} {}", w, h))
        .unwrap_or_else(|| "Image".to_string());
    let detail = match std::fs::metadata(path) {
        Ok(meta) => format!("{} image, {}", kind, preview::human_size(meta.len())),
        Err(_) => format!("{} image, file missing", kind),
    };
    (title, detail)
}

fn thumbnail_widget(hash: &str, path: &Path) -> gtk4::Widget {
    match thumbnail(hash, path) {
        Some(texture) => {
            let picture = gtk4::Picture::for_paintable(&texture);
            picture.set_size_request(THUMB_W, THUMB_H);
            picture.set_can_shrink(true);
            picture.set_overflow(gtk4::Overflow::Hidden);
            picture.set_valign(gtk4::Align::Center);
            picture.add_css_class("thumb");
            picture.upcast()
        }
        None => {
            let icon = gtk4::Image::from_icon_name("image-x-generic-symbolic");
            icon.set_pixel_size(20);
            icon.set_size_request(THUMB_W, THUMB_H);
            icon.set_valign(gtk4::Align::Center);
            icon.add_css_class("thumb");
            icon.add_css_class("thumb-missing");
            icon.upcast()
        }
    }
}

/// A small cached preview of an image, so opening the picker doesn't decode
/// every full-size screenshot each time.
fn thumbnail(hash: &str, path: &Path) -> Option<gdk4::Texture> {
    let cache = storage::global().thumbnail_path(hash);
    let pixbuf = match Pixbuf::from_file(&cache) {
        Ok(pixbuf) => pixbuf,
        Err(_) => {
            let pixbuf = render_thumbnail(path)?;
            if let Some(dir) = cache.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = pixbuf.savev(&cache, "png", &[]) {
                log::debug!("Cannot cache thumbnail {}: {}", cache.display(), e);
            }
            pixbuf
        }
    };
    Some(gdk4::Texture::for_pixbuf(&pixbuf))
}

/// Scale the image to cover the thumbnail box (never enlarging it) and crop the middle.
fn render_thumbnail(path: &Path) -> Option<Pixbuf> {
    let (_, w, h) = Pixbuf::file_info(path)?;
    if w <= 0 || h <= 0 {
        return None;
    }
    let (tw, th) = (THUMB_W * 2, THUMB_H * 2);
    let scale = (tw as f64 / w as f64).max(th as f64 / h as f64).min(1.0);
    let sw = ((w as f64 * scale).round() as i32).max(1);
    let sh = ((h as f64 * scale).round() as i32).max(1);
    let scaled = Pixbuf::from_file_at_scale(path, sw, sh, false).ok()?;
    let (cw, ch) = (sw.min(tw), sh.min(th));
    scaled
        .new_subpixbuf((sw - cw) / 2, (sh - ch) / 2, cw, ch)
        .copy()
}
