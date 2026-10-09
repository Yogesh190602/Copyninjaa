use super::{blocking, is_secret, session};
use crate::content::IMAGE_MIMES;
use anyhow::{bail, Result};
use log::{debug, error, info};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Start monitoring the Wayland clipboard via `wl-paste --watch`.
/// This function runs indefinitely until the watcher process exits.
pub async fn start() -> Result<()> {
    info!("Starting Wayland clipboard watcher (wl-paste --watch)");

    let mut child = session::command("wl-paste")
        .args(["--watch", "echo", ""])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    // Check if the process exits immediately (no Wayland display)
    tokio::time::sleep(Duration::from_millis(500)).await;
    if let Ok(Some(status)) = child.try_wait() {
        bail!(
            "wl-paste exited immediately with status: {} (no Wayland display?)",
            status
        );
    }

    info!("wl-paste --watch is running, monitoring clipboard changes");

    let stdout = child
        .stdout
        .take()
        .expect("stdout was piped but is missing");
    let mut lines = BufReader::new(stdout).lines();

    // Each line from wl-paste --watch means the clipboard changed
    while let Ok(Some(_line)) = lines.next_line().await {
        debug!("Clipboard change detected, fetching content");
        fetch_and_store().await;
    }

    // If we get here, the wl-paste process exited
    let status = child.wait().await?;
    error!("wl-paste --watch exited with status: {}", status);
    bail!("wl-paste --watch exited unexpectedly")
}

/// Detect available MIME types and fetch the best one.
async fn fetch_and_store() {
    // Check what MIME types are available
    let types = match list_mime_types().await {
        Some(t) => t,
        None => return,
    };

    if is_secret(&types) {
        debug!("Skipping clipboard content marked as a password");
        return;
    }

    // Prefer images over text (user likely wants to capture the image)
    for mime in IMAGE_MIMES {
        if types.iter().any(|t| t == mime) {
            match fetch_clipboard_bytes(mime).await {
                Ok(data) if !data.is_empty() => {
                    debug!("Captured image clipboard ({}, {} bytes)", mime, data.len());
                    blocking(move || crate::storage::process_image(&data, mime)).await;
                    return;
                }
                _ => continue,
            }
        }
    }

    // File-manager image copy: Nautilus/Files puts text/uri-list (a file:// URI)
    // in the clipboard, not image bytes. If the URI points to a local image,
    // read the file and store its bytes as a proper image entry.
    if types.iter().any(|t| t == "text/uri-list") {
        if let Ok(uri_list) = fetch_clipboard_bytes("text/uri-list").await {
            let max = crate::storage::global().max_image_bytes;
            let found = tokio::task::spawn_blocking(move || {
                super::read_image_from_uri_list(&uri_list, max)
            })
            .await
            .ok()
            .flatten();
            if let Some((data, mime)) = found {
                debug!(
                    "Captured image from URI list ({}, {} bytes)",
                    mime,
                    data.len()
                );
                blocking(move || crate::storage::process_image(&data, &mime)).await;
                return;
            }
        }
    }

    // Fall back to text
    if types
        .iter()
        .any(|t| t.contains("text/") || t == "UTF8_STRING")
    {
        match fetch_clipboard_text().await {
            Ok(text) if !text.trim().is_empty() => {
                blocking(move || crate::storage::process_text(&text)).await;
            }
            _ => {}
        }
    }
}

async fn list_mime_types() -> Option<Vec<String>> {
    let output = session::command("wl-paste")
        .args(["--list-types"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let types = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Some(types)
}

async fn fetch_clipboard_text() -> Result<String> {
    let output = session::command("wl-paste")
        .args(["--type", "text/plain", "--no-newline"])
        .output()
        .await?;

    if !output.status.success() {
        bail!("wl-paste failed with status: {}", output.status);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn fetch_clipboard_bytes(mime: &str) -> Result<Vec<u8>> {
    let output = session::command("wl-paste")
        .args(["--type", mime])
        .output()
        .await?;

    if !output.status.success() {
        bail!(
            "wl-paste --type {} failed with status: {}",
            mime,
            output.status
        );
    }

    Ok(output.stdout)
}
