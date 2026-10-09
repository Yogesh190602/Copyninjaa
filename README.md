# CopyNinja

A lightweight clipboard history manager for Linux desktops (Wayland & X11). Provides a **Super+Shift+V** clipboard panel — similar to Windows 11 — with a native GTK4 UI, search, pin, delete, and auto-paste.

Supports **text and images**, **cross-device sync**, and a **runtime config file**.

## Features

- **Clipboard monitoring** — event-driven on Wayland (`wl-paste --watch`), polling on X11 (`xclip`)
- **Text & image support** — captures both text and images (PNG, JPEG, WebP, GIF, BMP) from clipboard
- **GTK4 picker** — keyboard-first, full-text search with highlighted matches, image thumbnails, color swatches for color codes, and tidy previews for links and file paths
- **Themes** — 13 built-in themes (Catppuccin, Tokyo Night, Dracula, Nord, Gruvbox, Rosé Pine, Adwaita), `auto` light/dark switching, custom colors, fonts, size and transparency, plus your own `style.css`
- **Pin entries** — keep frequently used snippets at the top (protected from pruning)
- **Auto-paste** — pastes into the previously focused window after selection (configurable)
- **Terminal-aware** — uses Ctrl+Shift+V in terminals, Ctrl+V elsewhere
- **Deduplication** — duplicate content is moved to the top, not stored twice
- **Cross-device sync** — optional file-based sync via Syncthing, Nextcloud, or any cloud folder
- **Crash recovery** — automatic backup rotation, recovers from corrupt history files
- **Private by default** — history and images are readable only by you; copies marked secret by password managers (KeePassXC, KDE Wallet) are never stored
- **Runtime config** — TOML config file, no rebuild needed to change settings; a bad setting is skipped with a warning instead of resetting everything
- **Multi-DE support** — GNOME, KDE, Hyprland, Sway, i3, and more
- **Systemd integration** — auto-starts on login, restarts on failure
- **Release builds** — GitHub Actions builds and publishes binaries for tagged releases

## Architecture

```
Daemon (copyninja daemon)             Picker (copyninja pick)
 - monitors clipboard (text + images)  - reads ~/.clipboard_history.json
 - detects MIME types automatically     - shows text previews & image thumbnails
 - deduplicates via MD5 hash           - copies selected entry to clipboard
 - stores entries as JSON              - auto-pastes via wtype/xdotool/ydotool
 - runs as systemd user service        - invoked by Super+Shift+V keybinding
 - optional sync watcher               - writes tombstones for sync deletes
```

**Auto-paste fallback chain** (modifier depends on `paste_mode`: `Ctrl+V` for normal, `Ctrl+Shift+V` for terminal):

| Priority | Tool | Environment |
|----------|------|-------------|
| 1 | `ydotool key` | **GNOME Wayland first** (Mutter drops `wtype` events — uinput bypasses it) |
| 2 | `wtype` | wlroots Wayland (Hyprland, Sway) |
| 3 | `xdotool` | X11 sessions, and XWayland windows on Wayland (skipped on GNOME Wayland — triggers Remote Desktop dialog) |
| 4 | `ydotool key` | Other Wayland compositors, e.g. KDE (uinput fallback) |
| 5 | `ydotool type` | Last resort, only for single-line plain-ASCII text and never into terminals (typing a newline would run a command) |
| 6 | Copy-only + notification | If all tools unavailable |

## Installation

### Dependencies

| Package | Purpose | Arch Linux |
|---------|---------|------------|
| `gtk4` | UI framework | `sudo pacman -S gtk4` |
| `wl-clipboard` | Wayland clipboard access | `sudo pacman -S wl-clipboard` |
| `wtype` | Wayland auto-paste | `sudo pacman -S wtype` |
| `xclip` | X11 clipboard access | `sudo pacman -S xclip` |
| `xdotool` | X11 auto-paste | `sudo pacman -S xdotool` |
| `ydotool` | GNOME Wayland auto-paste | `sudo pacman -S ydotool` |
| `libnotify` | Notifications | `sudo pacman -S libnotify` |
| `rustup` | Rust toolchain (only to build from source, Rust 1.89+) | `sudo pacman -S rustup && rustup default stable` |

`install.sh` detects `pacman`, `apt`, `dnf` and `zypper` and offers to install whatever is missing under the right package names.

### Install

```bash
cd copyninja-rs
./install.sh
```

This will:
1. **Acquire the binary** — tries to download a prebuilt binary from the latest [GitHub Release](https://github.com/Yogesh190602/Copyninjaa/releases) matching your architecture (`x86_64` / `aarch64`), but only if that release is the same version as your checkout; otherwise it builds your checkout from source (when Rust is installed). Verifies the SHA256 (if published) and that the binary actually runs on your system. Falls back to building from source with `cargo build --release` if any step fails (no release, glibc too old, download failed, unusual architecture). Force source build with `COPYNINJA_BUILD_FROM_SOURCE=1 ./install.sh`.
2. Install to `~/.local/bin/copyninja`
3. Set up the systemd user service (starts at login on every desktop, including plain Hyprland/Sway/i3)
4. Configure the Super+Shift+V keybinding for your DE
5. On **Wayland**, offer to install a udev rule giving your desktop session write access to `/dev/uinput`, which `ydotool` needs to send the paste keystroke. This grants access to `/dev/uinput` only; the old approach of joining the `input` group would let every program read all keyboards, so it's only offered as a fallback.
6. On **GNOME Wayland only**, create a default `~/.config/copyninja/config.toml` with `paste_mode = "terminal"` — because auto-detection is impossible on GNOME Wayland (see [Paste mode](#paste-mode) below). Never overwrites an existing config.

### Update

Pull the new code and run `./install.sh` again. It replaces the installed binary in place, even while the daemon is running, restarts the daemon and keeps your history, config and keybinding. There's no need to uninstall first.

### Uninstall

```bash
./uninstall.sh
```

## Usage

The daemon starts automatically on login. Open the picker with **Super+Shift+V**.

### Picker Keybindings

| Key | Action |
|-----|--------|
| Type anything | Search the full text of every clip (several words narrow the results) |
| `↑` / `↓`, `Page Up` / `Page Down` | Move the selection |
| `Enter` or click | Copy & auto-paste the selected clip |
| `Ctrl+P` | Pin / unpin the selected clip |
| `Ctrl+D` | Delete the selected clip |
| `Ctrl+L` | Clear history (press again to confirm; pinned clips are kept) |
| `Escape` | Clear the search, or close the picker |

Hovering a clip shows its pin and delete buttons. Pressing the hotkey while the picker is open brings the existing window back instead of opening a second one.

### Service Commands

```bash
systemctl --user status copyninja       # Check daemon status
systemctl --user restart copyninja      # Restart daemon
journalctl --user -u copyninja -f       # Live logs
copyninja --version                      # Show version
./doctor.sh                              # Installation health check
```

### Health check

If auto-paste silently fails or something seems off, run the bundled diagnostic:

```bash
./doctor.sh
```

It verifies: binary presence, daemon status, config file problems, GNOME keybinding registration, `ydotoold` running, `/dev/uinput` access, history file permissions, all required tools, and prints any recent daemon warnings. For each failure it tells you exactly which command to run to fix it. Non-destructive, safe to run any time.

### D-Bus Interface

Add entries programmatically:

```bash
dbus-send --session /com/copyninja/Daemon com.copyninja.Daemon.NewEntry string:"Some text"
```

## Configuration

Create `~/.config/copyninja/config.toml` to customize settings. All fields are optional — missing fields use defaults.

```toml
max_entries = 50          # Max clipboard history entries
max_backups = 3           # Number of backup files for crash recovery
log_level = "info"        # Logging: error, warn, info, debug
auto_paste = true         # Auto-paste after selecting an entry
paste_mode = "auto"       # Paste shortcut: "auto", "terminal", or "normal"
max_image_size_mb = 10    # Larger images are not saved
max_text_size_kb = 1024   # Larger text is not saved

# Optional: cross-device sync
[sync]
enabled = false
sync_dir = ""             # e.g. "~/Syncthing/copyninja"
```

`~` is expanded in paths. A setting with a bad value or an unknown name is skipped (the rest still applies) and reported in the daemon log (`journalctl --user -u copyninja`) and by `./doctor.sh`. The picker reads the config each time it opens; restart the daemon after changing settings it uses (`max_entries`, `history_file`, sync, …).

| Setting | Default | Description |
|---------|---------|-------------|
| `max_entries` | 50 | Maximum clipboard history entries (pinned entries are never pruned) |
| `max_backups` | 3 | Backup file count for crash recovery |
| `history_file` | `~/.clipboard_history.json` | History file path |
| `log_level` | `info` | Log verbosity |
| `auto_paste` | `true` | Auto-paste after selection |
| `paste_mode` | `"auto"` | Paste shortcut mode — see [Paste mode](#paste-mode) below |
| `image_dir` | `~/.local/share/copyninja/images/` | Image storage directory |
| `max_image_size_mb` | 10 | Max image size to capture (MB) |
| `max_text_size_kb` | 1024 | Max text size to capture (KB) |
| `sync.enabled` | `false` | Enable cross-device sync |
| `sync.sync_dir` | _(empty)_ | Path to sync folder |
| `appearance.*` | see [Appearance](#appearance) | Theme, colors, fonts, size, opacity |

### Appearance

Pick a theme and fine-tune the picker under `[appearance]`. Run `copyninja themes` to list the built-in themes.

```toml
[appearance]
theme = "catppuccin-mocha"   # or "auto" to follow your system's light/dark setting
dark_theme = "catppuccin-mocha"   # used by "auto"
light_theme = "catppuccin-latte"  # used by "auto"
font = ""                    # interface font, e.g. "Inter" ("" = system font)
mono_font = "monospace"      # font for clip text, e.g. "JetBrains Mono"
font_size = 13               # base text size in pixels (8–32)
width = 460                  # window size in pixels
height = 580
opacity = 1.0                # 0.5–1.0; below 1.0 the background turns translucent

# Override any color of the chosen theme
[appearance.colors]
accent = "#89b4fa"
```

| Theme | Kind |
|-------|------|
| `catppuccin-mocha` *(default)*, `catppuccin-macchiato`, `catppuccin-frappe` | dark |
| `catppuccin-latte` | light |
| `tokyo-night`, `dracula`, `nord`, `gruvbox-dark`, `rose-pine`, `adwaita-dark` | dark |
| `gruvbox-light`, `rose-pine-dawn`, `adwaita` | light |
| `auto` | `dark_theme` or `light_theme`, following the system setting |

Colors you can override: `background` (window), `bar` (search bar and footer), `border` (dividers), `surface` (search field, selected clip, key caps), `surface_strong` (hovered buttons), `text`, `subtext` (secondary text on the selected clip), `muted` (times, hints, placeholders), `accent` (selection edge, focus ring, search highlights), `pin` and `danger`. Use `#rgb` or `#rrggbb`.

For anything else, create `~/.config/copyninja/style.css`. It's GTK CSS, applied on top of the theme. Every rule should start with `window.copyninja`. These classes are available:

| Selector | What it styles |
|----------|----------------|
| `.topbar`, `entry.search-field` | Search bar and field |
| `list.clips > row.clip`, `row.clip:selected` | A clip, and the selected one |
| `row.section`, `.section-label` | "Pinned" / "Recent" headings |
| `.clip-text` | Text previews (monospace) |
| `.clip-title`, `.clip-sub`, `.clip-meta`, `.clip-time` | Titles, link addresses and folders, details, times |
| `.thumb`, `.pin-mark` | Image thumbnails, the pin mark |
| `button.row-action`, `.pinned`, `.delete` | Pin and delete buttons |
| `.footer`, `.key`, `.hint`, `.status`, `button.clear` | Footer |
| `.empty-icon`, `.empty-title`, `.empty-body` | Empty and no-match messages |

Theme colors are available in `style.css` as `@cn_background`, `@cn_accent`, and so on. For example:

```css
window.copyninja list.clips > row.clip:selected { box-shadow: inset 4px 0 @cn_accent; }
window.copyninja .clip-text { font-size: 13px; }
```

The picker reads the config and `style.css` each time it opens, so changes show up the next time you press Super+Shift+V.

### Paste mode

Controls which keyboard shortcut the auto-paste simulates:

| Value | Shortcut | When to use |
|-------|----------|-------------|
| `"auto"` *(default)* | Detects focused window class — `Ctrl+Shift+V` in terminals, `Ctrl+V` elsewhere | Most wlroots compositors (Hyprland, Sway) |
| `"terminal"` | Always `Ctrl+Shift+V` | GNOME Wayland users who mainly paste into terminals (see note below) |
| `"normal"` | Always `Ctrl+V` | GNOME Wayland users who mainly paste into text fields / browsers |

#### ⚠️ GNOME Wayland users — read this

On **GNOME Wayland**, `"auto"` detection **does not work for native Wayland terminals** (Ghostty, GNOME Console/kgx, kitty, alacritty, etc.). Mutter does not expose the focused window class to external apps — `xdotool` returns `(null)` and `org.gnome.Shell.Introspect.GetWindows` is blocked (`AccessDenied`) on recent GNOME versions. Without a shell extension like *Window Calls*, no public API can identify the focused window.

As a result, `paste_mode = "auto"` will silently default to `Ctrl+V` for native Wayland windows, which terminals ignore — **auto-paste appears to do nothing**.

**Fix:** explicitly set the mode. For terminal-heavy workflows:

```bash
mkdir -p ~/.config/copyninja
cat > ~/.config/copyninja/config.toml <<'EOF'
paste_mode = "terminal"
EOF
```

Tradeoff for `"terminal"` mode: `Ctrl+Shift+V` works in terminals and in most browsers (as "paste without formatting"), but in GTK text fields it opens the Unicode entry dialog instead of pasting, and in VS Code it toggles Markdown preview. If that bothers you, use `"normal"` mode instead and accept that terminal paste won't work — or file an issue asking for per-keybinding `--terminal` / `--normal` CLI flags.

On **wlroots compositors** (Hyprland, Sway) and **X11 sessions**, leave `paste_mode = "auto"` — detection works correctly there.

## Cross-Device Sync

CopyNinja supports syncing clipboard history across machines using any file-sync tool (Syncthing, Nextcloud, Dropbox, etc.).

### Setup

1. Create a shared folder (e.g. `~/Syncthing/copyninja`)
2. Add to your config:
   ```toml
   [sync]
   enabled = true
   sync_dir = "~/Syncthing/copyninja"
   ```
3. Restart the daemon: `systemctl --user restart copyninja`
4. Repeat on other machines

### How it works

- Each clipboard entry is a JSON file in `sync_dir/entries/`, written as soon as you copy, pin or unpin it. Images are copied to `sync_dir/images/`
- Deleting a clip (or clearing history) writes a tombstone in `sync_dir/deleted/`, which removes it on every device. Copying the same thing again later brings it back
- When devices disagree (pinned on one, unpinned on another) the most recent change wins
- Each device keeps its newest `max_entries` clips, and the shared folder is trimmed to match; tombstones expire after 30 days
- Everything read from the folder is validated, so a synced file can never make CopyNinja read or delete files outside its own directories

## Desktop Environment Support

| DE | Clipboard | Auto-paste | Keybinding |
|----|-----------|------------|------------|
| Hyprland | wl-paste | wtype | Auto-configured |
| Sway | wl-paste | wtype | Auto-configured |
| GNOME (Wayland) | wl-paste | ydotool | Auto-configured |
| KDE (Wayland) | wl-paste | ydotool | Manual |
| i3 (X11) | xclip | xdotool | Auto-configured |
| XFCE (X11) | xclip | xdotool | Manual |

## Project Structure

```
copyninja-rs/
├── src/
│   ├── main.rs              # CLI entry point (daemon/pick/themes subcommands)
│   ├── config.rs             # TOML config loading with defaults
│   ├── content.rs            # ClipContent enum (Text/Image)
│   ├── storage.rs            # History storage, backup rotation, dedup, pruning
│   ├── sync.rs               # Cross-device sync (export, import, tombstones, watcher)
│   ├── theme.rs              # Built-in color themes, resolving [appearance]
│   ├── daemon/
│   │   ├── mod.rs            # Daemon orchestration + retry loop
│   │   ├── session.rs        # Wayland/X11 session detection
│   │   ├── wayland.rs        # wl-paste --watch + MIME type detection
│   │   ├── x11.rs            # xclip polling + MIME type detection
│   │   └── dbus.rs           # D-Bus service
│   └── picker/
│       ├── mod.rs            # Picker entry point
│       ├── app.rs            # GTK4 UI, search, keybindings, image thumbnails
│       ├── paste.rs          # Auto-paste fallback chain + image clipboard
│       ├── preview.rs        # Row previews, search highlighting, clip kinds
│       └── css.rs            # Stylesheet generated from the theme
├── Cargo.toml
├── CHANGELOG.md
├── install.sh
└── uninstall.sh
```

## Development

```bash
cargo build --release        # Build
cargo test                   # Run the unit tests
cargo clippy                 # Lint
cargo fmt                    # Format
```

Release binaries are built by GitHub Actions (`.github/workflows/release.yml`) when a `v*` tag is pushed.

## Known Limitations

- **GNOME Wayland terminal detection** — auto-detection of focused native Wayland terminals is impossible without a shell extension. Set `paste_mode = "terminal"` in `~/.config/copyninja/config.toml` if auto-paste silently fails in your terminal. See [Paste mode](#paste-mode) for details.
- **GNOME/KDE Wayland auto-paste** — requires `ydotoold` running and write access to `/dev/uinput`. `install.sh` sets up both (`ydotool.service` and a udev rule). If auto-paste still shows "Auto-paste is unavailable", run `./doctor.sh`; if you used the `input` group fallback, log out and back in.
- **Image auto-paste** — always fires `Ctrl+V` regardless of `paste_mode`, since `Ctrl+Shift+V` never pastes images in any common app. Pasting works in browsers, image editors (GIMP, Inkscape), document apps (LibreOffice), etc. Terminals cannot accept image paste.
- **File-manager image copy** — Ctrl+C on an image file in Nautilus/Files/Nemo/Thunar/Dolphin puts a file URI in the clipboard, not image bytes. CopyNinja detects this, reads the file, and stores it as a proper image entry.
- **Sync and `max_entries`** — each device trims the shared folder to its own `max_entries`, so use the same limit on every synced device

## License

MIT
