//! Application state and router assembly.

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, header};
use axum::routing::get;
use sqlx::SqlitePool;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::{assets, routes};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
}

pub fn build_router(state: AppState) -> Router {
    // Axum defaults to 2 MiB; raise to match the configured upload cap plus
    // multipart overhead (entry form can send display file + master together).
    let max_upload = state.config.max_upload_bytes as usize;
    let body_limit = max_upload
        .saturating_mul(2)
        .saturating_add(1024 * 1024)
        .max(max_upload);

    Router::new()
        .merge(routes::router())
        // CSS and JS are compiled into the binary — nothing to ship alongside it.
        .route("/static/{file}", get(assets::serve))
        .route("/favicon.ico", get(assets::no_favicon))
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(TraceLayer::new_for_http())
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        ))
        .with_state(state)
}
