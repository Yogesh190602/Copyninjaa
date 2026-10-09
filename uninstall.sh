#!/usr/bin/env bash
# uninstall.sh — Remove CopyNinja (Rust edition) from the system
set -euo pipefail

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
info()  { echo -e "${GREEN}[✓]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
error() { echo -e "${RED}[✗]${NC} $*"; exit 1; }

DESKTOP="${XDG_CURRENT_DESKTOP:-unknown}"
COPYNINJA_MARKER="# CopyNinja keybinding"
COPYNINJA_RULES_MARKER="# CopyNinja window rules"

# If XDG var isn't set, detect DE from running processes
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

echo ""
echo "  This will completely remove CopyNinja from your system."
echo ""
read -rp "  Continue? [y/N] " answer
[[ "$answer" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 0; }

# ── 1. Stop and disable the systemd service ───────────────────────────────
if systemctl --user is-active copyninja.service &>/dev/null; then
    systemctl --user stop copyninja.service
    info "Stopped copyninja service."
fi

if systemctl --user is-enabled copyninja.service &>/dev/null; then
    systemctl --user disable copyninja.service
    info "Disabled copyninja service."
fi

rm -f "$HOME/.config/systemd/user/copyninja.service"
systemctl --user daemon-reload
info "Removed systemd unit file."

# ── 2. Remove binary (and legacy Python scripts if present) ───────────────
rm -f "$HOME/.local/bin/copyninja"
rm -f "$HOME/.local/bin/clipdaemon.py"
rm -f "$HOME/.local/bin/clippick.py"
info "Removed binary and any legacy scripts."

# ── 3. Remove keybinding (DE-specific) ────────────────────────────────────
remove_gnome_keybinding() {
    if ! command -v gsettings &>/dev/null; then return; fi
    EXISTING=$(gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings 2>/dev/null || echo "@as []")

    for path_entry in $(echo "$EXISTING" | tr -d "[]',"); do
        slot_name=$(gsettings get "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${path_entry}" name 2>/dev/null || true)
        if [[ "$slot_name" == "'Clipboard History'" ]]; then
            gsettings reset "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${path_entry}" name
            gsettings reset "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${path_entry}" command
            gsettings reset "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:${path_entry}" binding

            NEW_LIST=$(echo "$EXISTING" | sed "s|, '${path_entry}'||; s|'${path_entry}', ||; s|'${path_entry}'||")
            gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings "$NEW_LIST"
            info "Removed GNOME keybinding (Super+Shift+V)."
            break
        fi
    done
}

remove_wm_keybinding() {
    local config_file="$1"
    if [[ -f "$config_file" ]] && grep -qF "$COPYNINJA_MARKER" "$config_file" 2>/dev/null; then
        sed -i "/$COPYNINJA_MARKER/{N;d;}" "$config_file"
        sed -i -e :a -e '/^\n*$/{$d;N;ba' -e '}' "$config_file"
        info "Removed keybinding from $config_file"
    fi
}

remove_wm_window_rules() {
    local config_file="$1"
    if [[ -f "$config_file" ]] && grep -qF "$COPYNINJA_RULES_MARKER" "$config_file" 2>/dev/null; then
        # Remove the marker line and only the CopyNinja rule lines right after
        # it — never user lines that happen to follow without a blank line.
        # `cat >` keeps symlinked dotfiles intact.
        local tmp
        tmp="$(mktemp)"
        awk -v marker="$COPYNINJA_RULES_MARKER" '
            index($0, marker)                            { inrules = 1; next }
            inrules && index($0, "com.copyninja.picker") { next }
                                                         { inrules = 0; print }
        ' "$config_file" > "$tmp" && cat "$tmp" > "$config_file"
        rm -f "$tmp"
        sed -i -e :a -e '/^\n*$/{$d;N;ba' -e '}' "$config_file"
        info "Removed window rules from $config_file"
    fi
}

case "$DESKTOP" in
    *GNOME*)
        remove_gnome_keybinding
        ;;
    *Hyprland*|*hyprland*)
        remove_wm_keybinding "$HOME/.config/hypr/hyprland.conf"
        ;;
    *sway*|*Sway*)
        remove_wm_keybinding "$HOME/.config/sway/config"
        ;;
    *i3*|*I3*)
        remove_wm_keybinding "$HOME/.config/i3/config"
        ;;
    *)
        warn "Could not auto-remove keybinding for '$DESKTOP'. Please remove the Super+Shift+V binding manually."
        ;;
esac

# Remove window rules
case "$DESKTOP" in
    *Hyprland*|*hyprland*)
        remove_wm_window_rules "$HOME/.config/hypr/hyprland.conf"
        ;;
    *sway*|*Sway*)
        remove_wm_window_rules "$HOME/.config/sway/config"
        ;;
    *i3*|*I3*)
        remove_wm_window_rules "$HOME/.config/i3/config"
        ;;
esac

# ── 4. Remove legacy GNOME extension (from older installs) ────────────────
EXT_UUID="copyninja-clip@copyninja"
EXT_DIR="$HOME/.local/share/gnome-shell/extensions/$EXT_UUID"
if [[ -d "$EXT_DIR" ]]; then
    gnome-extensions disable "$EXT_UUID" 2>/dev/null || true
    rm -rf "$EXT_DIR"
    info "Removed legacy GNOME Shell extension."
fi

rm -f "$HOME/.config/autostart/copyninja-enable.desktop"

# ── 4b. Remove the /dev/uinput udev rule (installed for ydotool auto-paste) ─
UDEV_RULE="/etc/udev/rules.d/60-copyninja-uinput.rules"
MODULES_CONF="/etc/modules-load.d/copyninja-uinput.conf"
if [[ -f "$UDEV_RULE" ]]; then
    echo ""
    echo "  CopyNinja installed $UDEV_RULE, which gives your desktop"
    echo "  session write access to /dev/uinput (used by ydotool for auto-paste)."
    echo "  Other tools that use ydotool for typing/pasting may rely on it."
    read -rp "  Remove it? [y/N] " del_udev || del_udev=""
    if [[ "$del_udev" =~ ^[Yy]$ ]]; then
        sudo rm -f "$UDEV_RULE" "$MODULES_CONF"
        sudo udevadm control --reload-rules || true
        sudo udevadm trigger --name-match=uinput 2>/dev/null || true
        info "Removed $UDEV_RULE."
    fi
fi

# ── 5. Remove clipboard history ───────────────────────────────────────────
read -rp "  Delete clipboard history and cached images (~/.clipboard_history.json + backups + ~/.local/share/copyninja/images)? [y/N] " del_data
if [[ "$del_data" =~ ^[Yy]$ ]]; then
    rm -f "$HOME/.clipboard_history.json" "$HOME"/.clipboard_history.json.bak.* \
          "$HOME"/.clipboard_history.json.tmp* "$HOME/.clipboard_history.lock"
    rm -rf "$HOME/.local/share/copyninja/images"
    info "Removed history file, backups, and cached images."
fi

# ── 6. Remove config ──────────────────────────────────────────────────────
if [[ -d "$HOME/.config/copyninja" ]]; then
    read -rp "  Delete config directory (~/.config/copyninja)? [y/N] " del_conf
    if [[ "$del_conf" =~ ^[Yy]$ ]]; then
        rm -rf "$HOME/.config/copyninja"
        info "Removed config directory."
    fi
fi

echo ""
info "CopyNinja has been uninstalled."
echo ""
