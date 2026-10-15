mod app;
mod css;
pub(crate) mod paste;
mod preview;

use crate::config::Config;

pub fn run(config: &Config) {
    // Detect the previously focused window BEFORE the picker steals focus.
    // This is the only reliable way to know if a terminal was focused.
    let target = paste::detect_target();
    let keys = paste::PasteKeys::choose(config.paste_mode, &target);
    app::run(config.auto_paste, keys, target, config.appearance.clone());
}
