//! Built-in color themes for the picker, and resolving the configured one.

use crate::config::Appearance;

/// Colors the picker is drawn with. Every value is "#rrggbb".
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    /// Window background
    pub background: String,
    /// Search bar and footer
    pub bar: String,
    /// Dividers and the thumbnail backdrop
    pub border: String,
    /// Search field, selected clip, key caps
    pub surface: String,
    /// Hovered buttons
    pub surface_strong: String,
    /// Main text
    pub text: String,
    /// Secondary text on the selected clip
    pub subtext: String,
    /// Times, sizes, hints, placeholders, line-break marks
    pub muted: String,
    /// Selection edge, focus ring, search highlights
    pub accent: String,
    /// Pin marks
    pub pin: String,
    /// Delete and clear confirmations
    pub danger: String,
}

/// Name, whether it's dark, and colors in `Palette` field order.
type Preset = (&'static str, bool, [&'static str; 11]);

#[rustfmt::skip]
pub const PRESETS: &[Preset] = &[
    //                       background bar        border     surface    surf.strong text       subtext    muted      accent     pin        danger
    ("catppuccin-mocha",     true,  ["#1e1e2e", "#181825", "#11111b", "#313244", "#45475a", "#cdd6f4", "#a6adc8", "#7f849c", "#cba6f7", "#f9e2af", "#f38ba8"]),
    ("catppuccin-macchiato", true,  ["#24273a", "#1e2030", "#181926", "#363a4f", "#494d64", "#cad3f5", "#a5adcb", "#8087a2", "#c6a0f6", "#eed49f", "#ed8796"]),
    ("catppuccin-frappe",    true,  ["#303446", "#292c3c", "#232634", "#414559", "#51576d", "#c6d0f5", "#a5adce", "#838ba7", "#ca9ee6", "#e5c890", "#e78284"]),
    ("catppuccin-latte",     false, ["#eff1f5", "#e6e9ef", "#dce0e8", "#ccd0da", "#bcc0cc", "#4c4f69", "#5c5f77", "#7c7f93", "#8839ef", "#df8e1d", "#d20f39"]),
    ("tokyo-night",          true,  ["#1a1b26", "#16161e", "#101014", "#292e42", "#3b4261", "#c0caf5", "#a9b1d6", "#737aa2", "#7aa2f7", "#e0af68", "#f7768e"]),
    ("dracula",              true,  ["#282a36", "#21222c", "#191a21", "#44475a", "#565970", "#f8f8f2", "#d4d4cf", "#8b93c4", "#bd93f9", "#f1fa8c", "#ff5555"]),
    ("nord",                 true,  ["#2e3440", "#292e39", "#232831", "#3b4252", "#4c566a", "#eceff4", "#d8dee9", "#8c96aa", "#88c0d0", "#ebcb8b", "#bf616a"]),
    ("gruvbox-dark",         true,  ["#282828", "#1d2021", "#141617", "#3c3836", "#504945", "#ebdbb2", "#d5c4a1", "#a89984", "#fe8019", "#fabd2f", "#fb4934"]),
    ("gruvbox-light",        false, ["#fbf1c7", "#f2e5bc", "#d5c4a1", "#e6d4a8", "#d5c4a1", "#3c3836", "#504945", "#7c6f64", "#af3a03", "#b57614", "#9d0006"]),
    ("rose-pine",            true,  ["#191724", "#1f1d2e", "#121019", "#2f2c44", "#403d52", "#e0def4", "#c4c0dd", "#908caa", "#c4a7e7", "#f6c177", "#eb6f92"]),
    ("rose-pine-dawn",       false, ["#faf4ed", "#f2e9e1", "#dfdad9", "#e9dfd6", "#dcd1c7", "#575279", "#6e6a8a", "#797593", "#907aa9", "#ea9d34", "#b4637a"]),
    ("adwaita-dark",         true,  ["#242424", "#2e2e2e", "#1a1a1a", "#3e3e3e", "#505050", "#ffffff", "#d0d0d0", "#9a9a9a", "#78aeed", "#f8e45c", "#ff7b63"]),
    ("adwaita",              false, ["#fafafa", "#f0f0f0", "#d6d6d6", "#e0e0e0", "#cfcfcf", "#2e2e2e", "#4a4a4a", "#6b6b6b", "#1c71d8", "#b8860b", "#c01c28"]),
];

impl Palette {
    fn from_colors(colors: &[&str; 11]) -> Self {
        let [background, bar, border, surface, surface_strong, text, subtext, muted, accent, pin, danger] =
            colors.map(str::to_string);
        Self {
            background,
            bar,
            border,
            surface,
            surface_strong,
            text,
            subtext,
            muted,
            accent,
            pin,
            danger,
        }
    }

    fn fields_mut(&mut self) -> [(&'static str, &mut String); 11] {
        [
            ("background", &mut self.background),
            ("bar", &mut self.bar),
            ("border", &mut self.border),
            ("surface", &mut self.surface),
            ("surface_strong", &mut self.surface_strong),
            ("text", &mut self.text),
            ("subtext", &mut self.subtext),
            ("muted", &mut self.muted),
            ("accent", &mut self.accent),
            ("pin", &mut self.pin),
            ("danger", &mut self.danger),
        ]
    }
}

/// The built-in theme with this name.
pub fn preset(name: &str) -> Option<Palette> {
    PRESETS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, _, colors)| Palette::from_colors(colors))
}

/// The palette for the configured appearance. `system_prefers_dark` is only
/// asked when the theme is "auto"; no answer means dark.
pub fn resolve(
    appearance: &Appearance,
    system_prefers_dark: impl FnOnce() -> Option<bool>,
) -> Palette {
    let name = if appearance.theme == "auto" {
        if system_prefers_dark().unwrap_or(true) {
            &appearance.dark_theme
        } else {
            &appearance.light_theme
        }
    } else {
        &appearance.theme
    };
    let mut palette = preset(name).unwrap_or_else(|| Palette::from_colors(&PRESETS[0].2));
    for (key, value) in palette.fields_mut() {
        if let Some(color) = appearance.colors.get(key) {
            *value = color.to_string();
        }
    }
    palette
}

/// Names of the color keys that can be overridden, in palette order.
pub const COLOR_KEYS: [&str; 11] = [
    "background",
    "bar",
    "border",
    "surface",
    "surface_strong",
    "text",
    "subtext",
    "muted",
    "accent",
    "pin",
    "danger",
];

/// Parse "#rgb" or "#rrggbb" into lowercase "#rrggbb".
pub fn parse_color(s: &str) -> Option<String> {
    let hex = s.trim().strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        3 => Some(format!(
            "#{}",
            hex.chars()
                .flat_map(|c| [c, c])
                .collect::<String>()
                .to_lowercase()
        )),
        6 => Some(format!("#{}", hex.to_lowercase())),
        _ => None,
    }
}

/// "#rrggbb" → (r, g, b) in 0..=255.
pub fn rgb(color: &str) -> (u8, u8, u8) {
    let hex = color.trim_start_matches('#');
    let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    (channel(0), channel(2), channel(4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_well_formed() {
        let mut names = std::collections::HashSet::new();
        for (name, _, colors) in PRESETS {
            assert!(names.insert(name), "duplicate theme {}", name);
            for color in colors {
                assert_eq!(
                    parse_color(color).as_deref(),
                    Some(*color),
                    "{} in {}",
                    color,
                    name
                );
            }
        }
    }

    #[test]
    fn default_theme_is_the_original_look() {
        let palette = resolve(&Appearance::default(), || None);
        assert_eq!(palette.background, "#1e1e2e");
        assert_eq!(palette.accent, "#cba6f7");
    }

    #[test]
    fn auto_follows_the_system() {
        let appearance = Appearance {
            theme: "auto".into(),
            ..Appearance::default()
        };
        assert_eq!(resolve(&appearance, || Some(false)).background, "#eff1f5");
        assert_eq!(resolve(&appearance, || Some(true)).background, "#1e1e2e");
        assert_eq!(resolve(&appearance, || None).background, "#1e1e2e");
    }

    #[test]
    fn color_overrides_apply_on_top_of_the_theme() {
        let mut appearance = Appearance {
            theme: "nord".into(),
            ..Appearance::default()
        };
        appearance.colors.insert("accent".into(), "#ff0000".into());
        let palette = resolve(&appearance, || None);
        assert_eq!(palette.accent, "#ff0000");
        assert_eq!(palette.background, "#2e3440");
    }

    #[test]
    fn color_parsing() {
        assert_eq!(parse_color("#ABC").as_deref(), Some("#aabbcc"));
        assert_eq!(parse_color(" #A1b2C3 ").as_deref(), Some("#a1b2c3"));
        assert_eq!(parse_color("red"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#ff0000; } window { color: red"), None);
        assert_eq!(rgb("#cba6f7"), (203, 166, 247));
    }
}
