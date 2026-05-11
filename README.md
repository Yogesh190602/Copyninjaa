# CopyNinja

A lightweight clipboard history manager for Linux desktops (Wayland & X11). Provides a **Super+Shift+V** clipboard panel — similar to Windows 11 — with a native GTK4 UI, search, pin, delete, and auto-paste.

Supports **text and images**, **cross-device sync**, and a **runtime config file**.

## Features

- **Clipboard monitoring** — event-driven on Wayland (`wl-paste --watch`), polling on X11 (`xclip`)
- **Text & image support** — captures both text and images (PNG, JPEG, WebP, GIF, BMP) from clipboard
- **GTK4 picker** — dark Catppuccin Mocha theme, live search, image thumbnails, relative timestamps
- **Pin entries** — keep frequently used snippets at the top (protected from pruning)
- **Auto-paste** — pastes into the previously focused window after selection (configurable)
- **Terminal-aware** — uses Ctrl+Shift+V in terminals, Ctrl+V elsewhere
- **Deduplication** — duplicate content is moved to the top, not stored twice
- **Cross-device sync** — optional file-based sync via Syncthing, Nextcloud, or any cloud folder
- **Crash recovery** — automatic backup rotation, recovers from corrupt history files
- **Runtime config** — TOML config file, no rebuild needed to change settings
- **Multi-DE support** — GNOME, KDE, Hyprland, Sway, i3, and more
- **Systemd integration** — auto-starts on login, restarts on failure
- **CI/CD** — GitHub Actions for build, lint, test, and release

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
| 3 | `xdotool` | X11 (skipped on GNOME Wayland — triggers Remote Desktop dialog) |
| 4 | `ydotool key` | All other Wayland compositors (uinput fallback) |
| 5 | `ydotool type` | Final fallback — types char-by-char via uinput |
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
| `rustup` | Rust toolchain | `sudo pacman -S rustup && rustup default stable` |

### Install

```bash
cd copyninja-rs
./install.sh
```

This will:
1. **Acquire the binary** — tries to download a prebuilt binary from the latest [GitHub Release](https://github.com/Yogesh190602/Copyninjaa/releases) matching your architecture (`x86_64` / `aarch64`). Verifies the SHA256 (if published) and that the binary actually runs on your system. Falls back to building from source with `cargo build --release` if any step fails (no release, glibc too old, download failed, unusual architecture). Force source build with `COPYNINJA_BUILD_FROM_SOURCE=1 ./install.sh`.
2. Install to `~/.local/bin/copyninja`
3. Set up the systemd user service
4. Configure the Super+Shift+V keybinding for your DE
5. On **GNOME Wayland only**, create a default `~/.config/copyninja/config.toml` with `paste_mode = "terminal"` — because auto-detection is impossible on GNOME Wayland (see [Paste mode](#paste-mode) below). Never overwrites an existing config.

### Uninstall

```bash
./uninstall.sh
```

## Usage

The daemon starts automatically on login. Open the picker with **Super+Shift+V**.

### Picker Keybindings

| Key | Action |
|-----|--------|
| `Enter` | Copy & auto-paste selected entry |
| `Ctrl+P` | Toggle pin on selected entry |
| `Ctrl+D` | Delete selected entry |
| `Ctrl+L` | Clear all (two-step confirmation) |
| `Escape` | Close picker |
| Type anything | Live search filter |

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

It verifies: binary presence, daemon status, config file, GNOME keybinding registration, `ydotoold` running, `input` group membership, `/dev/uinput`, all required tools, and prints any recent daemon warnings. For each failure it tells you exactly which command to run to fix it. Non-destructive, safe to run any time.

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
max_image_size_mb = 10    # Max image size to capture

# Optional: cross-device sync
[sync]
enabled = false
sync_dir = ""             # e.g. "~/Syncthing/copyninja"
```

| Setting | Default | Description |
|---------|---------|-------------|
| `max_entries` | 50 | Maximum clipboard history entries |
| `max_backups` | 3 | Backup file count for crash recovery |
| `history_file` | `~/.clipboard_history.json` | History file path |
| `log_level` | `info` | Log verbosity |
| `auto_paste` | `true` | Auto-paste after selection |
| `paste_mode` | `"auto"` | Paste shortcut mode — see [Paste mode](#paste-mode) below |
| `image_dir` | `~/.local/share/copyninja/images/` | Image storage directory |
| `max_image_size_mb` | 10 | Max image size to capture (MB) |
| `sync.enabled` | `false` | Enable cross-device sync |
| `sync.sync_dir` | _(empty)_ | Path to sync folder |

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
   sync_dir = "/home/youruser/Syncthing/copyninja"
   ```
3. Restart the daemon: `systemctl --user restart copyninja`
4. Repeat on other machines

### How it works

- Each clipboard entry is exported as an individual JSON file in `sync_dir/entries/`
- File-sync tools handle file creation/deletion atomically — no merge conflicts
- Deleted entries create tombstone files in `sync_dir/deleted/` to prevent re-import
- Pinned state uses OR logic: if pinned on any device, it stays pinned everywhere
- A unique device ID is generated per machine in `~/.config/copyninja/device_id`

## Desktop Environment Support

| DE | Clipboard | Auto-paste | Keybinding |
|----|-----------|------------|------------|
| Hyprland | wl-paste | wtype | Auto-configured |
| Sway | wl-paste | wtype | Auto-configured |
| GNOME (Wayland) | wl-paste | ydotool | Auto-configured |
| KDE (Wayland) | wl-paste | wtype/ydotool | Manual |
| i3 (X11) | xclip | xdotool | Auto-configured |
| XFCE (X11) | xclip | xdotool | Manual |

## Project Structure

```
copyninja-rs/
├── src/
│   ├── main.rs              # CLI entry point (daemon/pick subcommands)
│   ├── config.rs             # TOML config loading with defaults
│   ├── content.rs            # ClipContent enum (Text/Image)
│   ├── storage.rs            # History storage, backup rotation, dedup, pruning
│   ├── sync.rs               # Cross-device sync (export, import, tombstones, watcher)
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
│       └── css.rs            # Catppuccin Mocha theme
├── Cargo.toml
├── CHANGELOG.md
├── install.sh
└── uninstall.sh
```

## Development

```bash
cargo build --release        # Build
cargo test                   # Run 18 unit tests
cargo clippy                 # Lint
cargo fmt                    # Format
```

CI runs automatically on push/PR via GitHub Actions (`.github/workflows/ci.yml`).

## Known Limitations

- **GNOME Wayland terminal detection** — auto-detection of focused native Wayland terminals is impossible without a shell extension. Set `paste_mode = "terminal"` in `~/.config/copyninja/config.toml` if auto-paste silently fails in your terminal. See [Paste mode](#paste-mode) for details.
- **GNOME Wayland auto-paste** — requires `ydotoold` running and the user in the `input` group. `install.sh` handles both automatically: enables `systemctl --user enable --now ydotool.service` and runs `sudo usermod -aG input $USER`. **The group change requires a logout/reboot to take effect.** If auto-paste shows "Auto-paste unavailable" on first run after install, log out and back in, then retry.
- **Image auto-paste** — always fires `Ctrl+V` regardless of `paste_mode`, since `Ctrl+Shift+V` never pastes images in any common app. Pasting works in browsers, image editors (GIMP, Inkscape), document apps (LibreOffice), etc. Terminals cannot accept image paste.
- **File-manager image copy** — Ctrl+C on an image file in Nautilus/Files/Nemo/Thunar/Dolphin puts a file URI in the clipboard, not image bytes. CopyNinja detects this, reads the file, and stores it as a proper image entry.
- **Sync conflicts** — concurrent writes from multiple devices within the same second may cause a race; file-sync tools handle this gracefully in practice

## License

MIT
