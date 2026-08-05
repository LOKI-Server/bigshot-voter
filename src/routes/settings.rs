//! Account settings: sign-in identity (username / optional email) and password.

use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum_extra::extract::{CookieJar, Form};
use serde::Deserialize;

use crate::AppState;
use crate::auth::{
    AuthUser, create_session_cookie, ensure_csrf_cookie, hash_password_async, set_flash,
    take_flash, validate_email, validate_password, validate_username, verify_csrf,
    verify_password_async,
};
use crate::error::AppResult;
use crate::models::User;
use crate::templates::{NavUser, SettingsPage};

#[derive(Debug, Deserialize)]
pub struct IdentityForm {
    pub username: String,
    pub email: String,
    pub current_password: String,
    pub csrf_token: String,
}

#[derive(Debug, Deserialize)]
pub struct PasswordForm {
    pub current_password: String,
    pub new_password: String,
    pub new_password_confirm: String,
    pub csrf_token: String,
}

pub async fn settings_get(jar: CookieJar, AuthUser(user): AuthUser) -> AppResult<Response> {
    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    Ok((jar, Html(page(&user, csrf_token, flash, None, None).render()?)).into_response())
}

pub async fn update_identity(
    State(state): State<AppState>,
    jar: CookieJar,
    AuthUser(user): AuthUser,
    Form(form): Form<IdentityForm>,
) -> AppResult<Response> {
    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let username = form.username.trim().to_string();
    let email = form.email.trim().to_string();
    let email = (!email.is_empty()).then_some(email);

    let reject = |jar: CookieJar, csrf_token: String, msg: String| -> AppResult<Response> {
        let page = page(&user, csrf_token, None, Some(msg), None);
        Ok((jar, Html(page.render()?)).into_response())
    };

    // Changing how you sign in is credential-sensitive: prove it's really you.
    if !verify_password_async(form.current_password, user.password_hash.clone()).await {
        return reject(jar, csrf_token, "Current password is incorrect.".into());
    }

    if let Err(e) = validate_username(&username) {
        return reject(jar, csrf_token, e);
    }
    if let Some(addr) = email.as_deref()
        && let Err(e) = validate_email(addr)
    {
        return reject(jar, csrf_token, e);
    }

    // Taken-by-someone-else checks; matching your own row is a no-op rename.
    if let Some(existing) = crate::db::find_user_by_username(&state.pool, &username).await?
        && existing.id != user.id
    {
        return reject(jar, csrf_token, "That username is already taken.".into());
    }
    if let Some(addr) = email.as_deref()
        && let Some(existing) = crate::db::find_user_by_email(&state.pool, addr).await?
        && existing.id != user.id
    {
        return reject(
            jar,
            csrf_token,
            "That email is already used by another account.".into(),
        );
    }

    match crate::db::update_user_identity(&state.pool, &user.id, &username, email.as_deref()).await
    {
        Ok(()) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return reject(
                jar,
                csrf_token,
                "That username or email is already taken.".into(),
            );
        }
        Err(e) => return Err(e.into()),
    }

    let jar = set_flash(jar, "Account details updated.");
    Ok((jar, Redirect::to("/settings")).into_response())
}

pub async fn update_password(
    State(state): State<AppState>,
    jar: CookieJar,
    AuthUser(user): AuthUser,
    Form(form): Form<PasswordForm>,
) -> AppResult<Response> {
    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let reject = |jar: CookieJar, csrf_token: String, msg: String| -> AppResult<Response> {
        let page = page(&user, csrf_token, None, None, Some(msg));
        Ok((jar, Html(page.render()?)).into_response())
    };

    if !verify_password_async(form.current_password, user.password_hash.clone()).await {
        return reject(jar, csrf_token, "Current password is incorrect.".into());
    }
    if let Err(e) = validate_password(&form.new_password) {
        return reject(jar, csrf_token, e);
    }
    if form.new_password != form.new_password_confirm {
        return reject(jar, csrf_token, "New passwords do not match.".into());
    }

    let hash = hash_password_async(form.new_password).await?;
    crate::db::update_user_password(&state.pool, &user.id, &hash).await?;

    // Refresh the session cookie so the current browser keeps a full-length
    // session rather than aging out on the pre-change clock.
    let jar = jar.add(create_session_cookie(&state.config.secret, &user.id));
    let jar = set_flash(jar, "Password updated.");
    Ok((jar, Redirect::to("/settings")).into_response())
}

fn page(
    user: &User,
    csrf_token: String,
    flash: Option<String>,
    identity_error: Option<String>,
    password_error: Option<String>,
) -> SettingsPage {
    SettingsPage {
        user: Some(NavUser::from(user)),
        flash,
        csrf_token,
        username: user.username.clone(),
        email: user.email.clone().unwrap_or_default(),
        identity_error,
        password_error,
    }
}
