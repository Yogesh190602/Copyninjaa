mod app;
mod css;
pub(crate) mod paste;
mod preview;

use crate::config::{Config, PasteMode};

pub fn run(config: &Config) {
    // Detect the previously focused window BEFORE the picker steals focus.
    // This is the only reliable way to know if a terminal was focused.
    let target = paste::detect_target();
    let target_is_terminal = match config.paste_mode {
        PasteMode::Terminal => true,
        PasteMode::Normal => false,
        PasteMode::Auto => target
            .class
            .as_deref()
            .is_some_and(paste::is_terminal_class),
    };
    app::run(
        config.auto_paste,
        target_is_terminal,
        target,
        config.appearance.clone(),
    );
}
