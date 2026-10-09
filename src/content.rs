use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Image MIME types we capture, in order of preference.
pub const IMAGE_MIMES: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/bmp",
];

/// File extension used when storing an image of the given MIME type.
pub fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "png",
    }
}

/// Clipboard content type — text or image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClipContent {
    Text {
        text: String,
        preview: String,
    },
    Image {
        /// Path to the full image file
        path: PathBuf,
        /// MIME type (e.g. "image/png")
        mime: String,
    },
}
