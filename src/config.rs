use crate::theme;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Which paste shortcut to send after picking an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PasteMode {
    /// Detect whether the focused window is a terminal.
    #[default]
    Auto,
    /// Always Ctrl+Shift+V.
    Terminal,
    /// Always Ctrl+V.
    Normal,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub max_entries: usize,
    pub max_backups: usize,
    pub history_file: PathBuf,
    pub log_level: String,
    pub auto_paste: bool,
    pub paste_mode: PasteMode,
    pub image_dir: PathBuf,
    pub max_image_size_mb: u32,
    pub max_text_size_kb: u32,
    pub sync: SyncConfig,
    pub appearance: Appearance,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyncConfig {
    pub enabled: bool,
    pub sync_dir: PathBuf,
}

/// How the picker looks (`[appearance]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    /// A built-in theme, or "auto" to follow the system's light/dark setting.
    pub theme: String,
    /// The themes "auto" switches between.
    pub dark_theme: String,
    pub light_theme: String,
    /// Font for the interface ("" uses the system font).
    pub font: String,
    /// Font for clip text.
    pub mono_font: String,
    /// Base text size in pixels.
    pub font_size: u32,
    /// Window size in pixels.
    pub width: u32,
    pub height: u32,
    /// Background opacity, 0.5–1.0 (needs a compositor that supports transparency).
    pub opacity: f64,
    /// Overrides for single theme colors, e.g. `accent = "#89b4fa"`.
    pub colors: BTreeMap<String, String>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: "catppuccin-mocha".to_string(),
            dark_theme: "catppuccin-mocha".to_string(),
            light_theme: "catppuccin-latte".to_string(),
            font: String::new(),
            mono_font: "monospace".to_string(),
            font_size: 13,
            width: 460,
            height: 580,
            opacity: 1.0,
            colors: BTreeMap::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_entries: 50,
            max_backups: 3,
            history_file: default_history_file(),
            log_level: "info".to_string(),
            auto_paste: true,
            paste_mode: PasteMode::Auto,
            image_dir: default_image_dir(),
            max_image_size_mb: 10,
            max_text_size_kb: 1024,
            sync: SyncConfig::default(),
            appearance: Appearance::default(),
        }
    }
}

impl Config {
    /// The sync folder, if sync is enabled and a folder is configured.
    pub fn sync_dir(&self) -> Option<PathBuf> {
        (self.sync.enabled && !self.sync.sync_dir.as_os_str().is_empty())
            .then(|| self.sync.sync_dir.clone())
    }
}

fn default_image_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("cannot determine home directory")
                .join(".local/share")
        })
        .join("copyninja")
        .join("images")
}

fn default_history_file() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".clipboard_history.json")
}

fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("copyninja").join("config.toml"))
}

/// Expand a leading `~` to the home directory (TOML has no notion of it).
fn expand_tilde(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), dirs::home_dir()) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

/// Parse config from a TOML string.
///
/// A bad or unknown key is skipped and reported in `warnings`; the remaining
/// keys still apply. Only a file that isn't valid TOML at all falls back to
/// the defaults entirely.
pub fn parse(content: &str, warnings: &mut Vec<String>) -> Config {
    let mut table: toml::Table = match content.parse() {
        Ok(table) => table,
        Err(e) => {
            warnings.push(format!("not valid TOML, using defaults: {}", e.message()));
            return Config::default();
        }
    };

    let sync = match table.remove("sync") {
        Some(toml::Value::Table(sync)) => lenient::<SyncConfig>(sync, "sync.", warnings),
        Some(_) => {
            warnings.push("ignoring `sync`: expected a [sync] table".to_string());
            SyncConfig::default()
        }
        None => SyncConfig::default(),
    };

    let appearance = match table.remove("appearance") {
        Some(toml::Value::Table(appearance)) => parse_appearance(appearance, warnings),
        Some(_) => {
            warnings.push("ignoring `appearance`: expected an [appearance] table".to_string());
            Appearance::default()
        }
        None => Appearance::default(),
    };

    let mut config: Config = lenient(table, "", warnings);
    config.sync = sync;
    config.appearance = appearance;
    config.history_file = expand_tilde(&config.history_file);
    config.image_dir = expand_tilde(&config.image_dir);
    config.sync.sync_dir = expand_tilde(&config.sync.sync_dir);
    config
}

/// Parse `[appearance]`, replacing each invalid value with its default.
fn parse_appearance(mut table: toml::Table, warnings: &mut Vec<String>) -> Appearance {
    let colors = table.remove("colors");
    let mut a: Appearance = lenient(table, "appearance.", warnings);
    let defaults = Appearance::default();
    let mut warn = |key: &str, problem: String| {
        warnings.push(format!("ignoring `appearance.{}`: {}", key, problem));
    };

    match colors {
        Some(toml::Value::Table(colors)) => {
            for (key, value) in colors {
                if !theme::COLOR_KEYS.contains(&key.as_str()) {
                    let expected = theme::COLOR_KEYS.join(", ");
                    warn(
                        &format!("colors.{}", key),
                        format!("unknown color, expected one of {}", expected),
                    );
                } else if let Some(color) = value.as_str().and_then(theme::parse_color) {
                    a.colors.insert(key, color);
                } else {
                    warn(
                        &format!("colors.{}", key),
                        "expected a color like \"#89b4fa\"".to_string(),
                    );
                }
            }
        }
        Some(_) => warn(
            "colors",
            "expected an [appearance.colors] table".to_string(),
        ),
        None => {}
    }

    let unknown_theme = |name: &str| {
        format!(
            "unknown theme \"{}\" (run `copyninja themes` to list them)",
            name
        )
    };
    if a.theme != "auto" && theme::preset(&a.theme).is_none() {
        warn("theme", unknown_theme(&a.theme));
        a.theme = defaults.theme.clone();
    }
    if theme::preset(&a.dark_theme).is_none() {
        warn("dark_theme", unknown_theme(&a.dark_theme));
        a.dark_theme = defaults.dark_theme.clone();
    }
    if theme::preset(&a.light_theme).is_none() {
        warn("light_theme", unknown_theme(&a.light_theme));
        a.light_theme = defaults.light_theme.clone();
    }

    // Font names end up in CSS, so only plain family lists are allowed.
    let valid_font = |s: &str| {
        s.chars()
            .all(|c| c.is_alphanumeric() || " ,.-_".contains(c))
    };
    if !valid_font(&a.font) {
        warn("font", "use a font family name, e.g. \"Inter\"".to_string());
        a.font = defaults.font.clone();
    }
    if !valid_font(&a.mono_font) || a.mono_font.trim().is_empty() {
        warn(
            "mono_font",
            "use a font family name, e.g. \"JetBrains Mono\"".to_string(),
        );
        a.mono_font = defaults.mono_font.clone();
    }

    if !(8..=32).contains(&a.font_size) {
        warn("font_size", "expected a size from 8 to 32".to_string());
        a.font_size = defaults.font_size;
    }
    if !(280..=2000).contains(&a.width) {
        warn("width", "expected a width from 280 to 2000".to_string());
        a.width = defaults.width;
    }
    if !(280..=2000).contains(&a.height) {
        warn("height", "expected a height from 280 to 2000".to_string());
        a.height = defaults.height;
    }
    if !(0.5..=1.0).contains(&a.opacity) {
        warn("opacity", "expected a value from 0.5 to 1.0".to_string());
        a.opacity = if a.opacity.is_nan() {
            defaults.opacity
        } else {
            a.opacity.clamp(0.5, 1.0)
        };
    }
    a
}

/// Deserialize `table` into `T`, dropping (and reporting) each key that
/// doesn't deserialize on its own.
fn lenient<T: DeserializeOwned + Default>(
    mut table: toml::Table,
    prefix: &str,
    warnings: &mut Vec<String>,
) -> T {
    let keys: Vec<String> = table.keys().cloned().collect();
    for key in keys {
        let mut single = toml::Table::new();
        single.insert(key.clone(), table[&key].clone());
        if let Err(e) = single.try_into::<T>() {
            warnings.push(format!("ignoring `{}{}`: {}", prefix, key, e.message()));
            table.remove(&key);
        }
    }
    table.try_into().unwrap_or_default()
}

/// Load the config file. Problems are returned as warnings rather than logged,
/// because this runs before the logger exists (the log level comes from here).
pub fn load() -> (Config, Vec<String>) {
    let mut warnings = Vec::new();
    let Some(path) = config_path() else {
        return (Config::default(), warnings);
    };

    let config = match std::fs::read_to_string(&path) {
        Ok(content) => {
            let config = parse(&content, &mut warnings);
            for w in warnings.iter_mut() {
                *w = format!("{}: {}", path.display(), w);
            }
            config
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => {
            warnings.push(format!("cannot read {}: {}", path.display(), e));
            Config::default()
        }
    };
    (config, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(content: &str) -> Config {
        let mut warnings = Vec::new();
        let config = parse(content, &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
        config
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.max_entries, 50);
        assert_eq!(config.max_backups, 3);
        assert_eq!(config.log_level, "info");
        assert!(config.auto_paste);
        assert_eq!(config.paste_mode, PasteMode::Auto);
    }

    #[test]
    fn test_partial_config() {
        let config = parse_ok("max_entries = 100");
        assert_eq!(config.max_entries, 100);
        // Other fields should have defaults
        assert_eq!(config.max_backups, 3);
        assert!(config.auto_paste);
    }

    #[test]
    fn test_full_config() {
        let config = parse_ok(
            r#"
            max_entries = 200
            max_backups = 5
            log_level = "debug"
            auto_paste = false
            paste_mode = "terminal"
            history_file = "/tmp/test.json"
            max_text_size_kb = 64

            [sync]
            enabled = true
            sync_dir = "/tmp/sync"
        "#,
        );
        assert_eq!(config.max_entries, 200);
        assert_eq!(config.max_backups, 5);
        assert_eq!(config.log_level, "debug");
        assert!(!config.auto_paste);
        assert_eq!(config.paste_mode, PasteMode::Terminal);
        assert_eq!(config.history_file, PathBuf::from("/tmp/test.json"));
        assert_eq!(config.max_text_size_kb, 64);
        assert_eq!(config.sync_dir(), Some(PathBuf::from("/tmp/sync")));
    }

    #[test]
    fn test_invalid_toml_returns_defaults() {
        let mut warnings = Vec::new();
        let config = parse("NOT VALID TOML {{{{", &mut warnings);
        assert_eq!(config.max_entries, 50);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn test_empty_string_returns_defaults() {
        let config = parse_ok("");
        assert_eq!(config.max_entries, 50);
        assert!(config.auto_paste);
    }

    #[test]
    fn test_bad_key_only_drops_that_key() {
        let mut warnings = Vec::new();
        let config = parse(
            r#"
            max_entries = "100"
            paste_mode = "normal"
            auto_paste = false
        "#,
            &mut warnings,
        );
        assert_eq!(config.max_entries, 50, "bad value falls back to default");
        assert_eq!(
            config.paste_mode,
            PasteMode::Normal,
            "other keys still apply"
        );
        assert!(!config.auto_paste);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("max_entries"), "{}", warnings[0]);
    }

    #[test]
    fn test_unknown_key_and_bad_paste_mode_are_reported() {
        let mut warnings = Vec::new();
        let config = parse("max_entires = 10\npaste_mode = \"Terminal\"", &mut warnings);
        assert_eq!(config.max_entries, 50);
        assert_eq!(config.paste_mode, PasteMode::Auto);
        assert_eq!(warnings.len(), 2, "{:?}", warnings);
    }

    #[test]
    fn test_bad_sync_key_keeps_rest_of_sync() {
        let mut warnings = Vec::new();
        let config = parse(
            "[sync]\nenabled = true\nsync_dir = \"/tmp/s\"\nbogus = 1",
            &mut warnings,
        );
        assert_eq!(config.sync_dir(), Some(PathBuf::from("/tmp/s")));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("sync.bogus"), "{}", warnings[0]);
    }

    #[test]
    fn test_tilde_paths_are_expanded() {
        let config = parse_ok(
            "history_file = \"~/h.json\"\n[sync]\nenabled = true\nsync_dir = \"~/Sync/cn\"",
        );
        let home = dirs::home_dir().unwrap();
        assert_eq!(config.history_file, home.join("h.json"));
        assert_eq!(config.sync.sync_dir, home.join("Sync/cn"));
    }

    #[test]
    fn test_appearance_defaults_and_values() {
        assert_eq!(parse_ok("").appearance.theme, "catppuccin-mocha");
        let a = parse_ok(
            r##"
            [appearance]
            theme = "auto"
            light_theme = "gruvbox-light"
            font = "Inter, sans-serif"
            mono_font = "JetBrains Mono"
            font_size = 14
            opacity = 0.9

            [appearance.colors]
            accent = "#89B4FA"
            pin = "#fc0"
        "##,
        )
        .appearance;
        assert_eq!(a.theme, "auto");
        assert_eq!(a.light_theme, "gruvbox-light");
        assert_eq!(a.mono_font, "JetBrains Mono");
        assert_eq!(a.font_size, 14);
        assert_eq!(a.opacity, 0.9);
        assert_eq!(a.colors.get("accent").map(String::as_str), Some("#89b4fa"));
        assert_eq!(a.colors.get("pin").map(String::as_str), Some("#ffcc00"));
    }

    #[test]
    fn test_bad_appearance_values_fall_back_one_by_one() {
        let mut warnings = Vec::new();
        let a = parse(
            r##"
            [appearance]
            theme = "solarized-neon"
            font = "Inter; } window { color: red"
            font_size = 99
            opacity = 0.1
            width = 520

            [appearance.colors]
            accent = "purple"
            sparkle = "#fff"
            text = "#eeeeee"
        "##,
            &mut warnings,
        )
        .appearance;
        assert_eq!(a.theme, "catppuccin-mocha");
        assert_eq!(a.font, "");
        assert_eq!(a.font_size, 13);
        assert_eq!(a.opacity, 0.5, "clamped");
        assert_eq!(a.width, 520, "valid keys still apply");
        assert_eq!(a.colors.len(), 1);
        assert_eq!(a.colors.get("text").map(String::as_str), Some("#eeeeee"));
        assert_eq!(warnings.len(), 6, "{:#?}", warnings);
    }

    #[test]
    fn test_sync_dir_requires_enabled_and_path() {
        assert_eq!(parse_ok("[sync]\nenabled = true").sync_dir(), None);
        assert_eq!(parse_ok("[sync]\nsync_dir = \"/tmp/s\"").sync_dir(), None);
    }
}
