//! Domain models shared across routes and templates.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    Pending,
    Approved,
    Rejected,
}

impl UserStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            UserStatus::Pending => "pending",
            UserStatus::Approved => "approved",
            UserStatus::Rejected => "rejected",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(UserStatus::Pending),
            "approved" => Some(UserStatus::Approved),
            "rejected" => Some(UserStatus::Rejected),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Image,
    Audio,
    Video,
    Other,
}

impl MediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Image => "image",
            MediaType::Audio => "audio",
            MediaType::Video => "video",
            MediaType::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "image" => Some(MediaType::Image),
            "audio" => Some(MediaType::Audio),
            "video" => Some(MediaType::Video),
            "other" => Some(MediaType::Other),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MediaType::Image => "Image",
            MediaType::Audio => "Audio / Music",
            MediaType::Video => "Video",
            MediaType::Other => "Other file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelStatus {
    Open,
    Closed,
}

impl PanelStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PanelStatus::Open => "open",
            PanelStatus::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(PanelStatus::Open),
            "closed" => Some(PanelStatus::Closed),
            _ => None,
        }
    }
}

/// Format an RFC3339 timestamp for display (`2024-01-15 12:30 UTC`).
/// Avoids pulling chrono — we only ever store/display these strings.
pub fn format_created_at(rfc3339: &str) -> String {
    // "2024-01-15T12:30:00.123456789+00:00" / "...Z"
    if rfc3339.len() >= 16 && rfc3339.as_bytes().get(10) == Some(&b'T') {
        format!("{} {} UTC", &rfc3339[0..10], &rfc3339[11..16])
    } else {
        rfc3339.to_string()
    }
}

/// Render a byte count as a short human-readable size (`32 MB`, `1.5 GB`).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["bytes", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else if value < 10.0 && value.fract() >= 0.05 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{} {}", value.round(), UNITS[unit])
    }
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub username: String,
    /// Optional alternative sign-in identifier.
    pub email: Option<String>,
    pub password_hash: String,
    pub role: Role,
    pub status: UserStatus,
    /// RFC3339 timestamp
    pub created_at: String,
}

impl User {
    #[inline]
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }

    #[inline]
    pub fn is_approved(&self) -> bool {
        self.status == UserStatus::Approved
    }

    #[inline]
    pub fn is_pending(&self) -> bool {
        self.status == UserStatus::Pending
    }
}

#[derive(Debug, Clone)]
pub struct Panel {
    pub id: String,
    pub title: String,
    /// Markdown source; render with [`crate::markdown`].
    pub description: String,
    /// Optional related URL. Empty when unset.
    pub link: String,
    pub media_type: MediaType,
    pub status: PanelStatus,
    pub created_by: String,
    /// RFC3339 timestamp
    pub created_at: String,
}

impl Panel {
    #[inline]
    pub fn is_open(&self) -> bool {
        self.status == PanelStatus::Open
    }
}

/// A panel plus the aggregate counts shown on index pages.
#[derive(Debug, Clone)]
pub struct PanelListing {
    pub panel: Panel,
    pub entry_count: i64,
    pub vote_count: i64,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub panel_id: String,
    pub user_id: String,
    pub username: String,
    pub title: String,
    pub original_filename: String,
    pub stored_path: String,
    pub content_type: String,
    /// Optional master/source file (e.g. `.kra`, `.paint`) original name.
    pub master_original_filename: Option<String>,
    /// Relative path under uploads for the master file, when present.
    pub master_stored_path: Option<String>,
    /// RFC3339 timestamp
    pub created_at: String,
    pub vote_count: i64,
}

impl Entry {
    #[inline]
    pub fn has_master(&self) -> bool {
        self.master_stored_path
            .as_ref()
            .is_some_and(|p| !p.is_empty())
    }
}

#[derive(Debug, Clone)]
pub struct PendingUser {
    pub id: String,
    pub username: String,
    /// RFC3339 timestamp
    pub created_at: String,
}
