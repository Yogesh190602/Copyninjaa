use log::{debug, warn};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// The window that had focus before the picker opened.
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// Window class, if it could be detected.
    pub class: Option<String>,
    /// The window is an X11 (XWayland) window inside a Wayland session, so
    /// xdotool can reach it.
    pub xwayland: bool,
}

/// Write text to the system clipboard synchronously using wl-copy or xclip.
/// Returns true if the clipboard was successfully set.
pub fn write_clipboard_sync(text: &str) -> bool {
    // Try wl-copy (Wayland) — blocks until compositor accepts
    if let Ok(mut child) = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
            drop(stdin);
        }
        if let Ok(status) = child.wait() {
            if status.success() {
                debug!("Clipboard set via wl-copy (sync)");
                return true;
            }
        }
        debug!("wl-copy failed, trying xclip");
    } else {
        debug!("wl-copy not found, trying xclip");
    }

    // Fallback: xclip (X11/XWayland)
    if let Ok(mut child) = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
            drop(stdin);
        }
        if let Ok(status) = child.wait() {
            if status.success() {
                debug!("Clipboard set via xclip (sync)");
                return true;
            }
        }
    }

    warn!("Both wl-copy and xclip failed to set clipboard");
    false
}

/// Write an image file to the system clipboard using wl-copy or xclip.
/// Returns true if the clipboard was successfully set.
pub fn write_image_clipboard_sync(path: &Path, mime: &str) -> bool {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to read image file {}: {}", path.display(), e);
            return false;
        }
    };

    // Try wl-copy with MIME type
    if let Ok(mut child) = Command::new("wl-copy")
        .args(["--type", mime])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&data);
            drop(stdin);
        }
        if let Ok(status) = child.wait() {
            if status.success() {
                debug!("Image clipboard set via wl-copy ({})", mime);
                return true;
            }
        }
    }

    // Fallback: xclip with target type
    if let Ok(mut child) = Command::new("xclip")
        .args(["-selection", "clipboard", "-t", mime, "-i"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&data);
            drop(stdin);
        }
        if let Ok(status) = child.wait() {
            if status.success() {
                debug!("Image clipboard set via xclip ({})", mime);
                return true;
            }
        }
    }

    warn!("Failed to set image clipboard");
    false
}

/// Show a desktop notification.
pub fn notify(summary: &str, body: &str) {
    let _ = Command::new("notify-send")
        .args(["-a", "CopyNinja", summary, body])
        .spawn();
}

/// Detect the currently focused window.
/// Called BEFORE the picker window opens, so focus is still on the previous window.
pub fn detect_target() -> Target {
    // Hyprland
    if let Ok(output) = Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
    {
        if output.status.success() {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                if let Some(class) = json.get("class").and_then(|v| v.as_str()) {
                    if !class.is_empty() {
                        debug!("Pre-launch focused class (hyprctl): '{}'", class);
                        let xwayland = json.get("xwayland").and_then(|v| v.as_bool()) == Some(true);
                        return Target {
                            class: Some(class.to_string()),
                            xwayland,
                        };
                    }
                }
            }
        }
    }

    // X11 / XWayland
    if let Ok(output) = Command::new("xdotool")
        .args(["getactivewindow", "getwindowclassname"])
        .output()
    {
        if output.status.success() {
            let class = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // xdotool returns "(null)" for native Wayland windows on GNOME/KDE —
            // it can only see XWayland windows. Treat "(null)" and empty as
            // "detection failed" rather than a real class name.
            if !class.is_empty() && class != "(null)" {
                debug!("Pre-launch focused class (xdotool): '{}'", class);
                return Target {
                    class: Some(class),
                    xwayland: is_wayland(),
                };
            }
            debug!(
                "xdotool returned invalid class '{}' (native Wayland window?)",
                class
            );
        }
    }

    Target::default()
}

/// Check if a window class name belongs to a known terminal emulator.
pub(crate) fn is_terminal_class(class: &str) -> bool {
    let class = class.to_lowercase();

    const TERMINALS: &[&str] = &[
        "alacritty",
        "kitty",
        "foot",
        "wezterm",
        "ghostty",
        "konsole",
        "tilix",
        "terminator",
        "sakura",
        "guake",
        "yakuake",
        "tilda",
        "contour",
        "rio",
        "xterm",
        "urxvt",
        "rxvt",
        "st",
        "st-256color",
        "kgx",
        "ptyxis",
        "blackbox",
    ];

    // Also match reverse-DNS app IDs: "com.mitchellh.ghostty", "org.gnome.Ptyxis".
    let short = class.rsplit('.').next().unwrap_or(&class);
    if TERMINALS.iter().any(|t| class == *t || short == *t) {
        return true;
    }

    // Catch variants like "gnome-terminal", "xfce4-terminal",
    // "org.gnome.Terminal", "org.kde.konsole", "org.gnome.Console", etc.
    class.contains("terminal") || class.contains("konsole") || class.contains("console")
}

fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Detect if we're running on GNOME Wayland.
/// On GNOME Wayland, xdotool triggers a "Remote Desktop" permission dialog
/// instead of actually pasting, so we must skip it entirely.
fn is_gnome_wayland() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let result = is_wayland() && desktop.to_lowercase().contains("gnome");
    if result {
        debug!("Detected GNOME Wayland — xdotool will be skipped");
    }
    result
}

/// xdotool only reaches X11 windows. On Wayland it still exits 0 when the
/// target is a native Wayland window, so a "success" would hide the failure.
fn xdotool_can_paste(wayland: bool, gnome_wayland: bool, target: &Target) -> bool {
    !wayland || (target.xwayland && !gnome_wayland)
}

/// Typing text key by key is only safe for a single line of plain ASCII: a
/// typed newline runs the command in a terminal (typing bypasses bracketed
/// paste), and ydotool maps characters with a US keyboard layout.
fn safe_to_type(text: &str, terminal: bool) -> bool {
    !terminal && !text.is_empty() && text.chars().all(|c| c == ' ' || c.is_ascii_graphic())
}

/// Wait until the CopyNinja picker window no longer has focus.
/// Polls every 50ms via hyprctl or xdotool, gives up after ~1s.
fn wait_for_focus_loss() {
    // Check once which tool is available for focus detection
    let has_hyprctl = Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    for i in 0..20 {
        std::thread::sleep(Duration::from_millis(50));

        if has_hyprctl {
            if let Ok(output) = Command::new("hyprctl")
                .args(["activewindow", "-j"])
                .output()
            {
                if output.status.success() {
                    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                        if let Some(class) = json.get("class").and_then(|v| v.as_str()) {
                            if !class.to_lowercase().contains("copyninja") {
                                debug!(
                                    "Focus left picker after {}ms (hyprctl, active: '{}')",
                                    (i + 1) * 50,
                                    class
                                );
                                return;
                            }
                            continue;
                        }
                    }
                }
                // hyprctl returned unexpected result — focus likely moved
                debug!("hyprctl returned unexpected result, assuming focus moved");
                return;
            }
        }

        // Fallback: xdotool (X11/XWayland)
        if let Ok(output) = Command::new("xdotool")
            .args(["getactivewindow", "getwindowclassname"])
            .output()
        {
            if output.status.success() {
                let class = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .to_lowercase();
                if !class.contains("copyninja") {
                    debug!(
                        "Focus left picker after {}ms (xdotool, active: '{}')",
                        (i + 1) * 50,
                        class
                    );
                    return;
                }
            } else {
                // xdotool failed (e.g. no XWayland window focused) — previous window likely has focus
                debug!("xdotool returned error, assuming focus moved away");
                return;
            }
        } else if !has_hyprctl {
            // Neither tool available, fall back to fixed delay
            debug!("No focus detection tool available, using fixed delay");
            std::thread::sleep(Duration::from_millis(200));
            return;
        }
    }
    debug!("Focus poll timed out after 1s, proceeding anyway");
}

/// Send Ctrl+V (or Ctrl+Shift+V) through ydotool. Tries the ydotool 1.x
/// keycode syntax, then the 0.1.x key-name syntax shipped by Debian/Ubuntu.
fn ydotool_key(terminal: bool) -> bool {
    // evdev keycodes: KEY_LEFTCTRL=29, KEY_LEFTSHIFT=42, KEY_V=47
    let v1: &[&str] = if terminal {
        &["key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"]
    } else {
        &["key", "29:1", "47:1", "47:0", "29:0"]
    };
    let v0: &[&str] = if terminal {
        &["key", "ctrl+shift+v"]
    } else {
        &["key", "ctrl+v"]
    };
    for args in [v1, v0] {
        match Command::new("ydotool").args(args).output() {
            Ok(output) if output.status.success() => return true,
            Ok(output) => debug!("ydotool {:?} failed (status {})", args, output.status),
            Err(_) => {
                debug!("ydotool not found");
                return false;
            }
        }
    }
    false
}

/// Simulate paste in the previously focused window.
/// Fallback chain:
///   1. wtype Ctrl+V        — wlroots Wayland (Hyprland, Sway)
///   2. xdotool Ctrl+V      — X11, or XWayland windows (skipped on GNOME Wayland)
///   3. ydotool key Ctrl+V  — GNOME/KDE Wayland (instant paste via uinput)
///   4. ydotool type        — single-line ASCII text only (types it via uinput)
///   5. copy-only notify    — total failure
///
/// Uses Ctrl+Shift+V for terminal emulators in steps 1-3.
pub fn simulate_paste(text: &str, terminal: bool, target: &Target) {
    // Wait for focus to return to the previous window.
    // The picker was just hidden, so the compositor needs time to refocus.
    wait_for_focus_loss();

    let wayland = is_wayland();
    let gnome_wayland = is_gnome_wayland();
    if terminal {
        debug!("Terminal detected — will use Ctrl+Shift+V");
    }

    // On GNOME Wayland, try ydotool key FIRST.
    // Mutter doesn't fully support the wlroots virtual-keyboard-v1 protocol
    // that wtype uses, so wtype often reports success but no paste reaches
    // the focused window (especially for Ctrl+Shift+V into terminals).
    if gnome_wayland {
        if ydotool_key(terminal) {
            debug!("Auto-paste via ydotool key succeeded (GNOME Wayland priority)");
            return;
        }
        debug!("ydotool key failed on GNOME Wayland, falling through to wtype");
    }

    // 1. Try wtype (native Wayland — Hyprland, Sway, wlroots compositors).
    // Modifiers are released explicitly so none stays stuck.
    let wtype_args: &[&str] = if terminal {
        &[
            "-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl",
        ]
    } else {
        &["-M", "ctrl", "-k", "v", "-m", "ctrl"]
    };
    match Command::new("wtype").args(wtype_args).output() {
        Ok(output) if output.status.success() => {
            debug!("Auto-paste via wtype succeeded");
            return;
        }
        Ok(output) => {
            debug!("wtype failed (status {})", output.status);
        }
        Err(_) => {
            debug!("wtype not found");
        }
    }

    // 2. Try xdotool — only where it can actually reach the target window
    if xdotool_can_paste(wayland, gnome_wayland, target) {
        std::thread::sleep(Duration::from_millis(50));
        let _ = Command::new("xdotool")
            .args([
                "keyup", "super", "Super_L", "Super_R", "shift", "Shift_L", "Shift_R", "ctrl",
                "alt",
            ])
            .output();
        std::thread::sleep(Duration::from_millis(50));
        let xdotool_key = if terminal { "ctrl+shift+v" } else { "ctrl+v" };
        match Command::new("xdotool").args(["key", xdotool_key]).output() {
            Ok(output) if output.status.success() => {
                debug!("Auto-paste via xdotool succeeded");
                return;
            }
            Ok(output) => {
                debug!("xdotool failed (status {}), trying ydotool", output.status);
            }
            Err(_) => {
                debug!("xdotool not found, trying ydotool");
            }
        }
    }

    // 3. ydotool key Ctrl+V (instant paste from clipboard via uinput)
    if !gnome_wayland && ydotool_key(terminal) {
        debug!("Auto-paste via ydotool key succeeded");
        return;
    }

    // 4. ydotool type — only where typing can't run commands or garble text
    if safe_to_type(text, terminal) {
        match Command::new("ydotool")
            .args(["type", "--key-delay", "0", "--key-hold", "0", "--file", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                    drop(stdin);
                }
                match child.wait() {
                    Ok(status) if status.success() => {
                        debug!("Auto-paste via ydotool type succeeded");
                        return;
                    }
                    Ok(status) => {
                        warn!("ydotool type failed (status {})", status);
                    }
                    Err(e) => {
                        warn!("ydotool type wait failed ({})", e);
                    }
                }
            }
            Err(_) => {
                debug!("ydotool not found");
            }
        }
    }

    warn!("No paste tool worked (wtype/xdotool/ydotool) — copied to clipboard only");
    notify(
        "Copied to clipboard",
        "Auto-paste is unavailable here. Press Ctrl+V to paste.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_fallback_never_types_risky_text() {
        assert!(safe_to_type("hello world", false));
        assert!(!safe_to_type("rm -rf ~\n", false), "newline would run it");
        assert!(!safe_to_type("ls", true), "terminals get no typing");
        assert!(!safe_to_type("héllo", false), "non-ASCII gets garbled");
        assert!(!safe_to_type("", false), "images have no text");
    }

    #[test]
    fn xdotool_only_where_it_reaches_the_window() {
        let native = Target::default();
        let xwayland = Target {
            class: Some("steam".into()),
            xwayland: true,
        };
        assert!(xdotool_can_paste(false, false, &native), "X11 session");
        assert!(
            !xdotool_can_paste(true, false, &native),
            "KDE Wayland, native window"
        );
        assert!(
            xdotool_can_paste(true, false, &xwayland),
            "KDE Wayland, XWayland window"
        );
        assert!(
            !xdotool_can_paste(true, true, &xwayland),
            "GNOME shows a dialog instead"
        );
    }

    #[test]
    fn terminal_classes() {
        assert!(is_terminal_class("org.gnome.Ptyxis"));
        assert!(is_terminal_class("Alacritty"));
        assert!(is_terminal_class("gnome-terminal-server"));
        assert!(!is_terminal_class("firefox"));
    }
}
