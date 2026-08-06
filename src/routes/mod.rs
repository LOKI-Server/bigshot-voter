//! HTTP route handlers.

mod admin;
mod auth_routes;
mod pages;
mod panels;
mod settings;

use axum::Router;
use axum::routing::{get, post};

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(pages::home))
        .route("/register", get(auth_routes::register_get).post(auth_routes::register_post))
        .route("/login", get(auth_routes::login_get).post(auth_routes::login_post))
        .route("/logout", post(auth_routes::logout_post))
        .route("/forgot", get(auth_routes::forgot_get).post(auth_routes::forgot_post))
        .route("/reset", get(auth_routes::reset_get).post(auth_routes::reset_post))
        .route("/pending", get(pages::pending))
        .route("/settings", get(settings::settings_get))
        .route("/settings/identity", post(settings::update_identity))
        .route("/settings/password", post(settings::update_password))
        .route("/panels", get(panels::list))
        .route("/panels/{id}", get(panels::detail))
        .route("/panels/{id}/entries", post(panels::upload_entry))
        .route("/panels/{id}/votes", post(panels::cast_vote))
        .route("/media/{entry_id}", get(panels::serve_media))
        .route(
            "/media/{entry_id}/master",
            get(panels::serve_master).post(panels::upload_master),
        )
        .route("/admin", get(admin::dashboard))
        .route(
            "/admin/panels/new",
            get(admin::new_panel_get).post(admin::new_panel_post),
        )
        .route(
            "/admin/panels/{id}/edit",
            get(admin::edit_panel_get).post(admin::edit_panel_post),
        )
        .route("/admin/panels/{id}/close", post(admin::close_panel))
        .route("/admin/panels/{id}/open", post(admin::open_panel))
        .route("/admin/users/{id}/approve", post(admin::approve_user))
        .route("/admin/users/{id}/reject", post(admin::reject_user))
        .route("/admin/resets/{id}/dismiss", post(admin::dismiss_reset))
}
