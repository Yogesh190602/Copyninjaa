use anyhow::{bail, Result};
use log::{debug, info, warn};
use std::time::Duration;
use tokio::process::Command;

/// Start monitoring the X11 clipboard by polling `xclip` every 500ms.
/// This function runs indefinitely.
pub async fn start() -> Result<()> {
    info!("Starting X11 clipboard watcher (xclip polling)");

    // Test that xclip can connect to the display
    let test = Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
        .await?;

    let stderr = String::from_utf8_lossy(&test.stderr);
    if stderr.contains("Can't open display") {
        bail!("xclip cannot open display: {}", stderr.trim());
    }

    info!("xclip connected, polling every 500ms");

    let mut last_hash = get_clipboard_hash().await;
    let mut interval = tokio::time::interval(Duration::from_millis(500));

    loop {
        interval.tick().await;
        let current_hash = get_clipboard_hash().await;
        if current_hash != last_hash {
            debug!("X11 clipboard changed (hash: {} -> {})", last_hash, current_hash);
            last_hash = current_hash;
            fetch_and_store().await;
        }
    }
}

/// Detect available types and fetch the best one.
async fn fetch_and_store() {
    // Check available TARGETS
    let targets = list_targets().await.unwrap_or_default();

    // Try image types first
    let image_mimes = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/bmp"];
    for mime in &image_mimes {
        if targets.contains(&mime.to_string()) {
            if let Some(data) = get_clipboard_bytes(mime).await {
                if !data.is_empty() {
                    debug!("Captured X11 image clipboard ({}, {} bytes)", mime, data.len());
                    crate::storage::process_image(&data, mime);
                    return;
                }
            }
        }
    }

    // File-manager image copy: Nautilus/Files puts text/uri-list (a file:// URI)
    // in the clipboard, not image bytes. If the URI points to a local image,
    // read the file and store its bytes as a proper image entry.
    if targets.contains(&"text/uri-list".to_string()) {
        if let Some(uri_list) = get_clipboard_bytes("text/uri-list").await {
            if let Some((data, mime)) = crate::daemon::read_image_from_uri_list(&uri_list) {
                debug!("Captured X11 image from URI list ({}, {} bytes)", mime, data.len());
                crate::storage::process_image(&data, &mime);
                return;
            }
        }
    }

    // Fall back to text
    if let Some(text) = get_clipboard_text().await {
        if !text.trim().is_empty() {
            crate::storage::process_text(&text);
        }
    }
}

async fn list_targets() -> Option<Vec<String>> {
    let output = Command::new("xclip")
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
    let output = Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
        .await
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        warn!("xclip returned non-zero status");
        None
    }
}

async fn get_clipboard_bytes(mime: &str) -> Option<Vec<u8>> {
    let output = Command::new("xclip")
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

async fn get_clipboard_hash() -> String {
    // Hash TARGETS + preferred payload so image↔image and text↔image
    // transitions are detected, not just text changes.
    let targets = list_targets().await.unwrap_or_default();
    if targets.is_empty() {
        return String::new();
    }

    let image_mimes = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/bmp"];
    let payload = if let Some(mime) = image_mimes.iter().find(|m| targets.contains(&m.to_string())) {
        get_clipboard_bytes(mime).await.unwrap_or_default()
    } else {
        get_clipboard_text().await.map(|s| s.into_bytes()).unwrap_or_default()
    };

    let mut to_hash: Vec<u8> = targets.join(",").into_bytes();
    to_hash.extend_from_slice(&payload);
    crate::storage::get_hash_bytes(&to_hash)
}
