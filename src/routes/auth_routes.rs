use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum_extra::extract::{CookieJar, Form};
use serde::Deserialize;

use crate::auth::{
    OptionalUser, create_session_cookie, clear_session_cookie, dummy_password_work,
    ensure_csrf_cookie, hash_password_async, set_flash, take_flash, validate_password,
    validate_username, verify_csrf, verify_password_async,
};
use crate::error::AppResult;
use crate::models::{Role, UserStatus};
use crate::templates::{LoginPage, RegisterPage};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct RegisterForm {
    pub username: String,
    pub password: String,
    pub password_confirm: String,
    pub csrf_token: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
    pub csrf_token: String,
}

#[derive(Debug, Deserialize)]
pub struct CsrfForm {
    pub csrf_token: String,
}

pub async fn register_get(
    State(state): State<AppState>,
    jar: CookieJar,
    OptionalUser(user): OptionalUser,
) -> AppResult<Response> {
    if user.is_some() {
        return Ok(Redirect::to("/").into_response());
    }
    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let is_first_user = crate::db::user_count(&state.pool).await? == 0;
    let page = RegisterPage {
        user: None,
        flash,
        csrf_token,
        error: None,
        is_first_user,
    };
    Ok((jar, Html(page.render()?)).into_response())
}

pub async fn register_post(
    State(state): State<AppState>,
    jar: CookieJar,
    OptionalUser(user): OptionalUser,
    Form(form): Form<RegisterForm>,
) -> AppResult<Response> {
    if user.is_some() {
        return Ok(Redirect::to("/").into_response());
    }

    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;

    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let is_first_user = crate::db::user_count(&state.pool).await? == 0;

    let render_err = |jar: CookieJar, csrf_token: String, error: String| -> AppResult<Response> {
        let page = RegisterPage {
            user: None,
            flash: None,
            csrf_token,
            error: Some(error),
            is_first_user,
        };
        Ok((jar, Html(page.render()?)).into_response())
    };

    let username = form.username.trim().to_string();
    if let Err(e) = validate_username(&username) {
        return render_err(jar, csrf_token, e);
    }
    if let Err(e) = validate_password(&form.password) {
        return render_err(jar, csrf_token, e);
    }
    if form.password != form.password_confirm {
        return render_err(jar, csrf_token, "Passwords do not match.".into());
    }

    if crate::db::find_user_by_username(&state.pool, &username)
        .await?
        .is_some()
    {
        return render_err(jar, csrf_token, "That username is already taken.".into());
    }

    let password_hash = hash_password_async(form.password).await?;
    let (role, status) = if is_first_user {
        (Role::Admin, UserStatus::Approved)
    } else {
        (Role::User, UserStatus::Pending)
    };

    let new_user = match crate::db::create_user(
        &state.pool,
        &username,
        &password_hash,
        role,
        status,
    )
    .await
    {
        Ok(u) => u,
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            return render_err(jar, csrf_token, "That username is already taken.".into());
        }
        Err(e) => return Err(e.into()),
    };

    let session = create_session_cookie(&state.config.secret, &new_user.id);
    let jar = jar.add(session);
    let msg = if is_first_user {
        "Welcome! You are the overseeing admin."
    } else {
        "Account created. An admin must approve you before you can vote or upload."
    };
    let jar = set_flash(jar, msg);

    if is_first_user {
        Ok((jar, Redirect::to("/admin")).into_response())
    } else {
        Ok((jar, Redirect::to("/pending")).into_response())
    }
}

pub async fn login_get(
    jar: CookieJar,
    OptionalUser(user): OptionalUser,
) -> AppResult<Response> {
    if let Some(u) = &user {
        if u.is_pending() {
            return Ok(Redirect::to("/pending").into_response());
        }
        return Ok(Redirect::to("/panels").into_response());
    }
    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let page = LoginPage {
        user: None,
        flash,
        csrf_token,
        error: None,
    };
    Ok((jar, Html(page.render()?)).into_response())
}

pub async fn login_post(
    State(state): State<AppState>,
    jar: CookieJar,
    OptionalUser(user): OptionalUser,
    Form(form): Form<LoginForm>,
) -> AppResult<Response> {
    if user.is_some() {
        return Ok(Redirect::to("/").into_response());
    }

    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let fail = |jar: CookieJar, csrf_token: String| -> AppResult<Response> {
        let page = LoginPage {
            user: None,
            flash: None,
            csrf_token,
            error: Some("Invalid credentials.".into()),
        };
        Ok((jar, Html(page.render()?)).into_response())
    };

    // Members may sign in with a username or, if they set one, their email.
    let identifier = form.username.trim();
    let Some(db_user) = crate::db::find_user_by_login(&state.pool, identifier).await? else {
        // Dummy Argon2 work to reduce timing oracle on unknown accounts.
        dummy_password_work(form.password).await;
        return fail(jar, csrf_token);
    };

    if !verify_password_async(form.password, db_user.password_hash.clone()).await {
        return fail(jar, csrf_token);
    }

    if db_user.status == UserStatus::Rejected {
        let page = LoginPage {
            user: None,
            flash: None,
            csrf_token,
            error: Some("This account was rejected.".into()),
        };
        return Ok((jar, Html(page.render()?)).into_response());
    }

    let session = create_session_cookie(&state.config.secret, &db_user.id);
    let jar = jar.add(session);
    let jar = set_flash(jar, &format!("Welcome back, {}.", db_user.username));

    let dest = if db_user.is_pending() {
        "/pending"
    } else if db_user.is_admin() {
        "/admin"
    } else {
        "/panels"
    };
    Ok((jar, Redirect::to(dest)).into_response())
}

pub async fn logout_post(
    State(_state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    let jar = jar.add(clear_session_cookie());
    let jar = set_flash(jar, "Logged out.");
    Ok((jar, Redirect::to("/")).into_response())
}
