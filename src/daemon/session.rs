use log::{debug, warn};
use std::env;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub enum SessionType {
    Wayland,
    X11,
    Unknown,
}

/// Variables a child process needs to reach the user's display.
const DISPLAY_VARS: [&str; 5] = [
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XAUTHORITY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
];

type Vars = Vec<(String, String)>;

/// Display variables discovered at runtime. The daemon usually starts before
/// the graphical session has exported them, so they're looked up later and
/// handed to every child process through `command()` — changing our own
/// environment with `env::set_var` isn't safe once other threads are running.
static IMPORTED: Mutex<Vars> = Mutex::new(Vec::new());

/// A command with the discovered display variables applied.
pub fn command(program: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    for (key, value) in IMPORTED.lock().unwrap().iter() {
        cmd.env(key, value);
    }
    cmd
}

/// Look up a variable, preferring the discovered ones.
fn var(key: &str) -> Option<String> {
    let imported = IMPORTED
        .lock()
        .unwrap()
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone());
    imported
        .or_else(|| env::var(key).ok())
        .filter(|v| !v.is_empty())
}

pub fn detect() -> SessionType {
    // Without a display in our own environment, look for it elsewhere (again
    // on every retry, since the session may still be starting).
    if env::var_os("WAYLAND_DISPLAY").is_none() && env::var_os("DISPLAY").is_none() {
        let found = from_systemd_user_env()
            .or_else(from_own_processes)
            .or_else(from_sockets)
            .unwrap_or_default();
        if !found.is_empty() {
            debug!("Using display environment {:?}", found);
        }
        *IMPORTED.lock().unwrap() = found;
    }

    match var("XDG_SESSION_TYPE").as_deref() {
        Some("wayland") => {
            debug!("Detected Wayland via XDG_SESSION_TYPE");
            return SessionType::Wayland;
        }
        Some("x11") => {
            debug!("Detected X11 via XDG_SESSION_TYPE");
            return SessionType::X11;
        }
        _ => {}
    }
    if var("WAYLAND_DISPLAY").is_some() {
        debug!("Detected Wayland via WAYLAND_DISPLAY");
        return SessionType::Wayland;
    }
    if var("DISPLAY").is_some() {
        debug!("Detected X11 via DISPLAY");
        return SessionType::X11;
    }

    detect_from_processes().unwrap_or(SessionType::Unknown)
}

fn uid() -> Option<u32> {
    fs::metadata("/proc/self").ok().map(|m| m.uid())
}

/// Keep the display variables from a `KEY=VALUE` list, if they point at a
/// display that exists.
fn pick_display_vars<'a>(vars: impl Iterator<Item = &'a str>) -> Option<Vars> {
    let found: Vars = vars
        .filter_map(|v| v.split_once('='))
        .filter(|(k, v)| DISPLAY_VARS.contains(k) && !v.is_empty())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    display_alive(&found).then_some(found)
}

/// True if the Wayland socket or X11 display named in `vars` exists.
fn display_alive(vars: &Vars) -> bool {
    let get = |key: &str| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());

    if let Some(name) = get("WAYLAND_DISPLAY") {
        let runtime_dir = get("XDG_RUNTIME_DIR")
            .or_else(|| env::var("XDG_RUNTIME_DIR").ok())
            .unwrap_or_default();
        if Path::new(&runtime_dir).join(&name).exists() {
            return true;
        }
    }
    if let Some(display) = get("DISPLAY") {
        // ":0", ":0.0" and "unix:0" are local sockets; "host:0" can't be checked.
        let Some((host, rest)) = display.rsplit_once(':') else {
            return false;
        };
        if !host.is_empty() && host != "unix" {
            return true;
        }
        let number = rest.split('.').next().unwrap_or_default();
        return Path::new(&format!("/tmp/.X11-unix/X{}", number)).exists();
    }
    false
}

/// The systemd user manager's environment. Compositors export their display
/// there (`systemctl --user import-environment`,
/// `dbus-update-activation-environment --systemd`), often after we started.
fn from_systemd_user_env() -> Option<Vars> {
    let output = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let found = pick_display_vars(String::from_utf8_lossy(&output.stdout).lines());
    if found.is_some() {
        debug!("Found display in the systemd user environment");
    }
    found
}

/// Any of our own processes started inside the graphical session (a terminal,
/// a panel, …) has the display variables in its environment.
fn from_own_processes() -> Option<Vars> {
    let uid = uid()?;
    let me = std::process::id();
    let mut x11_only = None;
    for proc in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = proc
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == me || proc.metadata().map_or(true, |m| m.uid() != uid) {
            continue;
        }
        let Ok(environ) = fs::read(proc.path().join("environ")) else {
            continue;
        };
        let environ = String::from_utf8_lossy(&environ);
        let Some(found) = pick_display_vars(environ.split('\0')) else {
            continue;
        };
        if found.iter().any(|(k, _)| k == "WAYLAND_DISPLAY") {
            debug!("Found display in the environment of process {}", pid);
            return Some(found);
        }
        x11_only.get_or_insert(found);
    }
    x11_only
}

/// Last resort: guess the display from the sockets that exist.
fn from_sockets() -> Option<Vars> {
    let runtime_dir = env::var("XDG_RUNTIME_DIR")
        .ok()
        .or_else(|| uid().map(|uid| format!("/run/user/{}", uid)))?;
    let first_socket = |dir: &Path, prefix: &str| -> Option<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with(prefix) && !n.ends_with(".lock"))
            .collect();
        names.sort();
        names.into_iter().next()
    };

    let mut found = Vec::new();
    if let Some(name) = first_socket(&PathBuf::from(&runtime_dir), "wayland-") {
        found.push(("WAYLAND_DISPLAY".to_string(), name));
        found.push(("XDG_RUNTIME_DIR".to_string(), runtime_dir));
    }
    if let Some(name) = first_socket(Path::new("/tmp/.X11-unix"), "X") {
        found.push(("DISPLAY".to_string(), format!(":{}", &name[1..])));
    }
    if !found.is_empty() {
        debug!("Guessed display from sockets");
    }
    (!found.is_empty()).then_some(found)
}

fn detect_from_processes() -> Option<SessionType> {
    // gnome-shell runs on X11 too, but the Wayland watcher falls back to X11.
    let wayland_compositors = [
        "Hyprland",
        "sway",
        "gnome-shell",
        "kwin_wayland",
        "weston",
        "niri",
        "river",
        "labwc",
        "wayfire",
        "cosmic-comp",
    ];
    let x11_wms = ["Xorg", "i3", "openbox", "xfwm4", "kwin_x11"];

    for name in wayland_compositors {
        if process_running(name) {
            debug!("Detected Wayland via running process: {}", name);
            return Some(SessionType::Wayland);
        }
    }

    for name in x11_wms {
        if process_running(name) {
            debug!("Detected X11 via running process: {}", name);
            return Some(SessionType::X11);
        }
    }

    warn!("Could not detect session type from running processes");
    None
}

fn process_running(name: &str) -> bool {
    Command::new("pgrep")
        .args(["-x", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_vars_need_a_live_socket() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().display().to_string();
        let env = format!(
            "HOME=/h\nWAYLAND_DISPLAY=wayland-9\nXDG_RUNTIME_DIR={}",
            runtime
        );

        assert!(pick_display_vars(env.lines()).is_none(), "socket missing");
        fs::write(dir.path().join("wayland-9"), "").unwrap();
        let vars = pick_display_vars(env.lines()).unwrap();
        assert_eq!(vars.len(), 2, "only display variables are kept: {:?}", vars);
    }

    #[test]
    fn remote_x11_displays_are_trusted() {
        assert!(pick_display_vars(["DISPLAY=remote:10.0"].into_iter()).is_some());
        assert!(pick_display_vars(["DISPLAY=:987654"].into_iter()).is_none());
    }
}
