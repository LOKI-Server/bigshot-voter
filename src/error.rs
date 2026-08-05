//! Application error types mapped to HTTP responses.

use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use thiserror::Error;

use crate::templates::ErrorPage;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    Unauthorized(String),

    #[error("{0}")]
    Forbidden(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Conflict(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),

    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),

    #[error(transparent)]
    Template(#[from] askama::Error),
}

impl AppError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self::Unauthorized(msg.into())
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match &self {
            AppError::Unauthorized(_) => {
                return Redirect::to("/login").into_response();
            }
            AppError::Forbidden(msg) if msg.contains("pending") => {
                return Redirect::to("/pending").into_response();
            }
            _ => {}
        }

        let (status, title, message) = match &self {
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, "Bad request", m.clone()),
            AppError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, "Sign in required", m.clone()),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, "Forbidden", m.clone()),
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, "Not found", m.clone()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, "Conflict", m.clone()),
            AppError::Internal(e) => {
                tracing::error!(error = %e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Server error",
                    "Something went wrong. Please try again.".into(),
                )
            }
            AppError::Sqlx(e) => {
                tracing::error!(error = %e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Server error",
                    "Database error. Please try again.".into(),
                )
            }
            AppError::Template(e) => {
                tracing::error!(error = %e, "template error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Server error",
                    "Template error.".into(),
                )
            }
        };

        let page = ErrorPage {
            title: title.to_string(),
            message,
            user: None,
            flash: None,
            csrf_token: String::new(),
        };

        match page.render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => (status, message_fallback(status, title)).into_response(),
        }
    }
}

fn message_fallback(status: StatusCode, title: &str) -> Html<String> {
    Html(format!(
        "<!DOCTYPE html><html><head><title>{title}</title></head>\
         <body><h1>{title}</h1><p>HTTP {}</p><p><a href=\"/\">Home</a></p></body></html>",
        status.as_u16()
    ))
}

pub type AppResult<T> = Result<T, AppError>;
