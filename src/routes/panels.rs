use askama::Template;
use axum::body::Body;
use axum::extract::{Multipart, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum_extra::extract::{CookieJar, Form};
use mime::Mime;
use serde::Deserialize;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::AppState;
use crate::auth::{ApprovedUser, ensure_csrf_cookie, set_flash, take_flash, verify_csrf};
use crate::error::{AppError, AppResult};
use crate::media::{
    master_accept_attr, resolve_upload_path, validate_master_upload, validate_upload,
};
use crate::models::format_bytes;
use crate::routes::pages::panel_summary;
use crate::templates::{EntryView, NavUser, PanelDetailPage, PanelView, PanelsListPage};

#[derive(Debug, Deserialize)]
pub struct VoteForm {
    pub entry_id: String,
    pub csrf_token: String,
}

pub async fn list(
    State(state): State<AppState>,
    jar: CookieJar,
    ApprovedUser(user): ApprovedUser,
) -> AppResult<Response> {
    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let panels = crate::db::list_panel_summaries(&state.pool)
        .await?
        .iter()
        .map(panel_summary)
        .collect();
    let page = PanelsListPage {
        user: Some(NavUser::from(&user)),
        flash,
        csrf_token,
        panels,
    };
    Ok((jar, Html(page.render()?)).into_response())
}

pub async fn detail(
    State(state): State<AppState>,
    jar: CookieJar,
    ApprovedUser(user): ApprovedUser,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let (jar, flash) = take_flash(jar);
    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let panel = crate::db::find_panel(&state.pool, &id)
        .await?
        .ok_or_else(|| AppError::not_found("Panel not found."))?;

    // Parallel reads — independent queries for the panel view.
    let (entries, my_entry_id, my_vote_entry_id) = tokio::try_join!(
        crate::db::list_entries_for_panel(&state.pool, &panel.id),
        crate::db::user_entry_for_panel(&state.pool, &panel.id, &user.id),
        crate::db::user_vote_entry_id(&state.pool, &panel.id, &user.id),
    )?;

    let entry_views = EntryView::build(
        &entries,
        Some(user.id.as_str()),
        my_vote_entry_id.as_deref(),
        panel.is_open(),
    );

    let total_votes = entries.iter().map(|e| e.vote_count).sum();
    let can_upload = panel.is_open() && my_entry_id.is_none();
    let can_vote = panel.is_open() && !entries.is_empty();

    let page = PanelDetailPage {
        user: Some(NavUser::from(&user)),
        flash,
        csrf_token,
        panel: PanelView::from(&panel),
        entries: entry_views,
        my_entry_id,
        my_vote_entry_id,
        can_upload,
        can_vote,
        total_votes,
        max_upload_label: format_bytes(state.config.max_upload_bytes),
        master_accept: master_accept_attr(),
        is_admin: user.is_admin(),
    };
    Ok((jar, Html(page.render()?)).into_response())
}

/// User-facing message when an upload exceeds the configured size cap.
fn file_too_large_error(max_bytes: u64) -> AppError {
    AppError::bad_request(format!(
        "This file is too big. The limit is {}.",
        format_bytes(max_bytes)
    ))
}

/// Map multipart stream errors, translating body-length failures into a clear size message.
fn map_multipart_error(
    err: axum::extract::multipart::MultipartError,
    max_bytes: u64,
) -> AppError {
    if multipart_error_is_size_limit(&err) {
        return file_too_large_error(max_bytes);
    }
    AppError::bad_request(format!("Invalid upload: {err}"))
}

fn multipart_error_is_size_limit(err: &axum::extract::multipart::MultipartError) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        let s = e.to_string().to_ascii_lowercase();
        if s.contains("length limit")
            || s.contains("payload too large")
            || s.contains("body too large")
            || s.contains("content too long")
            || s.contains("body length limit")
        {
            return true;
        }
        cur = e.source();
    }
    false
}

/// Stream a multipart file field to disk under `uploads/{panel_id}/…`.
async fn stream_field_to_disk(
    field: &mut axum::extract::multipart::Field<'_>,
    uploads_dir: &std::path::Path,
    panel_id: &str,
    extension: &str,
    max_bytes: u64,
    empty_label: &str,
) -> AppResult<(String, std::path::PathBuf)> {
    let file_id = Uuid::new_v4().to_string();
    let rel_path = format!("{panel_id}/{file_id}.{extension}");
    let panel_dir = uploads_dir.join(panel_id);
    fs::create_dir_all(&panel_dir)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("mkdir uploads: {e}")))?;

    let abs = resolve_upload_path(uploads_dir, &rel_path).map_err(AppError::bad_request)?;

    let mut f = fs::File::create(&abs)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("create file: {e}")))?;

    let mut written: u64 = 0;
    loop {
        let chunk = field
            .chunk()
            .await
            .map_err(|e| map_multipart_error(e, max_bytes))?;
        let Some(chunk) = chunk else { break };
        written = written.saturating_add(chunk.len() as u64);
        if written > max_bytes {
            drop(f);
            let _ = fs::remove_file(&abs).await;
            return Err(file_too_large_error(max_bytes));
        }
        f.write_all(&chunk)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!("write file: {e}")))?;
    }
    f.flush()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("flush file: {e}")))?;

    if written == 0 {
        let _ = fs::remove_file(&abs).await;
        return Err(AppError::bad_request(format!("Empty {empty_label}.")));
    }

    Ok((rel_path, abs))
}

pub async fn upload_entry(
    State(state): State<AppState>,
    jar: CookieJar,
    ApprovedUser(user): ApprovedUser,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> AppResult<Response> {
    let panel = crate::db::find_panel(&state.pool, &id)
        .await?
        .ok_or_else(|| AppError::not_found("Panel not found."))?;

    if !panel.is_open() {
        return Err(AppError::bad_request("This panel is closed."));
    }

    if crate::db::user_entry_for_panel(&state.pool, &panel.id, &user.id)
        .await?
        .is_some()
    {
        return Err(AppError::conflict("You already uploaded an entry to this panel."));
    }

    let mut csrf_token: Option<String> = None;
    let mut title: Option<String> = None;
    let mut file_name: Option<String> = None;
    let mut stored_rel: Option<String> = None;
    let mut stored_abs: Option<std::path::PathBuf> = None;
    let mut validated_ct: Option<String> = None;
    let mut master_file_name: Option<String> = None;
    let mut master_rel: Option<String> = None;
    let mut master_abs: Option<std::path::PathBuf> = None;

    let max_bytes = state.config.max_upload_bytes;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| map_multipart_error(e, max_bytes))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "csrf_token" => {
                csrf_token = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| map_multipart_error(e, max_bytes))?,
                );
            }
            "title" => {
                title = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| map_multipart_error(e, max_bytes))?,
                );
            }
            "file" => {
                file_name = field.file_name().map(|s| s.to_string());
                let declared_ct = field.content_type().map(|s| s.to_string());

                let original_filename = file_name
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| AppError::bad_request("Missing file."))?;

                let validated = validate_upload(
                    panel.media_type,
                    original_filename,
                    declared_ct.as_deref(),
                )
                .map_err(AppError::bad_request)?;

                let (rel_path, abs) = stream_field_to_disk(
                    &mut field,
                    &state.config.uploads_dir,
                    &panel.id,
                    &validated.extension,
                    max_bytes,
                    "file",
                )
                .await?;

                stored_rel = Some(rel_path);
                stored_abs = Some(abs);
                validated_ct = Some(validated.content_type);
            }
            "master" => {
                // Optional: browsers still send the field with an empty filename when unused.
                let name = field.file_name().map(|s| s.to_string()).filter(|s| !s.is_empty());
                let Some(original_filename) = name else {
                    // Drain empty optional field and continue.
                    while field
                        .chunk()
                        .await
                        .map_err(|e| map_multipart_error(e, max_bytes))?
                        .is_some()
                    {}
                    continue;
                };

                let validated =
                    validate_master_upload(&original_filename).map_err(AppError::bad_request)?;

                let (rel_path, abs) = stream_field_to_disk(
                    &mut field,
                    &state.config.uploads_dir,
                    &panel.id,
                    &validated.extension,
                    max_bytes,
                    "master file",
                )
                .await?;

                master_file_name = Some(original_filename);
                master_rel = Some(rel_path);
                master_abs = Some(abs);
            }
            _ => {}
        }
    }

    verify_csrf(&jar, csrf_token.as_deref())?;

    let original_filename = file_name
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("Missing file."))?;
    let rel_path = stored_rel.ok_or_else(|| AppError::bad_request("Missing file."))?;
    let abs = stored_abs.ok_or_else(|| AppError::bad_request("Missing file."))?;
    let content_type = validated_ct.ok_or_else(|| AppError::bad_request("Missing file."))?;

    let entry_title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            std::path::Path::new(&original_filename)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Entry")
                .chars()
                .take(120)
                .collect()
        });

    let cleanup = || async {
        let _ = fs::remove_file(&abs).await;
        if let Some(ref m) = master_abs {
            let _ = fs::remove_file(m).await;
        }
    };

    match crate::db::create_entry(
        &state.pool,
        &panel.id,
        &user.id,
        &entry_title,
        &original_filename,
        &rel_path,
        &content_type,
        master_file_name.as_deref(),
        master_rel.as_deref(),
    )
    .await
    {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            cleanup().await;
            return Err(AppError::conflict(
                "You already uploaded an entry to this panel.",
            ));
        }
        Err(e) => {
            cleanup().await;
            return Err(e.into());
        }
    }

    let jar = set_flash(jar, "Entry uploaded.");
    Ok((jar, Redirect::to(&format!("/panels/{}", panel.id))).into_response())
}

pub async fn cast_vote(
    State(state): State<AppState>,
    jar: CookieJar,
    ApprovedUser(user): ApprovedUser,
    Path(id): Path<String>,
    Form(form): Form<VoteForm>,
) -> AppResult<Response> {
    verify_csrf(&jar, Some(form.csrf_token.as_str()))?;

    let panel = crate::db::find_panel(&state.pool, &id)
        .await?
        .ok_or_else(|| AppError::not_found("Panel not found."))?;

    if !panel.is_open() {
        return Err(AppError::bad_request(
            "This panel is closed; votes cannot be changed.",
        ));
    }

    // Single indexed lookup — no full Entry + vote_count join needed for voting.
    let owner_id = crate::db::entry_vote_target(&state.pool, &form.entry_id, &panel.id)
        .await?
        .ok_or_else(|| AppError::bad_request("Invalid entry for this panel."))?;

    if owner_id == user.id {
        return Err(AppError::bad_request(
            "You cannot vote for your own entry.",
        ));
    }

    crate::db::upsert_vote(&state.pool, &panel.id, &form.entry_id, &user.id).await?;

    let jar = set_flash(jar, "Vote recorded. You can change it until the panel closes.");
    Ok((jar, Redirect::to(&format!("/panels/{}", panel.id))).into_response())
}

pub async fn serve_media(
    State(state): State<AppState>,
    ApprovedUser(_user): ApprovedUser,
    Path(entry_id): Path<String>,
    request: Request,
) -> AppResult<Response> {
    let (stored_path, content_type, original_filename) =
        crate::db::find_entry_media(&state.pool, &entry_id)
            .await?
            .ok_or_else(|| AppError::not_found("Entry not found."))?;

    let path = resolve_upload_path(&state.config.uploads_dir, &stored_path)
        .map_err(|_| AppError::not_found("File not found."))?;

    let mime = content_type
        .parse::<Mime>()
        .unwrap_or(mime::APPLICATION_OCTET_STREAM);

    // ServeFile streams from disk *and* honours Range requests, so browsers can
    // seek within long video/audio entries instead of refetching from byte 0.
    let mut res = ServeFile::new_with_mime(&path, &mime)
        .oneshot(request)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("serve media: {e}")))?
        .map(Body::new);

    if res.status() == StatusCode::NOT_FOUND {
        return Err(AppError::not_found("File not found."));
    }

    // Media is only ever served to approved members, so keep it out of shared caches.
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );

    let disposition = if content_type.starts_with("image/")
        || content_type.starts_with("audio/")
        || content_type.starts_with("video/")
    {
        "inline"
    } else {
        "attachment"
    };
    let filename = original_filename.replace(['"', '\r', '\n', '\\'], "");
    if let Ok(val) = format!("{disposition}; filename=\"{filename}\"").parse() {
        res.headers_mut().insert(header::CONTENT_DISPOSITION, val);
    }

    Ok(res)
}

/// Download the optional master/source file for an entry (always as attachment).
pub async fn serve_master(
    State(state): State<AppState>,
    ApprovedUser(_user): ApprovedUser,
    Path(entry_id): Path<String>,
    request: Request,
) -> AppResult<Response> {
    let (stored_path, original_filename) = crate::db::find_entry_master(&state.pool, &entry_id)
        .await?
        .ok_or_else(|| AppError::not_found("Master file not found."))?;

    let path = resolve_upload_path(&state.config.uploads_dir, &stored_path)
        .map_err(|_| AppError::not_found("Master file not found."))?;

    let mut res = ServeFile::new_with_mime(&path, &mime::APPLICATION_OCTET_STREAM)
        .oneshot(request)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("serve master: {e}")))?
        .map(Body::new);

    if res.status() == StatusCode::NOT_FOUND {
        return Err(AppError::not_found("Master file not found."));
    }

    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );

    let filename = original_filename.replace(['"', '\r', '\n', '\\'], "");
    if let Ok(val) = format!("attachment; filename=\"{filename}\"").parse() {
        res.headers_mut().insert(header::CONTENT_DISPOSITION, val);
    }

    Ok(res)
}

/// Owner-only: attach a master file to an entry that was uploaded without one.
pub async fn upload_master(
    State(state): State<AppState>,
    jar: CookieJar,
    ApprovedUser(user): ApprovedUser,
    Path(entry_id): Path<String>,
    mut multipart: Multipart,
) -> AppResult<Response> {
    let (panel_id, owner_id, has_master) =
        crate::db::find_entry_master_owner(&state.pool, &entry_id)
            .await?
            .ok_or_else(|| AppError::not_found("Entry not found."))?;

    if owner_id != user.id {
        return Err(AppError::forbidden("Only the uploader can add a master file."));
    }
    if has_master {
        return Err(AppError::conflict("This entry already has a master file."));
    }

    let mut csrf_token: Option<String> = None;
    let mut master_file_name: Option<String> = None;
    let mut master_rel: Option<String> = None;
    let mut master_abs: Option<std::path::PathBuf> = None;
    let max_bytes = state.config.max_upload_bytes;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| map_multipart_error(e, max_bytes))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "csrf_token" => {
                csrf_token = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| map_multipart_error(e, max_bytes))?,
                );
            }
            "master" => {
                let original = field
                    .file_name()
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| AppError::bad_request("Missing master file."))?;

                let validated =
                    validate_master_upload(&original).map_err(AppError::bad_request)?;

                let (rel_path, abs) = stream_field_to_disk(
                    &mut field,
                    &state.config.uploads_dir,
                    &panel_id,
                    &validated.extension,
                    max_bytes,
                    "master file",
                )
                .await?;

                master_file_name = Some(original);
                master_rel = Some(rel_path);
                master_abs = Some(abs);
            }
            _ => {}
        }
    }

    verify_csrf(&jar, csrf_token.as_deref())?;

    let original_filename = master_file_name
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("Missing master file."))?;
    let rel_path = master_rel.ok_or_else(|| AppError::bad_request("Missing master file."))?;
    let abs = master_abs.ok_or_else(|| AppError::bad_request("Missing master file."))?;

    let attached = match crate::db::attach_entry_master(
        &state.pool,
        &entry_id,
        &user.id,
        &original_filename,
        &rel_path,
    )
    .await
    {
        Ok(ok) => ok,
        Err(e) => {
            let _ = fs::remove_file(&abs).await;
            return Err(e.into());
        }
    };

    if !attached {
        let _ = fs::remove_file(&abs).await;
        return Err(AppError::conflict("This entry already has a master file."));
    }

    let jar = set_flash(jar, "Master file uploaded.");
    Ok((jar, Redirect::to(&format!("/panels/{panel_id}"))).into_response())
}
