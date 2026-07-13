#!/usr/bin/env bash
# install.sh — One-shot installer for CopyNinja (Rust edition)
# Run as your normal user (NOT root).
set -euo pipefail

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; CYAN='\033[0;36m'; NC='\033[0m'
info()  { echo -e "${GREEN}[✓]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
error() { echo -e "${RED}[✗]${NC} $*"; exit 1; }
step()  { echo -e "${CYAN}[→]${NC} $*"; }

# ── Refuse to run as root ─────────────────────────────────────────────────
# CopyNinja installs a *user* systemd service and per-user keybindings/config.
# Running under sudo makes $HOME=/root (binary lands in /root/.local/bin) and
# `systemctl --user` fails with "$DBUS_SESSION_BUS_ADDRESS not defined" because
# root has no graphical user session bus. Run as your normal user — the script
# calls `sudo` itself only for the steps that genuinely need root.
if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
    if [[ -n "${SUDO_USER:-}" && "$SUDO_USER" != "root" ]]; then
        error "Don't run this with sudo. Re-run it as your normal user:

    ./install.sh

The script will prompt for sudo only when it needs to (installing packages,
adding you to the 'input' group)."
    else
        error "Don't run this as root. Log in as your normal desktop user and run:

    ./install.sh"
    fi
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY_NAME="copyninja"
INSTALL_DIR="$HOME/.local/bin"

# ── 0. Detect session ─────────────────────────────────────────────────────
SESSION_TYPE="${XDG_SESSION_TYPE:-unknown}"
DESKTOP="${XDG_CURRENT_DESKTOP:-unknown}"

# If XDG vars aren't set (SSH, TTY, etc.), detect from running processes
if [[ "$SESSION_TYPE" == "unknown" || "$SESSION_TYPE" == "tty" ]]; then
    if command -v loginctl &>/dev/null; then
        GRAPHICAL_SESSION=$(loginctl list-sessions --no-legend 2>/dev/null \
            | awk '{print $1}' \
            | while read -r sid; do
                stype=$(loginctl show-session "$sid" -p Type --value 2>/dev/null)
                if [[ "$stype" == "wayland" || "$stype" == "x11" ]]; then
                    echo "$stype"
                    break
                fi
            done)
        if [[ -n "${GRAPHICAL_SESSION:-}" ]]; then
            SESSION_TYPE="$GRAPHICAL_SESSION"
        fi
    fi
    if [[ "$SESSION_TYPE" == "unknown" || "$SESSION_TYPE" == "tty" ]]; then
        if pgrep -x "Hyprland|sway|mutter|kwin_wayland|weston" &>/dev/null; then
            SESSION_TYPE="wayland"
        elif pgrep -x "Xorg|Xwayland|i3|openbox|xfwm4" &>/dev/null; then
            SESSION_TYPE="x11"
        fi
    fi
fi

if [[ "$DESKTOP" == "unknown" ]]; then
    if pgrep -x "gnome-shell" &>/dev/null; then
        DESKTOP="GNOME"
    elif pgrep -x "Hyprland" &>/dev/null; then
        DESKTOP="Hyprland"
    elif pgrep -x "sway" &>/dev/null; then
        DESKTOP="sway"
    elif pgrep -x "i3" &>/dev/null; then
        DESKTOP="i3"
    elif pgrep -x "plasmashell" &>/dev/null; then
        DESKTOP="KDE"
    elif pgrep -x "xfce4-session" &>/dev/null; then
        DESKTOP="XFCE"
    fi
fi

info "Detected session: $SESSION_TYPE ($DESKTOP)"

if [[ "$SESSION_TYPE" != "wayland" && "$SESSION_TYPE" != "x11" ]]; then
    warn "Could not detect session type. Proceeding anyway — installing both Wayland and X11 tools."
    SESSION_TYPE="both"
fi

# ── 1. Check runtime dependencies ─────────────────────────────────────────
# The Rust binary has no Python/PyGObject dependency.
# Only external tools used at runtime need to be present.
step "Checking runtime dependencies…"

MISSING=()
PACKAGES_TO_INSTALL=()

# notify-send (optional, for future use)
command -v notify-send &>/dev/null || { MISSING+=("notify-send"); PACKAGES_TO_INSTALL+=("libnotify"); }

# xclip + xdotool always needed (fallback for GNOME Wayland via XWayland + X11)
command -v xclip   &>/dev/null || { MISSING+=("xclip");   PACKAGES_TO_INSTALL+=("xclip"); }
command -v xdotool &>/dev/null || { MISSING+=("xdotool"); PACKAGES_TO_INSTALL+=("xdotool"); }

if [[ "$SESSION_TYPE" == "wayland" || "$SESSION_TYPE" == "both" ]]; then
    command -v wl-paste &>/dev/null || { MISSING+=("wl-paste"); PACKAGES_TO_INSTALL+=("wl-clipboard"); }
    command -v wtype    &>/dev/null || { MISSING+=("wtype");    PACKAGES_TO_INSTALL+=("wtype"); }
fi

# Check GTK4 system library (needed by the Rust binary at runtime)
if ! pkg-config --exists gtk4 2>/dev/null; then
    MISSING+=("gtk4")
    PACKAGES_TO_INSTALL+=("gtk4")
fi

# Deduplicate
if [[ ${#PACKAGES_TO_INSTALL[@]} -gt 0 ]]; then
    PACKAGES_TO_INSTALL=($(printf '%s\n' "${PACKAGES_TO_INSTALL[@]}" | sort -u))
fi

if [[ ${#MISSING[@]} -gt 0 ]]; then
    warn "Missing: ${MISSING[*]}"
    echo "Packages needed: ${PACKAGES_TO_INSTALL[*]}"
    echo ""
    read -rp "Auto-install now? [y/N] " answer
    if [[ "$answer" =~ ^[Yy]$ ]]; then
        sudo pacman -S --needed --noconfirm "${PACKAGES_TO_INSTALL[@]}"
        info "Dependencies installed."
    else
        error "Please install missing dependencies manually, then re-run."
    fi
else
    info "All runtime dependencies found."
fi

# ── 1b. Wayland auto-paste plumbing: ydotoold + input group ───────────────
# On GNOME Wayland (and KDE Wayland), wtype often gets its events dropped
# by the compositor, and auto-paste falls back to ydotool — which needs
# two things that the Arch `ydotool` package does NOT set up for you:
#
#   1. ydotoold must be running (user systemd unit: ydotool.service)
#   2. The user must be in the `input` group to access /dev/uinput
#
# Without these the user sees a "Auto-paste unavailable" toast and has
# no idea why. Fix it here, once.
GROUP_ADDED=0

if [[ "$SESSION_TYPE" == "wayland" || "$SESSION_TYPE" == "both" ]] && command -v ydotool &>/dev/null; then
    step "Configuring ydotool for auto-paste…"

    # Enable the ydotoold user service if the unit exists and isn't already active.
    if systemctl --user list-unit-files 2>/dev/null | grep -q '^ydotool\.service'; then
        if ! systemctl --user is-active ydotool.service &>/dev/null; then
            systemctl --user enable --now ydotool.service 2>&1 | tail -1 || true
            if systemctl --user is-active ydotool.service &>/dev/null; then
                info "Enabled and started ydotool.service (user unit)."
            else
                warn "Failed to start ydotool.service — auto-paste via ydotool may not work."
            fi
        else
            info "ydotool.service already running."
        fi
    else
        warn "ydotool.service user unit not found — auto-paste via ydotool will be unavailable."
        echo "  The 'ydotool' package on Arch installs the unit at /usr/lib/systemd/user/ydotool.service."
        echo "  If it's missing, reinstall: sudo pacman -S ydotool"
    fi

    # Check input group membership — required for /dev/uinput access.
    if ! id -nG "$USER" 2>/dev/null | tr ' ' '\n' | grep -qx 'input'; then
        step "Adding $USER to the 'input' group (needed for /dev/uinput)…"
        if sudo usermod -aG input "$USER"; then
            GROUP_ADDED=1
            info "Added $USER to 'input' group."
        else
            warn "Failed to add $USER to 'input' group — auto-paste via ydotool will fail."
            echo "  Run manually: sudo usermod -aG input $USER"
        fi
    else
        info "$USER is already in the 'input' group."
    fi
fi

# ── 2. Acquire binary: prefer prebuilt from GitHub, fall back to source ───
GITHUB_REPO="Yogesh190602/Copyninjaa"
BUILT_BINARY=""

# Map `uname -m` to Rust target triples for release asset naming.
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64|amd64)  RUST_TARGET="x86_64-unknown-linux-gnu" ;;
    aarch64|arm64) RUST_TARGET="aarch64-unknown-linux-gnu" ;;
    *)             RUST_TARGET="" ;;
esac

# Allow the user to skip the download entirely.
if [[ "${COPYNINJA_BUILD_FROM_SOURCE:-0}" == "1" ]]; then
    warn "COPYNINJA_BUILD_FROM_SOURCE=1 set — skipping prebuilt binary download."
elif [[ -z "$RUST_TARGET" ]]; then
    warn "No prebuilt binary available for architecture '$ARCH' — will build from source."
elif ! command -v curl &>/dev/null; then
    warn "curl not found — cannot download prebuilt binary, will build from source."
else
    step "Looking for prebuilt binary on GitHub ($GITHUB_REPO)…"

    # Ask the GitHub API for the latest release. Unauthenticated requests are
    # rate-limited to 60/hour/IP — enough for a one-shot install script.
    # The whole block is wrapped in `|| true` at each step because
    # `set -e -o pipefail` would otherwise abort the script whenever grep
    # finds no match (e.g. the repo has no releases yet).
    API_URL="https://api.github.com/repos/$GITHUB_REPO/releases/latest"
    RELEASE_JSON="$(curl -fsSL -H 'Accept: application/vnd.github+json' "$API_URL" 2>/dev/null || true)"

    ASSET_NAME="copyninja-$RUST_TARGET.tar.gz"
    ASSET_URL=""
    RELEASE_TAG=""
    if [[ -n "$RELEASE_JSON" ]]; then
        # Extract the download URL for our asset from the JSON. Fragile but
        # avoids a jq dependency for the install script.
        ASSET_URL="$(printf '%s' "$RELEASE_JSON" \
            | grep -oE "\"browser_download_url\"[[:space:]]*:[[:space:]]*\"[^\"]*$ASSET_NAME\"" 2>/dev/null \
            | sed -E 's/.*"([^"]+)"$/\1/' \
            | head -n1 || true)"
        RELEASE_TAG="$(printf '%s' "$RELEASE_JSON" \
            | grep -oE '"tag_name"[[:space:]]*:[[:space:]]*"[^"]+"' 2>/dev/null \
            | sed -E 's/.*"([^"]+)"$/\1/' \
            | head -n1 || true)"
    fi

    if [[ -n "$ASSET_URL" ]]; then
        info "Found release $RELEASE_TAG → $ASSET_NAME"
        TMPDIR="$(mktemp -d)"
        trap 'rm -rf "$TMPDIR"' EXIT

        if curl -fsSL --retry 2 -o "$TMPDIR/$ASSET_NAME" "$ASSET_URL"; then
            # Optional integrity check — works if maintainer uploaded the .sha256 file.
            SHA_URL="${ASSET_URL}.sha256"
            if curl -fsSL -o "$TMPDIR/$ASSET_NAME.sha256" "$SHA_URL" 2>/dev/null; then
                if (cd "$TMPDIR" && sha256sum -c "$ASSET_NAME.sha256" >/dev/null 2>&1); then
                    info "SHA256 verified."
                else
                    warn "SHA256 mismatch — refusing to install this binary; will build from source."
                    rm -rf "$TMPDIR"
                    BUILT_BINARY=""
                fi
            fi

            if [[ -f "$TMPDIR/$ASSET_NAME" ]]; then
                tar -xzf "$TMPDIR/$ASSET_NAME" -C "$TMPDIR"
                CANDIDATE="$TMPDIR/$BINARY_NAME"

                # Sanity-check: the binary must actually run on this system.
                # If glibc is too old, `--version` will fail — we detect that
                # and fall back to source build transparently.
                if [[ -x "$CANDIDATE" ]] && "$CANDIDATE" --version &>/dev/null; then
                    BUILT_BINARY="$CANDIDATE"
                    info "Prebuilt binary works on this system — skipping source build."
                else
                    warn "Prebuilt binary won't run here (likely glibc mismatch). Building from source instead."
                    BUILT_BINARY=""
                fi
            fi
        else
            warn "Download failed — will build from source."
        fi
    else
        warn "No release found (or no asset for $RUST_TARGET) — will build from source."
    fi
fi

# Fall back to a local source build if the download path didn't succeed.
if [[ -z "$BUILT_BINARY" ]]; then
    step "Building CopyNinja from source (release mode)…"

    if ! command -v cargo &>/dev/null; then
        error "Rust toolchain not found. Install via: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    fi

    cd "$SCRIPT_DIR"
    cargo build --release 2>&1 | tail -3
    BUILT_BINARY="$SCRIPT_DIR/target/release/$BINARY_NAME"

    if [[ ! -f "$BUILT_BINARY" ]]; then
        error "Build failed — binary not found at $BUILT_BINARY"
    fi
fi

info "Binary ready: $(du -h "$BUILT_BINARY" | cut -f1)"

# ── 3. Install binary ─────────────────────────────────────────────────────
step "Installing binary to $INSTALL_DIR…"
mkdir -p "$INSTALL_DIR"
cp "$BUILT_BINARY" "$INSTALL_DIR/$BINARY_NAME"
chmod +x "$INSTALL_DIR/$BINARY_NAME"

# Clean up legacy Python scripts if present
if [[ -f "$INSTALL_DIR/clipdaemon.py" ]]; then
    rm -f "$INSTALL_DIR/clipdaemon.py"
    rm -f "$INSTALL_DIR/clippick.py"
    info "Removed legacy Python scripts."
fi

# ── 4. Install systemd user service ───────────────────────────────────────
step "Installing systemd user service…"
SYSTEMD_DIR="$HOME/.config/systemd/user"
mkdir -p "$SYSTEMD_DIR"

cat > "$SYSTEMD_DIR/copyninja.service" << EOF
[Unit]
Description=CopyNinja — Clipboard History Daemon
Documentation=https://github.com/Yogesh190602/CopyNinja
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Type=simple
ExecStart=%h/.local/bin/copyninja daemon
Restart=on-failure
RestartSec=3s
Environment=RUST_LOG=info

[Install]
WantedBy=graphical-session.target
EOF

systemctl --user daemon-reload
systemctl --user enable copyninja.service
systemctl --user restart copyninja.service
info "Daemon started and enabled on login."

if [[ "$SESSION_TYPE" != "wayland" && "$SESSION_TYPE" != "x11" ]]; then
    info "Note: Daemon will auto-detect the display and start monitoring once a graphical session is available."
fi

# ── 4b. GNOME Wayland: create default config ──────────────────────────────
# On GNOME Wayland, Mutter does not expose the focused window class to
# external tools, so auto-detection of "am I pasting into a terminal?"
# silently fails and pastes end up as Ctrl+V, which terminals ignore.
# Create a default config forcing terminal paste mode — only if the user
# doesn't already have a config file (never overwrite).
CONFIG_DIR="$HOME/.config/copyninja"
CONFIG_FILE="$CONFIG_DIR/config.toml"

if [[ "$SESSION_TYPE" == "wayland" && "$DESKTOP" == *GNOME* ]]; then
    if [[ -f "$CONFIG_FILE" ]]; then
        info "Config already exists at $CONFIG_FILE — leaving it alone."
    else
        step "Creating default GNOME Wayland config…"
        mkdir -p "$CONFIG_DIR"
        cat > "$CONFIG_FILE" <<'EOF'
# CopyNinja config — created by install.sh for GNOME Wayland.
#
# On GNOME Wayland, Mutter does not expose the focused window class to
# external apps (org.gnome.Shell.Introspect is locked down, xdotool
# returns "(null)" for native Wayland windows). This means paste_mode
# cannot auto-detect whether the focused window is a terminal, so it
# would silently default to Ctrl+V — which terminals ignore.
#
# "terminal" forces Ctrl+Shift+V, which works in all terminals and in
# most browsers (as paste-without-formatting). It does NOT work in GTK
# text fields, VS Code, or LibreOffice — change to "normal" if you
# mainly paste into those apps instead. Images always use Ctrl+V
# regardless of this setting.
paste_mode = "terminal"
EOF
        info "Wrote $CONFIG_FILE (paste_mode = \"terminal\")"
        warn "If you mainly paste into text fields (not terminals),"
        warn "  edit $CONFIG_FILE and change paste_mode to \"normal\"."
    fi
fi

# ── 5. Hotkey setup (DE-specific) ─────────────────────────────────────────
PICK_CMD="$INSTALL_DIR/$BINARY_NAME pick"
COPYNINJA_MARKER="# CopyNinja keybinding"

setup_gnome_keybinding() {
    if ! command -v gsettings &>/dev/null; then
        warn "gsettings not found — skipping GNOME keybinding setup."
        return
    fi
    step "Setting Super+Shift+V keybinding (GNOME)…"
    CUSTOM_PATH="/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings"

    EXISTING=$(gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings 2>/dev/null || echo "@as []")
    FOUND_SLOT=""
    for path_entry in $(echo "$EXISTING" | tr -d "[]',"); do
        slot_name=$(gsettings get "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${path_entry}" name 2>/dev/null)
        if [[ "$slot_name" == "'Clipboard History'" ]]; then
            FOUND_SLOT="$path_entry"
            break
        fi
    done

    if [[ -n "$FOUND_SLOT" ]]; then
        NEW_PATH="$FOUND_SLOT"
    else
        SLOT=0
        while echo "$EXISTING" | grep -q "custom${SLOT}/" 2>/dev/null; do
            SLOT=$((SLOT + 1))
        done
        NEW_PATH="${CUSTOM_PATH}/custom${SLOT}/"

        if [[ "$EXISTING" == "@as []" ]]; then
            NEW_LIST="['${NEW_PATH}']"
        else
            NEW_LIST=$(echo "$EXISTING" | sed "s|]|, '${NEW_PATH}']|")
        fi
        gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings "$NEW_LIST"
    fi

    gsettings set "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${NEW_PATH}" name 'Clipboard History'
    gsettings set "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${NEW_PATH}" command "$PICK_CMD"
    gsettings set "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${NEW_PATH}" binding '<Shift><Super>v'
    info "Keybinding set: Super+Shift+V → copyninja pick"
}

setup_wm_keybinding() {
    local config_file="$1"
    local bind_line="$2"

    if [[ ! -f "$config_file" ]]; then
        warn "Config file not found: $config_file — skipping keybinding setup."
        echo "  Add this line manually: $bind_line"
        return
    fi

    if grep -qF "$COPYNINJA_MARKER" "$config_file" 2>/dev/null; then
        # Update existing keybinding to point to the Rust binary
        sed -i "/$COPYNINJA_MARKER/{N;d;}" "$config_file"
        info "Updating existing keybinding in $config_file"
    fi

    echo "" >> "$config_file"
    echo "$COPYNINJA_MARKER" >> "$config_file"
    echo "$bind_line" >> "$config_file"
    info "Keybinding added to $config_file"
}

COPYNINJA_RULES_MARKER="# CopyNinja window rules"

setup_wm_window_rules() {
    local config_file="$1"
    shift
    local rules=("$@")

    if [[ ! -f "$config_file" ]]; then
        return
    fi

    # Avoid duplicates
    if grep -qF "$COPYNINJA_RULES_MARKER" "$config_file" 2>/dev/null; then
        info "Window rules already present in $config_file"
        return
    fi

    echo "" >> "$config_file"
    echo "$COPYNINJA_RULES_MARKER" >> "$config_file"
    for rule in "${rules[@]}"; do
        echo "$rule" >> "$config_file"
    done
    info "Window rules added to $config_file"
}

case "$DESKTOP" in
    *GNOME*)
        setup_gnome_keybinding
        ;;
    *Hyprland*|*hyprland*)
        setup_wm_keybinding \
            "$HOME/.config/hypr/hyprland.conf" \
            "bind = SUPER SHIFT, V, exec, $PICK_CMD"
        ;;
    *sway*|*Sway*)
        setup_wm_keybinding \
            "$HOME/.config/sway/config" \
            "bindsym Mod4+Shift+v exec $PICK_CMD"
        ;;
    *i3*|*I3*)
        setup_wm_keybinding \
            "$HOME/.config/i3/config" \
            "bindsym Mod4+Shift+v exec $PICK_CMD"
        ;;
    *)
        warn "Automatic keybinding setup not supported for '$DESKTOP'."
        echo "  Please bind Super+Shift+V to this command manually:"
        echo "  $PICK_CMD"
        ;;
esac

# ── 6. Window rules (DE-specific) ────────────────────────────────────────
case "$DESKTOP" in
    *Hyprland*|*hyprland*)
        setup_wm_window_rules \
            "$HOME/.config/hypr/hyprland.conf" \
            'windowrulev2 = float, class:^(com.copyninja.picker)$' \
            'windowrulev2 = dimaround, class:^(com.copyninja.picker)$' \
            'windowrulev2 = stayfocused, class:^(com.copyninja.picker)$' \
            'windowrulev2 = noborder, class:^(com.copyninja.picker)$'
        ;;
    *sway*|*Sway*)
        setup_wm_window_rules \
            "$HOME/.config/sway/config" \
            'for_window [app_id="com.copyninja.picker"] floating enable, border none'
        ;;
    *i3*|*I3*)
        setup_wm_window_rules \
            "$HOME/.config/i3/config" \
            'for_window [class="com.copyninja.picker"] floating enable, border none'
        ;;
esac

# ── Done ──────────────────────────────────────────────────────────────────
echo ""
info "Installation complete!"
echo ""
echo "  Binary:         $INSTALL_DIR/$BINARY_NAME ($(du -h "$INSTALL_DIR/$BINARY_NAME" | cut -f1))"
echo "  Daemon status:  systemctl --user status copyninja"
echo "  Live logs:      journalctl --user -u copyninja -f"
echo "  History file:   ~/.clipboard_history.json"
echo ""
echo "  Usage:"
echo "    - Copy text normally, it will be saved automatically"
echo "    - Press Super+Shift+V to open picker"
echo "    - Click on any entry to copy and auto-paste"
echo ""

if [[ "$GROUP_ADDED" == "1" ]]; then
    warn "IMPORTANT — you were just added to the 'input' group."
    warn "Group membership does not apply to the current session."
    warn "Auto-paste via ydotool WILL NOT WORK until you either:"
    echo "    • reboot, OR"
    echo "    • log out and log back in, OR"
    echo "    • run 'newgrp input' in each shell you want to test from"
    echo ""
    info "Everything else is ready. Enjoy CopyNinja after your next login!"
else
    info "Everything is ready — no logout needed. Enjoy CopyNinja!"
fi
