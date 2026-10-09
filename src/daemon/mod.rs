pub mod dbus;
pub mod session;
pub mod wayland;
pub mod x11;

use log::{debug, error, info, warn};
use session::SessionType;
use std::path::Path;
use std::time::Duration;

/// MIME type that password managers (KeePassXC, KDE Wallet, …) add to secrets they copy.
const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";

/// True if the clipboard owner marked the content as a secret that clipboard
/// managers shouldn't keep.
pub fn is_secret(types: &[String]) -> bool {
    types.iter().any(|t| t == PASSWORD_HINT)
}

/// Run storage work (file I/O, hashing) off the async runtime's worker threads.
pub(crate) async fn blocking(f: impl FnOnce() + Send + 'static) {
    if let Err(e) = tokio::task::spawn_blocking(f).await {
        error!("Storage task failed: {}", e);
    }
}

/// Parse a `text/uri-list` payload and, if any URI points to a local image
/// file no larger than `max_bytes`, read its bytes and return them together
/// with the detected MIME type. Returns None if no usable image is found.
///
/// Handles the Nautilus/GNOME Files "Ctrl+C on an image file" case, where the
/// clipboard contains a `file://` URI instead of raw image bytes.
pub fn read_image_from_uri_list(raw: &[u8], max_bytes: usize) -> Option<(Vec<u8>, String)> {
    let text = std::str::from_utf8(raw).ok()?;
    for line in text.lines() {
        let line = line.trim();
        // text/uri-list may have comments starting with '#'
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = match line.strip_prefix("file://") {
            Some(p) => percent_decode(p.strip_prefix("localhost").unwrap_or(p)),
            None => continue, // skip non-local URIs (http://, smb://, etc.)
        };
        // Anything else would be file://<remote-host>/...
        if !path.starts_with('/') {
            continue;
        }
        let p = Path::new(&path);
        let mime = match p
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
        {
            Some(ref e) if e == "png" => "image/png",
            Some(ref e) if e == "jpg" || e == "jpeg" => "image/jpeg",
            Some(ref e) if e == "webp" => "image/webp",
            Some(ref e) if e == "gif" => "image/gif",
            Some(ref e) if e == "bmp" => "image/bmp",
            _ => continue,
        };
        // Check the size before reading: the file could be huge.
        match std::fs::metadata(p) {
            Ok(meta) if !meta.is_file() => continue,
            Ok(meta) if meta.len() > max_bytes as u64 => {
                debug!("URI-list target {} is larger than max_image_size_mb", path);
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                debug!("Failed to stat URI-list target {}: {}", path, e);
                continue;
            }
        }
        match std::fs::read(p) {
            Ok(data) if !data.is_empty() => return Some((data, mime.to_string())),
            Ok(_) => debug!("URI-list target {} is empty", path),
            Err(e) => debug!("Failed to read URI-list target {}: {}", path, e),
        }
    }
    None
}

/// Minimal RFC 3986 percent-decode for file URIs (handles %20 etc.).
pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn run() {
    // Start sync watcher if enabled
    crate::sync::start_watcher(crate::storage::global());

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    rt.block_on(async {
        if let Err(e) = run_async().await {
            error!("Daemon fatal error: {}", e);
            std::process::exit(1);
        }
    });
}

async fn run_async() -> anyhow::Result<()> {
    info!("CopyNinja daemon starting");

    let _conn = dbus::setup().await?;
    info!("D-Bus service registered: com.copyninja.Daemon");

    // Retry loop: try to start clipboard watcher for up to 5 minutes
    const MAX_RETRIES: u32 = 60;
    for attempt in 0..MAX_RETRIES {
        let session = tokio::task::spawn_blocking(session::detect).await?;
        info!(
            "Session type: {:?} (attempt {}/{})",
            session,
            attempt + 1,
            MAX_RETRIES
        );

        match session {
            SessionType::Wayland => {
                // Try native Wayland watcher first
                match wayland::start().await {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        warn!("Wayland watcher failed: {}", e);
                        // Fall back to X11 (XWayland) like GNOME Wayland
                        info!("Trying X11/XWayland fallback...");
                        match x11::start().await {
                            Ok(()) => return Ok(()),
                            Err(e) => warn!("X11 fallback failed: {}", e),
                        }
                    }
                }
            }
            SessionType::X11 => match x11::start().await {
                Ok(()) => return Ok(()),
                Err(e) => warn!("X11 watcher failed: {}", e),
            },
            SessionType::Unknown => {
                warn!("No graphical session detected yet");
            }
        }

        if attempt < MAX_RETRIES - 1 {
            info!("Retrying in 5 seconds...");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    anyhow::bail!(
        "Failed to start clipboard watcher after {} attempts",
        MAX_RETRIES
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_detected() {
        let types = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_secret(&types(&[
            "text/plain",
            "x-kde-passwordManagerHint"
        ])));
        assert!(!is_secret(&types(&["text/plain", "UTF8_STRING"])));
    }

    #[test]
    fn uri_list_reads_local_images_within_limit() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("my pic.PNG");
        std::fs::write(&img, b"pngdata").unwrap();
        let uri = format!("# comment\nfile://{}\n", img.display()).replace(' ', "%20");

        let (data, mime) = read_image_from_uri_list(uri.as_bytes(), 1024).unwrap();
        assert_eq!(data, b"pngdata");
        assert_eq!(mime, "image/png");

        let localhost = format!("file://localhost{}", img.display()).replace(' ', "%20");
        assert!(read_image_from_uri_list(localhost.as_bytes(), 1024).is_some());

        assert!(
            read_image_from_uri_list(uri.as_bytes(), 3).is_none(),
            "over the size limit"
        );
        assert!(read_image_from_uri_list(b"file://otherhost/pic.png", 1024).is_none());
        assert!(read_image_from_uri_list(b"https://example.com/pic.png", 1024).is_none());
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("/a%20b/%C3%A9.png"), "/a b/é.png");
        assert_eq!(percent_decode("/100%"), "/100%");
    }
}
