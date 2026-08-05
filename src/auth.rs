//! Password hashing, signed session cookies, and CSRF tokens.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use time::{Duration, OffsetDateTime};

use crate::error::{AppError, AppResult};
use crate::models::{User, UserStatus};
use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

const SESSION_COOKIE: &str = "bigshot_session";
const CSRF_COOKIE: &str = "bigshot_csrf";
const SESSION_TTL_DAYS: i64 = 14;

// Dummy PHC hash used only for timing equalization when the user is missing.
// Precomputed Argon2id of "dummy-password-check" with a fixed salt is ideal;
// hashing fresh each miss is fine and keeps this module self-contained.
// We verify against a static hash so cost matches a real password check.
const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

// --- Passwords ----------------------------------------------------------------

/// CPU-bound; call from async via [`hash_password_async`].
pub fn hash_password(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut rand::thread_rng());
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("hash password: {e}")))?
        .to_string();
    Ok(hash)
}

/// Runs Argon2 on the blocking pool so the async runtime stays responsive.
pub async fn hash_password_async(password: String) -> AppResult<String> {
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("hash password join: {e}")))?
}

/// CPU-bound; call from async via [`verify_password_async`].
pub fn verify_password(password: &str, password_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(password_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Runs Argon2 verify on the blocking pool.
pub async fn verify_password_async(password: String, password_hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_password(&password, &password_hash))
        .await
        .unwrap_or(false)
}

/// Equalize login timing when the username is unknown (still do Argon2 work).
pub async fn dummy_password_work(password: String) {
    let _ = verify_password_async(password, DUMMY_PASSWORD_HASH.to_string()).await;
}

pub fn validate_username(username: &str) -> Result<(), String> {
    let username = username.trim();
    if username.len() < 3 || username.len() > 32 {
        return Err("Username must be 3–32 characters.".into());
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("Username may only contain letters, numbers, and underscores.".into());
    }
    Ok(())
}

/// Structural check only — we never send mail, so there is nothing to confirm.
/// The goal is to reject input that could not possibly be an address, not to
/// re-implement RFC 5322.
pub fn validate_email(email: &str) -> Result<(), String> {
    if email.len() > 254 {
        return Err("Email address is too long.".into());
    }
    if email.chars().any(char::is_whitespace) || email.matches('@').count() != 1 {
        return Err("Enter a valid email address.".into());
    }
    let Some((local, domain)) = email.split_once('@') else {
        return Err("Enter a valid email address.".into());
    };
    if local.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.starts_with('-')
    {
        return Err("Enter a valid email address.".into());
    }
    Ok(())
}

pub fn validate_password(password: &str) -> Result<(), String> {
    if password.len() < 8 {
        return Err("Password must be at least 8 characters.".into());
    }
    if password.len() > 128 {
        return Err("Password is too long.".into());
    }
    Ok(())
}

// --- Signed session cookie: base64(user_id|exp)|base64(hmac) ------------------

fn sign(secret: &[u8], message: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn verify_sig(secret: &[u8], message: &str, sig: &str) -> bool {
    let Ok(expected_raw) = URL_SAFE_NO_PAD.decode(sig) else {
        return false;
    };
    let mut mac =
        HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    mac.verify_slice(&expected_raw).is_ok()
}

pub fn create_session_cookie(secret: &[u8], user_id: &str) -> Cookie<'static> {
    let exp = OffsetDateTime::now_utc() + Duration::days(SESSION_TTL_DAYS);
    let exp_unix = exp.unix_timestamp();
    let payload = format!("{user_id}|{exp_unix}");
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload.as_bytes());
    let sig = sign(secret, &payload_b64);
    let value = format!("{payload_b64}.{sig}");

    Cookie::build((SESSION_COOKIE, value))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(Duration::days(SESSION_TTL_DAYS))
        .build()
}

pub fn clear_session_cookie() -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, ""))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(Duration::seconds(0))
        .build()
}

pub fn parse_session_user_id(secret: &[u8], jar: &CookieJar) -> Option<String> {
    let cookie = jar.get(SESSION_COOKIE)?;
    let value = cookie.value();
    let (payload_b64, sig) = value.split_once('.')?;
    if !verify_sig(secret, payload_b64, sig) {
        return None;
    }
    let payload = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let payload = String::from_utf8(payload).ok()?;
    let (user_id, exp_str) = payload.split_once('|')?;
    let exp: i64 = exp_str.parse().ok()?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    if now > exp {
        return None;
    }
    if user_id.is_empty() {
        return None;
    }
    Some(user_id.to_string())
}

// --- CSRF ---------------------------------------------------------------------

pub fn ensure_csrf_cookie(jar: CookieJar) -> (CookieJar, String) {
    if let Some(c) = jar.get(CSRF_COOKIE) {
        let token = c.value().to_string();
        if !token.is_empty() {
            return (jar, token);
        }
    }
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let cookie = Cookie::build((CSRF_COOKIE, token.clone()))
        .http_only(false) // readable by form if needed; we embed in HTML
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(Duration::days(SESSION_TTL_DAYS))
        .build();
    (jar.add(cookie), token)
}

pub fn csrf_token_from_jar(jar: &CookieJar) -> Option<String> {
    jar.get(CSRF_COOKIE).map(|c| c.value().to_string())
}

pub fn verify_csrf(jar: &CookieJar, submitted: Option<&str>) -> AppResult<()> {
    let Some(cookie_token) = csrf_token_from_jar(jar) else {
        return Err(AppError::bad_request("Missing CSRF token. Refresh and try again."));
    };
    let Some(submitted) = submitted.filter(|s| !s.is_empty()) else {
        return Err(AppError::bad_request("Missing CSRF token."));
    };
    if !constant_time_eq(cookie_token.as_bytes(), submitted.as_bytes()) {
        return Err(AppError::bad_request("Invalid CSRF token. Refresh and try again."));
    }
    Ok(())
}

#[inline]
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

// --- Extractors ---------------------------------------------------------------

/// Optional logged-in user (any status except we still load them).
pub struct OptionalUser(pub Option<User>);

impl FromRequestParts<AppState> for OptionalUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::bad_request("Invalid cookies"))?;
        let Some(user_id) = parse_session_user_id(&state.config.secret, &jar) else {
            return Ok(OptionalUser(None));
        };
        let user = crate::db::find_user_by_id(&state.pool, &user_id).await?;
        Ok(OptionalUser(user))
    }
}

/// Requires a valid session.
pub struct AuthUser(pub User);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let OptionalUser(user) = OptionalUser::from_request_parts(parts, state).await?;
        let user = user.ok_or_else(|| AppError::unauthorized("Please sign in."))?;
        if user.status == UserStatus::Rejected {
            return Err(AppError::forbidden("Your account was rejected."));
        }
        Ok(AuthUser(user))
    }
}

/// Requires approved (or admin) account.
pub struct ApprovedUser(pub User);

impl FromRequestParts<AppState> for ApprovedUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let AuthUser(user) = AuthUser::from_request_parts(parts, state).await?;
        if user.is_pending() {
            return Err(AppError::forbidden("pending: awaiting approval"));
        }
        if !user.is_approved() {
            return Err(AppError::forbidden("Account not approved."));
        }
        Ok(ApprovedUser(user))
    }
}

/// Requires admin role (the sole overseeing admin).
pub struct AdminUser(pub User);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let ApprovedUser(user) = ApprovedUser::from_request_parts(parts, state).await?;
        if !user.is_admin() {
            return Err(AppError::forbidden("Admin only."));
        }
        Ok(AdminUser(user))
    }
}

/// Flash message helpers via a short-lived cookie.
pub const FLASH_COOKIE: &str = "bigshot_flash";

pub fn set_flash(jar: CookieJar, message: &str) -> CookieJar {
    let encoded = urlencoding::encode(message).into_owned();
    let cookie = Cookie::build((FLASH_COOKIE, encoded))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(Duration::seconds(30))
        .build();
    jar.add(cookie)
}

pub fn take_flash(jar: CookieJar) -> (CookieJar, Option<String>) {
    if let Some(c) = jar.get(FLASH_COOKIE) {
        let msg = urlencoding::decode(c.value())
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| c.value().to_string());
        let clear = Cookie::build((FLASH_COOKIE, ""))
            .http_only(true)
            .same_site(SameSite::Lax)
            .path("/")
            .max_age(Duration::seconds(0))
            .build();
        return (jar.add(clear), Some(msg));
    }
    (jar, None)
}
