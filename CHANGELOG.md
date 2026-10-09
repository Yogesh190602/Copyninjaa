# Changelog

## [1.2.0] - 2026-10-09

### Added
- Redesigned picker: the search field is the title bar and shows the clip count; search covers the full text, several words narrow the results, and matches are highlighted (the preview jumps to a match further in the text); text shows in monospace with ↵ marking line breaks; images get cached thumbnails with their dimensions and size; pin and delete buttons appear on hover; the footer shows keyboard shortcuts
- Themes: 13 built-in themes (`copyninja themes` lists them) and `theme = "auto"` to follow the system's light/dark setting, set under a new `[appearance]` config section along with per-color overrides, fonts, text size, window size and background opacity; `~/.config/copyninja/style.css` is applied on top for anything else
- Smarter previews: color codes show a swatch, links show their site, and file paths show the file names and folder
- Hovering a clip swaps its time for the pin and delete buttons
- Picker keys: ↑/↓ and Page Up/Page Down move the selection while typing in the search field; Escape clears the search before closing the picker
- `max_text_size_kb` config option (default: 1024)
- Sync now copies images (`sync_dir/images/`), syncs pins and unpins in both directions (the newest change wins), and deleting or clearing a clip removes it on every device
- Fallback to the ydotool 0.1.x key syntax shipped by Debian/Ubuntu
- `install.sh` supports apt, dnf and zypper as well as pacman
- `doctor.sh` checks for config problems, `/dev/uinput` access and history file permissions

### Changed
- Text is stored exactly as copied, including leading and trailing whitespace
- `install.sh` grants `/dev/uinput` access with a udev rule instead of adding the user to the `input` group (offered only as a fallback, with a warning)
- The systemd unit is also `WantedBy=default.target`, so the daemon starts at login on Hyprland, sway and i3
- New entries no longer duplicate their text in the legacy `text`/`preview` fields (old files still load)
- A bad or unknown config key is skipped with a warning; the rest of the config still applies
- `paste_mode` only accepts `auto`, `terminal` or `normal`
- Pressing the hotkey while the picker is open shows the existing window instead of opening a second one
- Building from source needs Rust 1.89 or newer

### Fixed
- Clipboard entries were lost when the daemon, picker and sync thread wrote the history at the same time; every change now holds a file lock
- Config errors were never logged, and one bad value reset every setting to its default
- `~` in config paths wasn't expanded
- `max_image_size_mb` was ignored, and copying a huge file in a file manager read it all into memory
- Sync: new copies weren't exported until another device wrote, imports ignored `max_entries`, pruned clips came back, deletions didn't reach other devices and were exported again, pins from other devices were dropped, and the sync folder grew forever
- Auto-paste did nothing on KDE Wayland
- Pasting an image whose file was missing pasted the previous clipboard contents
- Search only matched the first 100 characters of a clip
- Once pinned clips filled `max_entries`, every new copy was dropped
- Terminals with reverse-DNS app IDs (Ghostty, Ptyxis, WezTerm) weren't detected for `paste_mode = "auto"`
- The X11 watcher read the whole clipboard image twice a second; it now uses the selection timestamp when available
- The daemon could fail to find the display when started before the desktop session, and changed environment variables unsafely while other threads were running
- Re-running `install.sh` while the daemon was running failed with "Text file busy"; it now replaces the binary in place and restarts the daemon, so updating no longer needs an uninstall
- `install.sh` installed the latest GitHub release even when the checkout was newer; it now builds the checkout from source unless the release is the same version
- `install.sh` overwrote and could delete the user's `TMPDIR`; `uninstall.sh` could delete user config lines that followed the window rules
- A manually started release built the default branch instead of the requested tag

### Security
- History, backups and images are readable only by the user (0600 files, 0700 image directory)
- Clips that password managers mark as secret (`x-kde-passwordManagerHint`) are not stored
- The sync folder is treated as untrusted: entry names are validated and image paths are rebuilt locally, so a file in the shared folder can no longer make CopyNinja delete or overwrite other files
- The last-resort `ydotool type` fallback only types single-line ASCII text and never into a terminal, where a typed newline would run a command

## [1.1.0] - 2026-03-23

### Added
- Runtime TOML config file (`~/.config/copyninja/config.toml`)
  - `max_entries` — max clipboard history entries (default: 50)
  - `max_backups` — backup file count for recovery (default: 3)
  - `history_file` — custom history file path
  - `log_level` — logging verbosity (default: info)
  - `auto_paste` — enable/disable auto-paste after selection (default: true)
- `copyninja --version` flag
- Corrupt JSON recovery with automatic backup rotation
- Pinned entries now survive pruning (bug fix)

### Fixed
- Pinned entries could be evicted when history reached max_entries

## [1.0.0] - 2026-03-22

### Added
- Initial Rust rewrite from Python
- Clipboard monitoring daemon (Wayland + X11)
- GTK4 picker UI with search, pin, delete, clear-all
- Auto-paste via wtype/xdotool/ydotool fallback chain
- Systemd user service integration
- D-Bus interface for external clipboard entry submission
- Catppuccin Mocha dark theme
