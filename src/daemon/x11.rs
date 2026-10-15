use super::{blocking, is_secret, session};
use crate::content::IMAGE_MIMES;
use anyhow::{bail, Context, Result};
use log::{debug, info, warn};
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc::error::TrySendError;
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::reexports::x11rb_protocol::parse_display::parse_display;
use x11rb::rust_connection::{DefaultStream, RustConnection};

/// Start monitoring the X11 clipboard. The X server reports every change of
/// clipboard owner (XFixes); without that, `xclip` is polled every 500ms.
/// This function runs indefinitely.
pub async fn start() -> Result<()> {
    info!("Starting X11 clipboard watcher");

    // Test that xclip can connect to the display
    let test = session::command("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
        .await?;

    let stderr = String::from_utf8_lossy(&test.stderr);
    if stderr.contains("Can't open display") {
        bail!("xclip cannot open display: {}", stderr.trim());
    }

    match tokio::task::spawn_blocking(subscribe).await? {
        Ok(conn) => watch(conn).await,
        Err(e) => {
            warn!(
                "X11 clipboard change events unavailable ({:#}), polling every 500ms instead",
                e
            );
            poll().await
        }
    }
}

/// Store a clip each time the X server reports a new clipboard owner.
async fn watch(conn: RustConnection) -> Result<()> {
    info!("Watching X11 clipboard changes (XFixes events)");

    // Room for one: changes made while a clip is being read collapse into
    // one more read, which gets the newest content.
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    std::thread::spawn(move || loop {
        match conn.wait_for_event() {
            Ok(Event::XfixesSelectionNotify(_)) => {
                if let Err(TrySendError::Closed(())) = tx.try_send(()) {
                    break;
                }
            }
            Ok(_) => {}
            Err(e) => {
                warn!("Lost the X11 connection: {}", e);
                break;
            }
        }
    });

    while rx.recv().await.is_some() {
        debug!("X11 clipboard owner changed");
        fetch_and_store().await;
    }
    bail!("X11 clipboard events stopped")
}

/// Connect to the X server and ask it to report clipboard owner changes.
fn subscribe() -> Result<RustConnection> {
    let display = session::var("DISPLAY").context("DISPLAY is not set")?;
    let parsed = parse_display(Some(&display))?;
    // x11rb reads the cookie from our own $XAUTHORITY, which the daemon
    // usually doesn't have (see session::command).
    let (auth_name, auth_data) = session::var("XAUTHORITY")
        .map(Into::into)
        .or_else(|| dirs::home_dir().map(|home| home.join(".Xauthority")))
        .and_then(|path| read_cookie(&path, parsed.display))
        .unwrap_or_default();
    let (stream, _) = parsed
        .connect_instruction()
        .find_map(|address| DefaultStream::connect(&address).ok())
        .with_context(|| format!("cannot reach X display {}", display))?;
    let screen = usize::from(parsed.screen);
    let conn =
        RustConnection::connect_to_stream_with_auth_info(stream, screen, auth_name, auth_data)?;

    // Required before any other XFixes request.
    conn.xfixes_query_version(5, 0)?.reply()?;
    let root = conn.setup().roots[screen].root;
    let clipboard = conn.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
    conn.xfixes_select_selection_input(root, clipboard, SelectionEventMask::SET_SELECTION_OWNER)?
        .check()?;
    Ok(conn)
}

/// The MIT-MAGIC-COOKIE-1 for local display number `display` in an
/// Xauthority file, as (name, data).
fn read_cookie(path: &Path, display: u16) -> Option<(Vec<u8>, Vec<u8>)> {
    let file = std::fs::read(path).ok()?;
    let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    parse_cookie(&file, display, hostname.trim().as_bytes())
}

fn parse_cookie(mut file: &[u8], display: u16, hostname: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    const FAMILY_LOCAL: u16 = 256;
    const FAMILY_WILD: u16 = 65535;
    const COOKIE: &[u8] = b"MIT-MAGIC-COOKIE-1";

    fn number(buf: &mut &[u8]) -> Option<u16> {
        let (bytes, rest) = buf.split_first_chunk::<2>()?;
        *buf = rest;
        Some(u16::from_be_bytes(*bytes))
    }
    fn field<'a>(buf: &mut &'a [u8]) -> Option<&'a [u8]> {
        let len = usize::from(number(buf)?);
        if buf.len() < len {
            return None;
        }
        let (field, rest) = buf.split_at(len);
        *buf = rest;
        Some(field)
    }

    let display = display.to_string();
    while !file.is_empty() {
        let family = number(&mut file)?;
        let address = field(&mut file)?;
        let entry_display = field(&mut file)?;
        let name = field(&mut file)?;
        let data = field(&mut file)?;
        let local = family == FAMILY_WILD || (family == FAMILY_LOCAL && address == hostname);
        if local
            && (entry_display.is_empty() || entry_display == display.as_bytes())
            && name == COOKIE
        {
            return Some((name.to_vec(), data.to_vec()));
        }
    }
    None
}

/// Poll `xclip` every 500ms and store the clip whenever it changes.
async fn poll() -> Result<()> {
    info!("Polling the X11 clipboard every 500ms");

    let mut last_hash = get_clipboard_hash().await;
    let mut interval = tokio::time::interval(Duration::from_millis(500));

    loop {
        interval.tick().await;
        let current_hash = get_clipboard_hash().await;
        if current_hash != last_hash {
            debug!(
                "X11 clipboard changed (hash: {} -> {})",
                last_hash, current_hash
            );
            last_hash = current_hash;
            fetch_and_store().await;
        }
    }
}

/// Detect available types and fetch the best one.
async fn fetch_and_store() {
    // Check available TARGETS
    let targets = list_targets().await.unwrap_or_default();

    if is_secret(&targets) {
        debug!("Skipping clipboard content marked as a password");
        return;
    }

    // Try image types first
    for mime in IMAGE_MIMES {
        if targets.iter().any(|t| t == mime) {
            if let Some(data) = get_clipboard_bytes(mime).await {
                if !data.is_empty() {
                    debug!(
                        "Captured X11 image clipboard ({}, {} bytes)",
                        mime,
                        data.len()
                    );
                    blocking(move || crate::storage::process_image(&data, mime)).await;
                    return;
                }
            }
        }
    }

    // File-manager image copy: Nautilus/Files puts text/uri-list (a file:// URI)
    // in the clipboard, not image bytes. If the URI points to a local image,
    // read the file and store its bytes as a proper image entry.
    if targets.iter().any(|t| t == "text/uri-list") {
        if let Some(uri_list) = get_clipboard_bytes("text/uri-list").await {
            let max = crate::storage::global().max_image_bytes;
            let found = tokio::task::spawn_blocking(move || {
                super::read_image_from_uri_list(&uri_list, max)
            })
            .await
            .ok()
            .flatten();
            if let Some((data, mime)) = found {
                debug!(
                    "Captured X11 image from URI list ({}, {} bytes)",
                    mime,
                    data.len()
                );
                blocking(move || crate::storage::process_image(&data, &mime)).await;
                return;
            }
        }
    }

    // Fall back to text
    if let Some(text) = get_clipboard_text().await {
        if !text.trim().is_empty() {
            blocking(move || crate::storage::process_text(&text)).await;
        }
    }
}

async fn list_targets() -> Option<Vec<String>> {
    let output = session::command("xclip")
        .args(["-selection", "clipboard", "-t", "TARGETS", "-o"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let targets = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Some(targets)
}

async fn get_clipboard_text() -> Option<String> {
    let output = session::command("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
        .await
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        // Normal when the clipboard holds no text (e.g. only an image).
        debug!("xclip returned non-zero status");
        None
    }
}

async fn get_clipboard_bytes(mime: &str) -> Option<Vec<u8>> {
    let output = session::command("xclip")
        .args(["-selection", "clipboard", "-t", mime, "-o"])
        .output()
        .await
        .ok()?;

    if output.status.success() {
        Some(output.stdout)
    } else {
        None
    }
}

/// The selection owner's TIMESTAMP (when it took ownership). It changes with
/// every new copy and is a few bytes, unlike the image itself.
async fn selection_timestamp(targets: &[String]) -> Option<Vec<u8>> {
    if !targets.iter().any(|t| t == "TIMESTAMP") {
        return None;
    }
    let ts = get_clipboard_bytes("TIMESTAMP").await?;
    // Some owners answer CurrentTime (0), which never changes.
    if ts.iter().all(|&b| b == 0) {
        return None;
    }
    Some(ts)
}

async fn get_clipboard_hash() -> String {
    // Hash TARGETS + preferred payload so image↔image and text↔image
    // transitions are detected, not just text changes.
    let targets = list_targets().await.unwrap_or_default();
    if targets.is_empty() {
        return String::new();
    }

    let payload = if let Some(mime) = IMAGE_MIMES.iter().find(|m| targets.iter().any(|t| t == *m)) {
        // Avoid pulling a multi-megabyte image through xclip twice a second
        // when the owner can tell us cheaply that nothing changed.
        match selection_timestamp(&targets).await {
            Some(ts) => ts,
            None => get_clipboard_bytes(mime).await.unwrap_or_default(),
        }
    } else {
        get_clipboard_text()
            .await
            .map(|s| s.into_bytes())
            .unwrap_or_default()
    };

    let mut to_hash: Vec<u8> = targets.join(",").into_bytes();
    to_hash.extend_from_slice(&payload);
    crate::storage::get_hash_bytes(&to_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(family: u16, address: &[u8], display: &[u8], name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = family.to_be_bytes().to_vec();
        for field in [address, display, name, data] {
            out.extend_from_slice(&(field.len() as u16).to_be_bytes());
            out.extend_from_slice(field);
        }
        out
    }

    #[test]
    fn cookie_for_the_local_display() {
        let cookie = b"MIT-MAGIC-COOKIE-1";
        let file = [
            // Another machine's display 0 (ssh forwarding)
            entry(0, &[10, 0, 0, 2], b"0", cookie, b"remote"),
            // This machine, another display
            entry(256, b"box", b"1", cookie, b"display1"),
            entry(256, b"box", b"0", b"XDM-AUTHORIZATION-1", b"other"),
            entry(256, b"box", b"0", cookie, b"local0"),
        ]
        .concat();
        let found = |display, host: &[u8]| parse_cookie(&file, display, host).map(|(_, d)| d);
        assert_eq!(found(0, b"box").as_deref(), Some(&b"local0"[..]));
        assert_eq!(found(1, b"box").as_deref(), Some(&b"display1"[..]));
        assert_eq!(found(0, b"elsewhere"), None, "hostname must match");
        assert_eq!(found(2, b"box"), None);
    }

    #[test]
    fn mutter_style_cookie_matches_any_display() {
        // Mutter writes a wildcard entry with no display number.
        let file = entry(65535, b"box", b"", b"MIT-MAGIC-COOKIE-1", b"xwayland");
        let (name, data) = parse_cookie(&file, 0, b"other-name").unwrap();
        assert_eq!(name, b"MIT-MAGIC-COOKIE-1");
        assert_eq!(data, b"xwayland");
    }

    #[test]
    fn truncated_cookie_file_is_rejected() {
        let file = entry(256, b"box", b"0", b"MIT-MAGIC-COOKIE-1", b"local0");
        for len in 0..file.len() {
            assert_eq!(parse_cookie(&file[..len], 0, b"box"), None, "{} bytes", len);
        }
    }
}
