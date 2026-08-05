use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;

use crate::auth::{OptionalUser, ensure_csrf_cookie, take_flash};
use crate::error::AppResult;
use crate::templates::{HomePage, NavUser, PanelSummary, PendingPage};
use crate::AppState;

pub async fn home(
    State(state): State<AppState>,
    jar: CookieJar,
    OptionalUser(user): OptionalUser,
) -> AppResult<Response> {
    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let count = crate::db::user_count(&state.pool).await?;
    let needs_bootstrap = count == 0;

    // Titles/status/counts only — media is never exposed on the home page.
    let panels = crate::db::list_panel_summaries(&state.pool)
        .await?
        .iter()
        .map(panel_summary)
        .collect();

    let page = HomePage {
        user: user.as_ref().map(NavUser::from),
        flash,
        csrf_token,
        panels,
        needs_bootstrap,
    };

    Ok((jar, Html(page.render()?)).into_response())
}

pub async fn pending(
    State(_state): State<AppState>,
    jar: CookieJar,
    OptionalUser(user): OptionalUser,
) -> AppResult<Response> {
    let Some(user) = user else {
        return Ok(Redirect::to("/login").into_response());
    };
    if user.is_approved() {
        return Ok(Redirect::to("/panels").into_response());
    }

    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let page = PendingPage {
        user: Some(NavUser::from(&user)),
        flash,
        csrf_token,
    };
    Ok((jar, Html(page.render()?)).into_response())
}

pub(crate) fn panel_summary(listing: &crate::models::PanelListing) -> PanelSummary {
    let p = &listing.panel;
    PanelSummary {
        id: p.id.clone(),
        title: p.title.clone(),
        // Cards get flattened text — Markdown block elements would break the
        // fixed-height card layout.
        excerpt: crate::markdown::to_plain_text(&p.description, 160),
        media_label: p.media_type.label().to_string(),
        status: p.status.as_str().to_string(),
        is_open: p.is_open(),
        entry_count: listing.entry_count,
        vote_count: listing.vote_count,
    }
}
