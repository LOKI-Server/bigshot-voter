//! Askama HTML templates and the view models they render.

use askama::Template;

use crate::media::{accept_attr, allowed_ext_list};
use crate::models::{Entry, MediaType, Panel, PendingUser, format_created_at};

/// Minimal user info for nav chrome.
#[derive(Debug, Clone)]
pub struct NavUser {
    pub username: String,
    pub is_admin: bool,
    pub is_approved: bool,
    pub is_pending: bool,
}

impl From<&crate::models::User> for NavUser {
    fn from(u: &crate::models::User) -> Self {
        Self {
            username: u.username.clone(),
            is_admin: u.is_admin(),
            is_approved: u.is_approved(),
            is_pending: u.is_pending(),
        }
    }
}

#[derive(Template)]
#[template(path = "home.html")]
pub struct HomePage {
    pub user: Option<NavUser>,
    pub flash: Option<String>,
    pub csrf_token: String,
    pub panels: Vec<PanelSummary>,
    pub needs_bootstrap: bool,
}

#[derive(Debug, Clone)]
pub struct PanelSummary {
    pub id: String,
    pub title: String,
    /// Markdown flattened to plain text — cards can't render block elements.
    pub excerpt: String,
    pub media_label: String,
    pub status: String,
    pub is_open: bool,
    pub entry_count: i64,
    pub vote_count: i64,
}

#[derive(Template)]
#[template(path = "register.html")]
pub struct RegisterPage {
    pub user: Option<NavUser>,
    pub flash: Option<String>,
    pub csrf_token: String,
    pub error: Option<String>,
    pub is_first_user: bool,
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginPage {
    pub user: Option<NavUser>,
    pub flash: Option<String>,
    pub csrf_token: String,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "pending.html")]
pub struct PendingPage {
    pub user: Option<NavUser>,
    pub flash: Option<String>,
    pub csrf_token: String,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsPage {
    pub user: Option<NavUser>,
    pub flash: Option<String>,
    pub csrf_token: String,
    pub username: String,
    /// Empty when the account has no sign-in email.
    pub email: String,
    pub identity_error: Option<String>,
    pub password_error: Option<String>,
}

#[derive(Template)]
#[template(path = "panels_list.html")]
pub struct PanelsListPage {
    pub user: Option<NavUser>,
    pub flash: Option<String>,
    pub csrf_token: String,
    pub panels: Vec<PanelSummary>,
}

#[derive(Template)]
#[template(path = "panel_detail.html")]
pub struct PanelDetailPage {
    pub user: Option<NavUser>,
    pub flash: Option<String>,
    pub csrf_token: String,
    pub panel: PanelView,
    pub entries: Vec<EntryView>,
    pub my_entry_id: Option<String>,
    pub my_vote_entry_id: Option<String>,
    pub can_upload: bool,
    pub can_vote: bool,
    pub total_votes: i64,
    pub max_upload_label: String,
    /// Shows the "Edit panel" affordance.
    pub is_admin: bool,
}

#[derive(Debug, Clone)]
pub struct PanelView {
    pub id: String,
    pub title: String,
    /// Pre-rendered Markdown. Safe to emit unescaped — see [`crate::markdown`].
    pub description_html: String,
    pub has_description: bool,
    /// Optional related URL; empty when unset.
    pub link: String,
    pub has_link: bool,
    /// `example.com/path` — the link without its scheme, for display.
    pub link_label: String,
    pub media_type: String,
    pub media_label: String,
    pub status: String,
    pub is_open: bool,
    pub is_image: bool,
    pub is_audio: bool,
    pub is_video: bool,
    pub is_other: bool,
    /// `accept="…"` for the upload input.
    pub accept: String,
    /// Human-readable allowed extension list.
    pub allowed_extensions: String,
}

impl From<&Panel> for PanelView {
    fn from(p: &Panel) -> Self {
        Self {
            id: p.id.clone(),
            title: p.title.clone(),
            description_html: crate::markdown::to_html(&p.description),
            has_description: !p.description.trim().is_empty(),
            link: p.link.clone(),
            has_link: !p.link.is_empty(),
            link_label: p
                .link
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches('/')
                .to_string(),
            media_type: p.media_type.as_str().to_string(),
            media_label: p.media_type.label().to_string(),
            status: p.status.as_str().to_string(),
            is_open: p.is_open(),
            is_image: p.media_type == MediaType::Image,
            is_audio: p.media_type == MediaType::Audio,
            is_video: p.media_type == MediaType::Video,
            is_other: p.media_type == MediaType::Other,
            accept: accept_attr(p.media_type),
            allowed_extensions: allowed_ext_list(p.media_type),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EntryView {
    pub id: String,
    pub title: String,
    pub username: String,
    pub vote_count: i64,
    pub is_mine: bool,
    pub is_my_vote: bool,
    /// 1-based position in the vote-sorted list.
    pub rank: usize,
    /// Share of the panel's total votes, for the tile meter.
    pub share_pct: u32,
    /// Top entry of a closed panel with at least one vote.
    pub is_winner: bool,
}

impl EntryView {
    /// Build the tile views for one panel. Entries must already be sorted by
    /// vote count descending (see `db::list_entries_for_panel`).
    pub fn build(
        entries: &[Entry],
        my_user_id: Option<&str>,
        my_vote: Option<&str>,
        panel_is_open: bool,
    ) -> Vec<Self> {
        // Entries are vote-sorted, so the first one holds the high score.
        let leader = entries.first().map(|e| e.vote_count).unwrap_or(0);
        let total: i64 = entries.iter().map(|e| e.vote_count).sum();

        entries
            .iter()
            .enumerate()
            .map(|(i, e)| Self {
                id: e.id.clone(),
                title: e.title.clone(),
                username: e.username.clone(),
                vote_count: e.vote_count,
                is_mine: my_user_id == Some(e.user_id.as_str()),
                is_my_vote: my_vote == Some(e.id.as_str()),
                rank: i + 1,
                // Share of every vote cast, so the numbers add up to 100%.
                // (Against the leader instead, a runner-up with half the
                // leader's votes would read "50%" and the column would sum
                // to well over 100.)
                share_pct: percent_of(e.vote_count, total),
                is_winner: !panel_is_open && e.vote_count > 0 && e.vote_count == leader,
            })
            .collect()
    }
}

/// Each share is rounded independently, so tied entries always display the same
/// number. Rounding can leave the column a point either side of 100.
fn percent_of(value: i64, total: i64) -> u32 {
    if total <= 0 || value <= 0 {
        return 0;
    }
    ((value.min(total) as f64 / total as f64) * 100.0).round() as u32
}

#[derive(Template)]
#[template(path = "admin.html")]
pub struct AdminPage {
    pub user: Option<NavUser>,
    pub flash: Option<String>,
    pub csrf_token: String,
    pub pending: Vec<PendingUserView>,
    pub panels: Vec<PanelSummary>,
}

#[derive(Debug, Clone)]
pub struct PendingUserView {
    pub id: String,
    pub username: String,
    pub created_at: String,
}

impl From<&PendingUser> for PendingUserView {
    fn from(p: &PendingUser) -> Self {
        Self {
            id: p.id.clone(),
            username: p.username.clone(),
            created_at: format_created_at(&p.created_at),
        }
    }
}

/// Shared by "new panel" and "edit panel" — the only differences are the form
/// action, whether media type is still selectable, and the button label.
#[derive(Template)]
#[template(path = "admin_panel_form.html")]
pub struct AdminPanelFormPage {
    pub user: Option<NavUser>,
    pub flash: Option<String>,
    pub csrf_token: String,
    pub error: Option<String>,
    /// `Some(id)` when editing an existing panel.
    pub panel_id: Option<String>,
    pub is_edit: bool,
    pub title: String,
    pub description: String,
    pub link: String,
    /// Fixed once entries exist, so it is shown read-only when editing.
    pub media_label: String,
}

impl AdminPanelFormPage {
    pub fn new_panel(user: Option<NavUser>, csrf_token: String, error: Option<String>) -> Self {
        Self {
            user,
            flash: None,
            csrf_token,
            error,
            panel_id: None,
            is_edit: false,
            title: String::new(),
            description: String::new(),
            link: String::new(),
            media_label: String::new(),
        }
    }
}

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorPage {
    pub title: String,
    pub message: String,
    pub user: Option<NavUser>,
    pub flash: Option<String>,
    pub csrf_token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, user: &str, votes: i64) -> Entry {
        Entry {
            id: id.into(),
            panel_id: "p".into(),
            user_id: format!("u-{user}"),
            username: user.into(),
            title: id.into(),
            original_filename: "f.png".into(),
            stored_path: "p/f.png".into(),
            content_type: "image/png".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            vote_count: votes,
        }
    }

    #[test]
    fn shares_are_of_the_total_not_the_leader() {
        // 4 votes across 4 entries, as seen on a real panel.
        let entries = [
            entry("icon", "aspenini", 2),
            entry("chicken", "caitlin", 1),
            entry("best", "nomad", 1),
            entry("greatest", "jax", 0),
        ];
        let views = EntryView::build(&entries, None, None, true);
        let shares: Vec<u32> = views.iter().map(|v| v.share_pct).collect();

        assert_eq!(shares, vec![50, 25, 25, 0]);
        assert_eq!(shares.iter().sum::<u32>(), 100, "shares must total 100%");
    }

    #[test]
    fn a_lone_voted_entry_takes_the_whole_share() {
        let entries = [entry("a", "x", 3), entry("b", "y", 0)];
        let views = EntryView::build(&entries, None, None, true);
        assert_eq!(views[0].share_pct, 100);
        assert_eq!(views[1].share_pct, 0);
    }

    #[test]
    fn no_votes_means_no_share_anywhere() {
        let entries = [entry("a", "x", 0), entry("b", "y", 0)];
        let views = EntryView::build(&entries, None, None, true);
        assert!(views.iter().all(|v| v.share_pct == 0));
    }

    #[test]
    fn tied_entries_display_identical_shares() {
        let entries = [entry("a", "x", 1), entry("b", "y", 1), entry("c", "z", 1)];
        let views = EntryView::build(&entries, None, None, true);
        let shares: Vec<u32> = views.iter().map(|v| v.share_pct).collect();
        assert_eq!(shares, vec![33, 33, 33], "ties must not be nudged apart");
    }

    #[test]
    fn winner_is_the_top_scorer_of_a_closed_panel_only() {
        let entries = [entry("a", "x", 2), entry("b", "y", 1)];

        let open = EntryView::build(&entries, None, None, true);
        assert!(!open[0].is_winner, "open panels have no winner yet");

        let closed = EntryView::build(&entries, None, None, false);
        assert!(closed[0].is_winner);
        assert!(!closed[1].is_winner);
        assert_eq!(closed[0].rank, 1);
        assert_eq!(closed[1].rank, 2);

        // A closed panel where nobody voted has no winner.
        let empty = [entry("a", "x", 0), entry("b", "y", 0)];
        assert!(EntryView::build(&empty, None, None, false)
            .iter()
            .all(|v| !v.is_winner));
    }
}
