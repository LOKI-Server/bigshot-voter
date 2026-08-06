use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum_extra::extract::{CookieJar, Form};
use serde::Deserialize;

use crate::auth::{AdminUser, ensure_csrf_cookie, set_flash, take_flash, verify_csrf};
use crate::error::{AppError, AppResult};
use crate::models::{MediaType, PanelStatus, UserStatus};
use crate::routes::pages::panel_summary;
use crate::templates::{
    AdminPage, AdminPanelFormPage, NavUser, PasswordResetView, PendingUserView,
};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CsrfForm {
    pub csrf_token: String,
}

#[derive(Debug, Deserialize)]
pub struct NewPanelForm {
    pub title: String,
    pub description: Option<String>,
    pub link: Option<String>,
    pub media_type: String,
    pub csrf_token: String,
}

#[derive(Debug, Deserialize)]
pub struct EditPanelForm {
    pub title: String,
    pub description: Option<String>,
    pub link: Option<String>,
    pub csrf_token: String,
}

/// Shared validation for both panel forms.
struct PanelFields {
    title: String,
    description: String,
    link: String,
}

fn validate_panel_fields(
    title: &str,
    description: Option<String>,
    link: Option<String>,
) -> Result<PanelFields, String> {
    let title = title.trim().to_string();
    if title.is_empty() || title.chars().count() > 120 {
        return Err("Title is required (max 120 characters).".into());
    }

    let description = description
        .unwrap_or_default()
        .trim()
        .chars()
        .take(4000)
        .collect::<String>();

    let link = link.unwrap_or_default().trim().to_string();
    if !link.is_empty() {
        crate::markdown::validate_link(&link)?;
    }

    Ok(PanelFields {
        title,
        description,
        link,
    })
}

pub async fn dashboard(
    State(state): State<AppState>,
    jar: CookieJar,
    AdminUser(user): AdminUser,
) -> AppResult<Response> {
    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let (pending_raw, resets_raw, panels_raw) = tokio::try_join!(
        crate::db::list_pending_users(&state.pool),
        crate::db::list_open_password_resets(&state.pool),
        crate::db::list_panel_summaries(&state.pool),
    )?;

    let pending = pending_raw.iter().map(PendingUserView::from).collect();
    let resets = resets_raw.iter().map(PasswordResetView::from).collect();
    let panels = panels_raw.iter().map(panel_summary).collect();

    let page = AdminPage {
        user: Some(NavUser::from(&user)),
        flash,
        csrf_token,
        pending,
        resets,
        panels,
    };
    Ok((jar, Html(page.render()?)).into_response())
}

pub async fn approve_user(
    State(state): State<AppState>,
    jar: CookieJar,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    crate::db::set_user_status(&state.pool, &id, UserStatus::Approved).await?;
    let jar = set_flash(jar, "User approved.");
    Ok((jar, Redirect::to("/admin")).into_response())
}

pub async fn reject_user(
    State(state): State<AppState>,
    jar: CookieJar,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    crate::db::set_user_status(&state.pool, &id, UserStatus::Rejected).await?;
    let jar = set_flash(jar, "User rejected.");
    Ok((jar, Redirect::to("/admin")).into_response())
}

/// Drop a reset request the admin does not want to relay (or has already
/// handled out of band). The code stops working immediately.
pub async fn dismiss_reset(
    State(state): State<AppState>,
    jar: CookieJar,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    crate::db::delete_password_reset(&state.pool, &id).await?;
    let jar = set_flash(jar, "Reset code dismissed. It can no longer be used.");
    Ok((jar, Redirect::to("/admin")).into_response())
}

pub async fn new_panel_get(jar: CookieJar, AdminUser(user): AdminUser) -> AppResult<Response> {
    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let mut page = AdminPanelFormPage::new_panel(Some(NavUser::from(&user)), csrf_token, None);
    page.flash = flash;
    Ok((jar, Html(page.render()?)).into_response())
}

pub async fn new_panel_post(
    State(state): State<AppState>,
    jar: CookieJar,
    AdminUser(user): AdminUser,
    Form(form): Form<NewPanelForm>,
) -> AppResult<Response> {
    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    // Re-render with what they typed instead of blanking the form on error.
    let redisplay = |jar: CookieJar, csrf_token: String, error: String| -> AppResult<Response> {
        let page = AdminPanelFormPage {
            title: form.title.trim().to_string(),
            description: form.description.clone().unwrap_or_default(),
            link: form.link.clone().unwrap_or_default(),
            ..AdminPanelFormPage::new_panel(Some(NavUser::from(&user)), csrf_token, Some(error))
        };
        Ok((jar, Html(page.render()?)).into_response())
    };

    let fields = match validate_panel_fields(
        &form.title,
        form.description.clone(),
        form.link.clone(),
    ) {
        Ok(f) => f,
        Err(e) => return redisplay(jar, csrf_token, e),
    };

    let Some(media_type) = MediaType::parse(form.media_type.trim()) else {
        return redisplay(jar, csrf_token, "Invalid media type.".into());
    };

    let panel = crate::db::create_panel(
        &state.pool,
        &fields.title,
        &fields.description,
        &fields.link,
        media_type,
        &user.id,
    )
    .await?;

    let jar = set_flash(
        jar,
        &format!("Panel “{}” is open for entries and votes.", panel.title),
    );
    Ok((jar, Redirect::to(&format!("/panels/{}", panel.id))).into_response())
}

pub async fn edit_panel_get(
    State(state): State<AppState>,
    jar: CookieJar,
    AdminUser(user): AdminUser,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let panel = crate::db::find_panel(&state.pool, &id)
        .await?
        .ok_or_else(|| AppError::not_found("Panel not found."))?;

    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let page = AdminPanelFormPage {
        user: Some(NavUser::from(&user)),
        flash,
        csrf_token,
        error: None,
        panel_id: Some(panel.id.clone()),
        is_edit: true,
        title: panel.title.clone(),
        description: panel.description.clone(),
        link: panel.link.clone(),
        media_label: panel.media_type.label().to_string(),
    };
    Ok((jar, Html(page.render()?)).into_response())
}

pub async fn edit_panel_post(
    State(state): State<AppState>,
    jar: CookieJar,
    AdminUser(user): AdminUser,
    Path(id): Path<String>,
    Form(form): Form<EditPanelForm>,
) -> AppResult<Response> {
    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let panel = crate::db::find_panel(&state.pool, &id)
        .await?
        .ok_or_else(|| AppError::not_found("Panel not found."))?;

    let redisplay = |jar: CookieJar, csrf_token: String, error: String| -> AppResult<Response> {
        let page = AdminPanelFormPage {
            user: Some(NavUser::from(&user)),
            flash: None,
            csrf_token,
            error: Some(error),
            panel_id: Some(panel.id.clone()),
            is_edit: true,
            title: form.title.trim().to_string(),
            description: form.description.clone().unwrap_or_default(),
            link: form.link.clone().unwrap_or_default(),
            media_label: panel.media_type.label().to_string(),
        };
        Ok((jar, Html(page.render()?)).into_response())
    };

    let fields =
        match validate_panel_fields(&form.title, form.description.clone(), form.link.clone()) {
            Ok(f) => f,
            Err(e) => return redisplay(jar, csrf_token, e),
        };

    crate::db::update_panel(
        &state.pool,
        &panel.id,
        &fields.title,
        &fields.description,
        &fields.link,
    )
    .await?;

    let jar = set_flash(jar, "Panel updated.");
    Ok((jar, Redirect::to(&format!("/panels/{}", panel.id))).into_response())
}

pub async fn close_panel(
    State(state): State<AppState>,
    jar: CookieJar,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    let panel = crate::db::find_panel(&state.pool, &id)
        .await?
        .ok_or_else(|| AppError::not_found("Panel not found."))?;
    crate::db::set_panel_status(&state.pool, &panel.id, PanelStatus::Closed).await?;
    let jar = set_flash(jar, "Panel closed. Votes are frozen.");
    Ok((jar, Redirect::to("/admin")).into_response())
}

pub async fn open_panel(
    State(state): State<AppState>,
    jar: CookieJar,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    let panel = crate::db::find_panel(&state.pool, &id)
        .await?
        .ok_or_else(|| AppError::not_found("Panel not found."))?;
    crate::db::set_panel_status(&state.pool, &panel.id, PanelStatus::Open).await?;
    let jar = set_flash(jar, "Panel re-opened.");
    Ok((jar, Redirect::to("/admin")).into_response())
}
