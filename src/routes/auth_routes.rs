use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum_extra::extract::{CookieJar, Form};
use serde::Deserialize;

use crate::auth::{
    OptionalUser, RESET_CODE_LEN, create_session_cookie, clear_session_cookie,
    dummy_password_work, ensure_csrf_cookie, generate_reset_code, hash_password_async,
    normalize_reset_code, set_flash, take_flash, validate_password, validate_username,
    verify_csrf, verify_password_async,
};
use crate::error::AppResult;
use crate::models::{Role, UserStatus};
use crate::templates::{ForgotPasswordPage, LoginPage, RegisterPage, ResetPasswordPage};
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

#[derive(Debug, Deserialize)]
pub struct ForgotForm {
    /// Username or email, same as the login field.
    pub identifier: String,
    pub csrf_token: String,
}

#[derive(Debug, Deserialize)]
pub struct ResetForm {
    pub code: String,
    pub new_password: String,
    pub new_password_confirm: String,
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

// --- Forgotten passwords ------------------------------------------------------
//
// There is no mail path in this app, so the admin is the delivery channel: the
// member asks here, the admin reads the code off their dashboard (and the
// console log) and passes it on however they already talk, and the member
// redeems it at /reset.

/// Said whether or not the account exists — an attacker must not learn which
/// usernames are real by watching this form.
const FORGOT_ACK: &str =
    "If that account exists, the admin now has a one-time code for it. \
     Ask them for the code, then enter it below.";

pub async fn forgot_get(jar: CookieJar, OptionalUser(user): OptionalUser) -> AppResult<Response> {
    // Already signed in? /settings is the direct route to a new password.
    if user.is_some() {
        return Ok(Redirect::to("/settings").into_response());
    }
    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let page = ForgotPasswordPage {
        user: None,
        flash,
        csrf_token,
        error: None,
    };
    Ok((jar, Html(page.render()?)).into_response())
}

pub async fn forgot_post(
    State(state): State<AppState>,
    jar: CookieJar,
    OptionalUser(user): OptionalUser,
    Form(form): Form<ForgotForm>,
) -> AppResult<Response> {
    if user.is_some() {
        return Ok(Redirect::to("/settings").into_response());
    }

    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let identifier = form.identifier.trim();
    if identifier.is_empty() {
        let page = ForgotPasswordPage {
            user: None,
            flash: None,
            csrf_token,
            error: Some("Enter your username or email.".into()),
        };
        return Ok((jar, Html(page.render()?)).into_response());
    }

    if let Some(account) = crate::db::find_user_by_login(&state.pool, identifier).await?
        // A rejected account has nothing to come back to.
        && account.status != UserStatus::Rejected
    {
        let code = generate_reset_code();
        let code_key = normalize_reset_code(&code);
        crate::db::create_password_reset(&state.pool, &account.id, &code, &code_key).await?;
        // The console is the admin's other window onto the server (tray →
        // Show Console), so the code lands there as well as on /admin.
        tracing::info!(
            username = %account.username,
            code = %code,
            "password reset code issued — pass it to the member"
        );
    }

    let jar = set_flash(jar, FORGOT_ACK);
    Ok((jar, Redirect::to("/reset")).into_response())
}

pub async fn reset_get(jar: CookieJar, OptionalUser(user): OptionalUser) -> AppResult<Response> {
    if user.is_some() {
        return Ok(Redirect::to("/settings").into_response());
    }
    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let page = ResetPasswordPage {
        user: None,
        flash,
        csrf_token,
        error: None,
        code: String::new(),
    };
    Ok((jar, Html(page.render()?)).into_response())
}

pub async fn reset_post(
    State(state): State<AppState>,
    jar: CookieJar,
    OptionalUser(user): OptionalUser,
    Form(form): Form<ResetForm>,
) -> AppResult<Response> {
    if user.is_some() {
        return Ok(Redirect::to("/settings").into_response());
    }

    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;
    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    // Held separately so the re-render can echo it without borrowing `form`,
    // whose password fields are moved below.
    let typed_code = form.code.trim().to_string();
    let reject = |jar: CookieJar, csrf_token: String, error: String| -> AppResult<Response> {
        let page = ResetPasswordPage {
            user: None,
            flash: None,
            csrf_token,
            error: Some(error),
            code: typed_code.clone(),
        };
        Ok((jar, Html(page.render()?)).into_response())
    };

    let bad_code = "That code is not valid, has already been used, or has expired. \
                    Ask the admin for a fresh one.";

    let code_key = normalize_reset_code(&form.code);
    if code_key.len() != RESET_CODE_LEN {
        return reject(jar, csrf_token, bad_code.into());
    }
    if let Err(e) = validate_password(&form.new_password) {
        return reject(jar, csrf_token, e);
    }
    if form.new_password != form.new_password_confirm {
        return reject(jar, csrf_token, "New passwords do not match.".into());
    }

    let Some((reset_id, user_id)) =
        crate::db::find_active_password_reset(&state.pool, &code_key).await?
    else {
        return reject(jar, csrf_token, bad_code.into());
    };

    // Spend the code before the slow Argon2 hash, so two submissions racing on
    // one code cannot both get through.
    if !crate::db::consume_password_reset(&state.pool, &reset_id).await? {
        return reject(jar, csrf_token, bad_code.into());
    }

    let password_hash = hash_password_async(form.new_password).await?;
    crate::db::update_user_password(&state.pool, &user_id, &password_hash).await?;
    tracing::info!(user_id = %user_id, "password reset code redeemed");

    let jar = set_flash(jar, "Password changed. Log in with your new password.");
    Ok((jar, Redirect::to("/login")).into_response())
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
