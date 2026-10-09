use crate::config::Appearance;
use crate::theme::Palette;

/// Generic CSS font families, which must not be quoted.
const GENERIC_FONTS: [&str; 6] = [
    "serif",
    "sans-serif",
    "monospace",
    "cursive",
    "fantasy",
    "system-ui",
];

/// The picker stylesheet for a palette and the appearance settings.
pub fn stylesheet(palette: &Palette, appearance: &Appearance) -> String {
    let translucent = |color: &str| {
        if appearance.opacity < 1.0 {
            format!("alpha({}, {:.2})", color, appearance.opacity)
        } else {
            color.to_string()
        }
    };
    let colors = [
        ("background", translucent(&palette.background)),
        ("bar", translucent(&palette.bar)),
        ("border", palette.border.clone()),
        ("surface", palette.surface.clone()),
        ("surface_strong", palette.surface_strong.clone()),
        ("text", palette.text.clone()),
        ("subtext", palette.subtext.clone()),
        ("muted", palette.muted.clone()),
        ("accent", palette.accent.clone()),
        ("pin", palette.pin.clone()),
        ("danger", palette.danger.clone()),
    ];

    let mut css = String::new();
    for (name, value) in colors {
        css.push_str(&format!("@define-color cn_{} {};\n", name, value));
    }

    let base = appearance.font_size;
    let mut rules = TEMPLATE
        .replace(
            "$mono_font",
            &font_family(&appearance.mono_font, "monospace"),
        )
        .replace("$size_title", &format!("{}px", base + 2))
        .replace("$size_large", &format!("{}px", base + 1))
        .replace("$size_base", &format!("{}px", base))
        .replace("$size_small", &format!("{}px", base - 1))
        .replace(
            "$size_tiny",
            &format!("{}px", base.saturating_sub(2).max(7)),
        );
    if !appearance.font.trim().is_empty() {
        rules.push_str(&format!(
            "\nwindow.copyninja {{ font-family: {}; }}\n",
            font_family(&appearance.font, "sans-serif")
        ));
    }
    css.push_str(&rules);
    css
}

/// `Inter, sans-serif` → `"Inter", sans-serif`, always ending in a generic family.
fn font_family(list: &str, generic: &str) -> String {
    let mut names: Vec<String> = list
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| {
            if GENERIC_FONTS.contains(&n) {
                n.to_string()
            } else {
                format!("\"{}\"", n)
            }
        })
        .collect();
    if !names.iter().any(|n| n == generic) {
        names.push(generic.to_string());
    }
    names.join(", ")
}

/// Rules shared by every theme. Colors come from the `cn_*` definitions above;
/// `$…` tokens are replaced with sizes and fonts from the config. Everything is
/// scoped to the picker window, and a user `style.css` is applied on top.
const TEMPLATE: &str = r#"
window.copyninja {
    background-color: @cn_background;
    color: @cn_text;
    font-size: $size_base;
}

/* ── Search bar (doubles as the title bar) ── */

window.copyninja .titlebar,
window.copyninja .topbar {
    background-color: @cn_bar;
    box-shadow: none;
}

window.copyninja .topbar {
    padding: 10px;
    border-bottom: 1px solid @cn_border;
}

window.copyninja entry.search-field {
    min-height: 38px;
    padding: 0 12px;
    border: 1px solid transparent;
    border-radius: 10px;
    background-color: @cn_surface;
    color: @cn_text;
    font-size: $size_large;
    box-shadow: none;
    outline: none;
}

window.copyninja entry.search-field:focus-within {
    border-color: alpha(@cn_accent, 0.6);
}

window.copyninja entry.search-field image {
    color: @cn_muted;
}

window.copyninja entry.search-field text placeholder {
    color: @cn_muted;
}

window.copyninja entry.search-field text selection {
    background-color: alpha(@cn_accent, 0.35);
}

/* ── Clip list ── */

window.copyninja scrolledwindow,
window.copyninja list.clips {
    background-color: transparent;
}

window.copyninja list.clips {
    padding: 6px 6px 10px 6px;
}

window.copyninja list.clips > row {
    padding: 0;
    margin: 1px 0;
    border-radius: 8px;
    background-color: transparent;
    outline: none;
    box-shadow: none;
}

window.copyninja list.clips > row.clip:hover {
    background-color: alpha(@cn_surface, 0.55);
}

/* The selected clip is what Enter pastes: a filled row with an accent edge. */
window.copyninja list.clips > row.clip:selected {
    background-color: @cn_surface;
    box-shadow: inset 3px 0 @cn_accent;
}

window.copyninja list.clips > row.section {
    margin-top: 6px;
}

window.copyninja .section-label {
    padding: 6px 12px 4px 12px;
    color: @cn_muted;
    font-size: $size_small;
    font-weight: 600;
}

window.copyninja .clip-body {
    padding: 9px 8px 9px 14px;
}

/* Clip text is shown literally, in monospace, the way it will paste. */
window.copyninja .clip-text {
    font-family: $mono_font;
    font-size: $size_small;
    color: @cn_text;
}

window.copyninja .clip-title {
    font-size: $size_base;
    color: @cn_text;
}

window.copyninja .clip-sub {
    font-family: $mono_font;
    font-size: $size_tiny;
    color: @cn_muted;
}

window.copyninja .clip-meta,
window.copyninja .clip-time {
    font-size: $size_tiny;
    color: @cn_muted;
}

window.copyninja row:selected .clip-time,
window.copyninja row:selected .clip-meta,
window.copyninja row:selected .clip-sub {
    color: @cn_subtext;
}

window.copyninja .pin-mark {
    color: @cn_pin;
}

window.copyninja .thumb {
    border-radius: 6px;
    background-color: @cn_border;
}

window.copyninja .thumb-missing {
    color: @cn_muted;
}

/* Pin and delete replace the time while the pointer is over a clip. */
window.copyninja button.row-action {
    min-width: 26px;
    min-height: 26px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: none;
    box-shadow: none;
    color: @cn_muted;
}

window.copyninja button.row-action:hover {
    background-color: @cn_surface_strong;
    color: @cn_text;
}

window.copyninja button.row-action.pinned {
    color: @cn_pin;
}

window.copyninja button.row-action.delete:hover {
    color: @cn_danger;
}

/* ── Empty states ── */

window.copyninja .empty-icon {
    color: @cn_surface_strong;
}

window.copyninja .empty-title {
    font-size: $size_title;
    font-weight: 600;
    color: @cn_text;
}

window.copyninja .empty-body {
    font-size: $size_base;
    color: @cn_muted;
}

/* ── Footer ── */

window.copyninja .footer {
    padding: 6px 8px 6px 14px;
    border-top: 1px solid @cn_border;
    background-color: @cn_bar;
}

window.copyninja .key {
    padding: 1px 6px;
    border-radius: 4px;
    background-color: @cn_surface;
    color: @cn_subtext;
    font-size: $size_tiny;
}

window.copyninja .hint,
window.copyninja .status {
    font-size: $size_small;
    color: @cn_muted;
}

window.copyninja .status {
    color: @cn_text;
}

window.copyninja button.clear {
    min-height: 28px;
    padding: 0 10px;
    border: none;
    border-radius: 6px;
    background: none;
    box-shadow: none;
    color: @cn_muted;
    font-size: $size_small;
}

window.copyninja button.clear:hover {
    background-color: @cn_surface;
    color: @cn_text;
}

window.copyninja button.clear.confirm {
    background-color: alpha(@cn_danger, 0.18);
    color: @cn_danger;
}

/* ── Scrollbar ── */

window.copyninja scrollbar {
    background: none;
    border: none;
}

window.copyninja scrollbar slider {
    min-width: 4px;
    border-radius: 4px;
    background-color: @cn_surface_strong;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;

    #[test]
    fn stylesheet_uses_palette_fonts_and_sizes() {
        let mut appearance = Appearance {
            theme: "nord".into(),
            font: "Inter".into(),
            mono_font: "JetBrains Mono, monospace".into(),
            font_size: 15,
            ..Appearance::default()
        };
        let palette = theme::resolve(&appearance, || None);
        let css = stylesheet(&palette, &appearance);
        assert!(css.contains("@define-color cn_background #2e3440;"));
        assert!(css.contains("@define-color cn_accent #88c0d0;"));
        assert!(css.contains("font-family: \"JetBrains Mono\", monospace;"));
        assert!(css.contains("window.copyninja { font-family: \"Inter\", sans-serif; }"));
        assert!(css.contains("font-size: 16px;"));
        assert!(!css.contains('$'), "every token replaced");

        appearance.opacity = 0.85;
        let css = stylesheet(&palette, &appearance);
        assert!(css.contains("@define-color cn_background alpha(#2e3440, 0.85);"));
        assert!(
            css.contains("@define-color cn_text #eceff4;"),
            "text stays opaque"
        );
    }

    #[test]
    fn default_stylesheet_has_no_custom_font_rule() {
        let appearance = Appearance::default();
        let css = stylesheet(&theme::resolve(&appearance, || None), &appearance);
        assert!(!css.contains("sans-serif"));
        assert!(css.contains("font-family: monospace;"));
    }
}
