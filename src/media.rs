//! Upload validation: media-type allowlists and safe extensions.

use std::path::{Path, PathBuf};

use crate::models::MediaType;

#[derive(Debug, Clone)]
pub struct ValidatedUpload {
    pub extension: String,
    pub content_type: String,
}

/// Validate extension + declared content-type against the panel media type.
pub fn validate_upload(
    media_type: MediaType,
    original_filename: &str,
    declared_content_type: Option<&str>,
) -> Result<ValidatedUpload, String> {
    let ext = Path::new(original_filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .ok_or_else(|| "File must have an extension.".to_string())?;

    if ext.contains('/') || ext.contains('\\') || ext.contains("..") {
        return Err("Invalid file extension.".into());
    }

    let allowed = allowed_for(media_type);
    let matched = allowed
        .iter()
        .find(|a| a.extensions.contains(&ext.as_str()))
        .ok_or_else(|| {
            format!(
                "Extension .{ext} is not allowed for {} panels. Allowed: {}.",
                media_type.label(),
                allowed_ext_list(media_type)
            )
        })?;

    // Prefer our canonical type for the extension; optionally check declaration.
    let content_type = matched.content_type.to_string();
    if let Some(declared) = declared_content_type {
        let declared = declared.split(';').next().unwrap_or(declared).trim();
        if !declared.is_empty()
            && declared != "application/octet-stream"
            && !matched.also_accepts.contains(&declared)
            && declared != matched.content_type
        {
            // Soft check: only warn via reject if clearly wrong family.
            if !content_type_compatible(media_type, declared) {
                return Err(format!(
                    "Content type '{declared}' does not match panel type {}.",
                    media_type.label()
                ));
            }
        }
    }

    Ok(ValidatedUpload {
        extension: ext,
        content_type,
    })
}

fn content_type_compatible(media_type: MediaType, declared: &str) -> bool {
    match media_type {
        MediaType::Image => declared.starts_with("image/"),
        MediaType::Audio => declared.starts_with("audio/"),
        MediaType::Video => declared.starts_with("video/"),
        MediaType::Other => true,
    }
}

struct Allow {
    extensions: &'static [&'static str],
    content_type: &'static str,
    also_accepts: &'static [&'static str],
}

fn allowed_for(media_type: MediaType) -> &'static [Allow] {
    match media_type {
        MediaType::Image => &[
            Allow {
                extensions: &["jpg", "jpeg"],
                content_type: "image/jpeg",
                also_accepts: &["image/jpg"],
            },
            Allow {
                extensions: &["png"],
                content_type: "image/png",
                also_accepts: &[],
            },
            Allow {
                extensions: &["gif"],
                content_type: "image/gif",
                also_accepts: &[],
            },
            Allow {
                extensions: &["webp"],
                content_type: "image/webp",
                also_accepts: &[],
            },
        ],
        MediaType::Audio => &[
            Allow {
                extensions: &["mp3"],
                content_type: "audio/mpeg",
                also_accepts: &["audio/mp3"],
            },
            Allow {
                extensions: &["ogg"],
                content_type: "audio/ogg",
                also_accepts: &[],
            },
            Allow {
                extensions: &["wav"],
                content_type: "audio/wav",
                also_accepts: &["audio/x-wav", "audio/wave"],
            },
            Allow {
                extensions: &["m4a"],
                content_type: "audio/mp4",
                also_accepts: &["audio/x-m4a", "audio/m4a"],
            },
            Allow {
                extensions: &["flac"],
                content_type: "audio/flac",
                also_accepts: &["audio/x-flac"],
            },
        ],
        MediaType::Video => &[
            Allow {
                extensions: &["mp4"],
                content_type: "video/mp4",
                also_accepts: &[],
            },
            Allow {
                extensions: &["webm"],
                content_type: "video/webm",
                also_accepts: &[],
            },
            Allow {
                extensions: &["mov"],
                content_type: "video/quicktime",
                also_accepts: &[],
            },
            Allow {
                extensions: &["mkv"],
                content_type: "video/x-matroska",
                also_accepts: &[],
            },
        ],
        MediaType::Other => &[
            Allow {
                extensions: &["pdf"],
                content_type: "application/pdf",
                also_accepts: &[],
            },
            Allow {
                extensions: &["zip"],
                content_type: "application/zip",
                also_accepts: &["application/x-zip-compressed"],
            },
            Allow {
                extensions: &["txt"],
                content_type: "text/plain",
                also_accepts: &[],
            },
            Allow {
                extensions: &["png", "jpg", "jpeg", "gif", "webp"],
                content_type: "application/octet-stream",
                also_accepts: &[],
            },
            Allow {
                extensions: &["mp3", "ogg", "wav", "mp4", "webm"],
                content_type: "application/octet-stream",
                also_accepts: &[],
            },
        ],
    }
}

/// Sorted, de-duplicated extensions accepted for a panel type (`.png`, …).
pub fn allowed_extensions(media_type: MediaType) -> Vec<String> {
    let mut exts: Vec<&str> = allowed_for(media_type)
        .iter()
        .flat_map(|a| a.extensions.iter().copied())
        .collect();
    exts.sort_unstable();
    exts.dedup();
    exts.iter().map(|e| format!(".{e}")).collect()
}

/// Human-readable list for error messages and upload hints.
pub fn allowed_ext_list(media_type: MediaType) -> String {
    allowed_extensions(media_type).join(", ")
}

/// Value for an `<input type="file" accept="…">` attribute.
pub fn accept_attr(media_type: MediaType) -> String {
    allowed_extensions(media_type).join(",")
}

/// Ensure a stored relative path stays under the uploads root.
pub fn resolve_upload_path(uploads_root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty()
        || relative.contains("..")
        || Path::new(relative).is_absolute()
        || relative.contains('\\')
    {
        return Err("Invalid stored path.".into());
    }
    let full = uploads_root.join(relative);
    let root = uploads_root
        .canonicalize()
        .map_err(|_| "Uploads directory missing.".to_string())?;
    // File may not exist yet for write; for read, canonicalize parent + check prefix.
    if let Ok(canon) = full.canonicalize() {
        if !canon.starts_with(&root) {
            return Err("Path escapes uploads directory.".into());
        }
        return Ok(canon);
    }
    // Not yet on disk: verify parent is under root.
    if let Some(parent) = full.parent()
        && let Ok(p) = parent.canonicalize()
        && !p.starts_with(&root)
    {
        return Err("Path escapes uploads directory.".into());
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_png_for_image() {
        let v = validate_upload(MediaType::Image, "logo.PNG", Some("image/png")).unwrap();
        assert_eq!(v.extension, "png");
        assert_eq!(v.content_type, "image/png");
    }

    #[test]
    fn rejects_exe_for_image() {
        assert!(validate_upload(MediaType::Image, "evil.exe", None).is_err());
    }

    #[test]
    fn accepts_mp3_for_audio() {
        let v = validate_upload(MediaType::Audio, "track.mp3", Some("audio/mpeg")).unwrap();
        assert_eq!(v.extension, "mp3");
    }
}
