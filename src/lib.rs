//! Bigshot Voter: a LOKI plugin using the typed Rust SDK.
use loki_sdk::{
    Context, Database, Form, Method, QueryResult, Request, Response, Result, Statement, StoredFile,
};
use serde::Deserialize;
use serde_json::{Value, json};
pub mod assets;
pub mod markdown;
pub mod media;
pub mod models;
pub mod templates;
mod voting;

#[derive(Deserialize)]
struct Settings {
    #[serde(rename = "max-upload")]
    max_upload_bytes: u64,
}
fn sql(statement: &str, parameters: Value) -> Result<QueryResult> {
    Database.query(statement, parameters)
}
fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
fn id() -> String {
    loki_sdk::random_id()
}
fn handle(request: Request) -> Result<Response> {
    let context = request.context();
    voting::handle(request.method(), request.path(), context).or_else(|error| {
        let _ = loki_sdk::log(error.message());
        Ok(Response::render(templates::ErrorPage {
            title: "That didn’t work".into(),
            message: if error.status() >= 500 {
                "The application could not complete this request.".into()
            } else {
                error.to_string()
            },
            user: voting::nav(context),
            flash: None,
            csrf_token: context.csrf.clone(),
        })?
        .with_status(error.status()))
    })
}
loki_sdk::export!(handle);
fn html(template: impl askama::Template) -> Result<Response> {
    Response::render(template)
}
fn redirect(path: &str) -> Result<Response> {
    Response::redirect(path)
}
