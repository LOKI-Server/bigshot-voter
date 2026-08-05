//! End-to-end-ish flow tests against an in-process app + temp data dir.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use bigshot_voter::app::{AppState, build_router};
use bigshot_voter::auth::{hash_password, verify_password};
use bigshot_voter::config::Config;
use bigshot_voter::db;
use bigshot_voter::models::{MediaType, Role, UserStatus};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn test_config(dir: &std::path::Path) -> Config {
    std::fs::create_dir_all(dir.join("uploads")).unwrap();
    let secret = vec![7u8; 32];
    std::fs::write(dir.join("secret.key"), &secret).unwrap();
    Config {
        data_dir: dir.to_path_buf(),
        db_path: dir.join("bigshot.db"),
        uploads_dir: dir.join("uploads"),
        secret,
        bind: "127.0.0.1:0".into(),
        max_upload_bytes: 1024 * 1024,
        config_file: dir.join("bigshot.json"),
    }
}

async fn setup() -> (axum::Router, tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path());
    let pool = db::connect(&config.db_path).await.unwrap();
    let state = AppState {
        pool,
        config: Arc::new(config),
    };
    let app = build_router(state.clone());
    (app, tmp, state)
}

async fn body_text(res: axum::response::Response) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn extract_set_cookie(res: &axum::response::Response, name: &str) -> Option<String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with(&format!("{name}=")))
        .map(|c| c.split(';').next().unwrap().to_string())
}

/// Log in and return `(session_value, csrf_value)` for subsequent requests.
async fn login(app: &axum::Router, username: &str, password: &str) -> (String, String) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = extract_set_cookie(&res, "bigshot_csrf")
        .expect("csrf cookie")
        .split_once('=')
        .unwrap()
        .1
        .to_string();

    let form = format!("username={username}&password={password}&csrf_token={csrf}");
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("bigshot_csrf={csrf}"))
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();

    let session = extract_set_cookie(&res, "bigshot_session")
        .expect("session cookie")
        .split_once('=')
        .unwrap()
        .1
        .to_string();
    let csrf = extract_set_cookie(&res, "bigshot_csrf")
        .map(|c| c.split_once('=').unwrap().1.to_string())
        .unwrap_or(csrf);
    (session, csrf)
}

async fn get_html(app: &axum::Router, uri: &str, session: &str, csrf: &str) -> String {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(
                    header::COOKIE,
                    format!("bigshot_session={session}; bigshot_csrf={csrf}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "GET {uri}");
    body_text(res).await
}

#[tokio::test]
async fn password_hash_roundtrip() {
    let h = hash_password("correct horse battery").unwrap();
    assert!(verify_password("correct horse battery", &h));
    assert!(!verify_password("wrong", &h));
}

#[tokio::test]
async fn first_user_is_admin_second_is_pending() {
    let (_app, _tmp, state) = setup().await;

    assert_eq!(db::user_count(&state.pool).await.unwrap(), 0);

    let admin = db::create_user(
        &state.pool,
        "AdminUser",
        &hash_password("password1").unwrap(),
        Role::Admin,
        UserStatus::Approved,
    )
    .await
    .unwrap();
    assert!(admin.is_admin());
    assert!(admin.is_approved());

    let user = db::create_user(
        &state.pool,
        "voter_one",
        &hash_password("password2").unwrap(),
        Role::User,
        UserStatus::Pending,
    )
    .await
    .unwrap();
    assert!(!user.is_admin());
    assert!(user.is_pending());

    db::set_user_status(&state.pool, &user.id, UserStatus::Approved)
        .await
        .unwrap();
    let user = db::find_user_by_id(&state.pool, &user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(user.is_approved());
}

#[tokio::test]
async fn one_entry_and_vote_change() {
    let (_app, tmp, state) = setup().await;

    let admin = db::create_user(
        &state.pool,
        "admin",
        &hash_password("password1").unwrap(),
        Role::Admin,
        UserStatus::Approved,
    )
    .await
    .unwrap();
    let alice = db::create_user(
        &state.pool,
        "alice",
        &hash_password("password2").unwrap(),
        Role::User,
        UserStatus::Approved,
    )
    .await
    .unwrap();
    let bob = db::create_user(
        &state.pool,
        "bob",
        &hash_password("password3").unwrap(),
        Role::User,
        UserStatus::Approved,
    )
    .await
    .unwrap();

    let panel = db::create_panel(
        &state.pool,
        "Best logo",
        "pick one",
        "",
        MediaType::Image,
        &admin.id,
    )
    .await
    .unwrap();

    // Fake files on disk
    let panel_dir = tmp.path().join("uploads").join(&panel.id);
    std::fs::create_dir_all(&panel_dir).unwrap();
    std::fs::write(panel_dir.join("e1.png"), b"png1").unwrap();
    std::fs::write(panel_dir.join("e2.png"), b"png2").unwrap();

    let _e1 = db::create_entry(
        &state.pool,
        &panel.id,
        &alice.id,
        "Alice art",
        "a.png",
        &format!("{}/e1.png", panel.id),
        "image/png",
    )
    .await
    .unwrap();
    let e2 = db::create_entry(
        &state.pool,
        &panel.id,
        &bob.id,
        "Bob art",
        "b.png",
        &format!("{}/e2.png", panel.id),
        "image/png",
    )
    .await
    .unwrap();

    // Second entry for alice must fail uniqueness
    let dup = db::create_entry(
        &state.pool,
        &panel.id,
        &alice.id,
        "again",
        "c.png",
        &format!("{}/e3.png", panel.id),
        "image/png",
    )
    .await;
    assert!(dup.is_err());

    // Alice votes for bob's entry (not her own)
    db::upsert_vote(&state.pool, &panel.id, &e2, &alice.id)
        .await
        .unwrap();
    assert_eq!(
        db::user_vote_entry_id(&state.pool, &panel.id, &alice.id)
            .await
            .unwrap()
            .as_deref(),
        Some(e2.as_str())
    );

    // Bob votes for alice's entry, then switches to... wait, only two entries;
    // bob can only vote for alice's, so add carol and change alice's vote.
    let carol = db::create_user(
        &state.pool,
        "carol",
        &hash_password("password4").unwrap(),
        Role::User,
        UserStatus::Approved,
    )
    .await
    .unwrap();
    std::fs::write(panel_dir.join("e3.png"), b"png3").unwrap();
    let e3 = db::create_entry(
        &state.pool,
        &panel.id,
        &carol.id,
        "Carol art",
        "c.png",
        &format!("{}/e3.png", panel.id),
        "image/png",
    )
    .await
    .unwrap();

    // Change alice's vote from bob to carol
    db::upsert_vote(&state.pool, &panel.id, &e3, &alice.id)
        .await
        .unwrap();
    assert_eq!(
        db::user_vote_entry_id(&state.pool, &panel.id, &alice.id)
            .await
            .unwrap()
            .as_deref(),
        Some(e3.as_str())
    );

    let entries = db::list_entries_for_panel(&state.pool, &panel.id)
        .await
        .unwrap();
    let carol_entry = entries.iter().find(|e| e.id == e3).unwrap();
    assert_eq!(carol_entry.vote_count, 1);
    let bob_entry = entries.iter().find(|e| e.id == e2).unwrap();
    assert_eq!(bob_entry.vote_count, 0);
}

#[tokio::test]
async fn cannot_vote_for_own_entry_via_http() {
    let (app, tmp, state) = setup().await;

    let admin = db::create_user(
        &state.pool,
        "admin",
        &hash_password("password1").unwrap(),
        Role::Admin,
        UserStatus::Approved,
    )
    .await
    .unwrap();
    let alice = db::create_user(
        &state.pool,
        "alice",
        &hash_password("password2").unwrap(),
        Role::User,
        UserStatus::Approved,
    )
    .await
    .unwrap();
    let bob = db::create_user(
        &state.pool,
        "bob",
        &hash_password("password3").unwrap(),
        Role::User,
        UserStatus::Approved,
    )
    .await
    .unwrap();

    let panel = db::create_panel(
        &state.pool,
        "Best logo",
        "pick one",
        "",
        MediaType::Image,
        &admin.id,
    )
    .await
    .unwrap();

    let panel_dir = tmp.path().join("uploads").join(&panel.id);
    std::fs::create_dir_all(&panel_dir).unwrap();
    std::fs::write(panel_dir.join("e1.png"), b"png1").unwrap();
    std::fs::write(panel_dir.join("e2.png"), b"png2").unwrap();

    let e1 = db::create_entry(
        &state.pool,
        &panel.id,
        &alice.id,
        "Alice art",
        "a.png",
        &format!("{}/e1.png", panel.id),
        "image/png",
    )
    .await
    .unwrap();
    let _e2 = db::create_entry(
        &state.pool,
        &panel.id,
        &bob.id,
        "Bob art",
        "b.png",
        &format!("{}/e2.png", panel.id),
        "image/png",
    )
    .await
    .unwrap();

    // Log in as alice: GET /login for CSRF, POST /login for session
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csrf = extract_set_cookie(&res, "bigshot_csrf")
        .expect("csrf cookie")
        .split_once('=')
        .unwrap()
        .1
        .to_string();

    let form = format!("username=alice&password=password2&csrf_token={csrf}");
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("bigshot_csrf={csrf}"))
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    let session = extract_set_cookie(&res, "bigshot_session").expect("session cookie");
    let csrf_after = extract_set_cookie(&res, "bigshot_csrf")
        .map(|c| c.split_once('=').unwrap().1.to_string())
        .unwrap_or(csrf);
    // Cookie header may need full cookie pair strings
    let session_val = session.split_once('=').unwrap().1;

    // Refresh CSRF from panel page while logged in
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/panels/{}", panel.id))
                .header(
                    header::COOKIE,
                    format!("bigshot_session={session_val}; bigshot_csrf={csrf_after}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let csrf = extract_set_cookie(&res, "bigshot_csrf")
        .map(|c| c.split_once('=').unwrap().1.to_string())
        .unwrap_or(csrf_after);
    let body = body_text(res).await;
    assert!(
        body.contains("You cannot vote for your own entry"),
        "own entry should not be votable in UI"
    );

    // Attempt to vote for own entry via POST
    let form = format!("entry_id={e1}&csrf_token={csrf}");
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/panels/{}/votes", panel.id))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(
                    header::COOKIE,
                    format!("bigshot_session={session_val}; bigshot_csrf={csrf}"),
                )
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = res.status();
    let body = body_text(res).await;
    assert!(
        status == StatusCode::BAD_REQUEST
            || body.contains("cannot vote for your own")
            || body.contains("own entry"),
        "expected rejection of self-vote, got {status}: {body}"
    );
    assert!(
        db::user_vote_entry_id(&state.pool, &panel.id, &alice.id)
            .await
            .unwrap()
            .is_none(),
        "self-vote must not be stored"
    );
}

#[tokio::test]
async fn email_is_an_alternative_login_identifier() {
    let (_app, _tmp, state) = setup().await;

    let user = db::create_user(
        &state.pool,
        "caitlin",
        &hash_password("password123").unwrap(),
        Role::User,
        UserStatus::Approved,
    )
    .await
    .unwrap();
    assert!(user.email.is_none());

    // Username still resolves before an email exists.
    assert!(
        db::find_user_by_login(&state.pool, "caitlin")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        db::find_user_by_login(&state.pool, "caitlin@example.com")
            .await
            .unwrap()
            .is_none()
    );

    db::update_user_identity(
        &state.pool,
        &user.id,
        "caitlin",
        Some("Caitlin.M@Example.COM"),
    )
    .await
    .unwrap();

    // Stored verbatim for display, matched case-insensitively for sign-in.
    let reloaded = db::find_user_by_id(&state.pool, &user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.email.as_deref(), Some("Caitlin.M@Example.COM"));

    for identifier in ["caitlin", "caitlin.m@example.com", "CAITLIN.M@EXAMPLE.COM"] {
        let found = db::find_user_by_login(&state.pool, identifier)
            .await
            .unwrap();
        assert_eq!(
            found.map(|u| u.id).as_deref(),
            Some(user.id.as_str()),
            "{identifier} should resolve to the account"
        );
    }

    // Clearing the email drops that sign-in route but keeps the username.
    db::update_user_identity(&state.pool, &user.id, "caitlin", None)
        .await
        .unwrap();
    assert!(
        db::find_user_by_login(&state.pool, "caitlin.m@example.com")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db::find_user_by_login(&state.pool, "caitlin")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn emails_are_unique_but_absent_emails_are_not() {
    let (_app, _tmp, state) = setup().await;

    let a = db::create_user(
        &state.pool,
        "alice",
        &hash_password("password123").unwrap(),
        Role::User,
        UserStatus::Approved,
    )
    .await
    .unwrap();
    let b = db::create_user(
        &state.pool,
        "bob",
        &hash_password("password123").unwrap(),
        Role::User,
        UserStatus::Approved,
    )
    .await
    .unwrap();

    // Two accounts without an email must not collide on NULL.
    db::update_user_identity(&state.pool, &a.id, "alice", None)
        .await
        .unwrap();
    db::update_user_identity(&state.pool, &b.id, "bob", None)
        .await
        .unwrap();

    db::update_user_identity(&state.pool, &a.id, "alice", Some("shared@example.com"))
        .await
        .unwrap();
    let clash =
        db::update_user_identity(&state.pool, &b.id, "bob", Some("SHARED@example.com")).await;
    assert!(clash.is_err(), "email must be unique across accounts");
}

#[tokio::test]
async fn password_change_invalidates_the_old_password() {
    let (_app, _tmp, state) = setup().await;

    let user = db::create_user(
        &state.pool,
        "riley",
        &hash_password("password123").unwrap(),
        Role::User,
        UserStatus::Approved,
    )
    .await
    .unwrap();

    let new_hash = hash_password("a-much-better-password").unwrap();
    db::update_user_password(&state.pool, &user.id, &new_hash)
        .await
        .unwrap();

    let reloaded = db::find_user_by_id(&state.pool, &user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!verify_password("password123", &reloaded.password_hash));
    assert!(verify_password(
        "a-much-better-password",
        &reloaded.password_hash
    ));
}

/// Databases created before optional sign-in emails must gain the columns
/// without losing their rows.
#[tokio::test]
async fn migration_adds_email_columns_to_a_legacy_database() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("legacy.db");

    {
        let url = format!("sqlite:{}?mode=rwc", db_path.display());
        let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
        sqlx::query(
            r#"
            CREATE TABLE users (
                id TEXT PRIMARY KEY NOT NULL,
                username TEXT NOT NULL COLLATE NOCASE,
                username_key TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL CHECK (role IN ('admin', 'user')),
                status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'rejected')),
                created_at TEXT NOT NULL
            );
            INSERT INTO users VALUES
                ('u1', 'Legacy', 'legacy', 'hash', 'admin', 'approved', '2024-01-01T00:00:00Z');
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    // Opening through the app runs the migration.
    let pool = db::connect(&db_path).await.unwrap();

    let user = db::find_user_by_id(&pool, "u1").await.unwrap().unwrap();
    assert_eq!(user.username, "Legacy");
    assert!(user.email.is_none(), "existing rows get a NULL email");

    db::update_user_identity(&pool, "u1", "Legacy", Some("legacy@example.com"))
        .await
        .unwrap();
    assert!(
        db::find_user_by_login(&pool, "legacy@example.com")
            .await
            .unwrap()
            .is_some()
    );

    // Re-running the migration on an already-updated database is a no-op.
    pool.close().await;
    let pool = db::connect(&db_path).await.unwrap();
    let user = db::find_user_by_id(&pool, "u1").await.unwrap().unwrap();
    assert_eq!(user.email.as_deref(), Some("legacy@example.com"));
}

#[tokio::test]
async fn panel_info_can_be_edited_and_description_renders_markdown() {
    let (app, _tmp, state) = setup().await;

    let admin = db::create_user(
        &state.pool,
        "admin",
        &hash_password("password1").unwrap(),
        Role::Admin,
        UserStatus::Approved,
    )
    .await
    .unwrap();

    let panel = db::create_panel(
        &state.pool,
        "Original title",
        "plain description",
        "",
        MediaType::Image,
        &admin.id,
    )
    .await
    .unwrap();
    assert_eq!(panel.link, "");

    // Edit title, Markdown description, and related link.
    db::update_panel(
        &state.pool,
        &panel.id,
        "JellyWin32 Icon",
        "Design an **icon**. See [the repo](https://github.com/Aspenini/jellywin32).\n\n\
         - square preferred\n- transparent PNG\n\n\
         <script>alert('xss')</script>",
        "https://github.com/Aspenini/jellywin32",
    )
    .await
    .unwrap();

    let updated = db::find_panel(&state.pool, &panel.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.title, "JellyWin32 Icon");
    assert_eq!(updated.link, "https://github.com/Aspenini/jellywin32");
    // The raw Markdown is stored verbatim; rendering happens at display time.
    assert!(updated.description.contains("**icon**"));

    // Sign in as admin and load the panel page.
    let (session, csrf) = login(&app, "admin", "password1").await;
    let html = get_html(&app, &format!("/panels/{}", panel.id), &session, &csrf).await;

    assert!(html.contains("<strong>icon</strong>"), "markdown not rendered");
    assert!(html.contains("<li>square preferred</li>"), "list not rendered");
    assert!(
        html.contains(r#"href="https://github.com/Aspenini/jellywin32""#),
        "markdown link not rendered"
    );
    // Raw HTML in the description must never reach the page.
    assert!(!html.contains("<script>alert"), "raw HTML leaked: {html}");
    assert!(!html.contains("alert('xss')"), "script body leaked");
    // Related link renders without its scheme, and admins see the edit action.
    assert!(html.contains("github.com/Aspenini/jellywin32"));
    assert!(html.contains(&format!("/admin/panels/{}/edit", panel.id)));

    // Card excerpts flatten Markdown rather than showing its syntax.
    let list = get_html(&app, "/panels", &session, &csrf).await;
    assert!(list.contains("Design an icon"), "excerpt not flattened: {list}");
    assert!(!list.contains("**icon**"), "raw markdown leaked into card");
}

#[tokio::test]
async fn only_admins_can_edit_a_panel() {
    let (app, _tmp, state) = setup().await;

    let admin = db::create_user(
        &state.pool,
        "admin",
        &hash_password("password1").unwrap(),
        Role::Admin,
        UserStatus::Approved,
    )
    .await
    .unwrap();
    db::create_user(
        &state.pool,
        "member",
        &hash_password("password2").unwrap(),
        Role::User,
        UserStatus::Approved,
    )
    .await
    .unwrap();

    let panel = db::create_panel(
        &state.pool,
        "Panel",
        "",
        "",
        MediaType::Image,
        &admin.id,
    )
    .await
    .unwrap();

    let (session, csrf) = login(&app, "member", "password2").await;

    // The edit affordance is hidden from non-admins...
    let html = get_html(&app, &format!("/panels/{}", panel.id), &session, &csrf).await;
    assert!(!html.contains("Edit panel"), "member sees admin action");

    // ...and the route itself rejects them.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/admin/panels/{}/edit", panel.id))
                .header(
                    header::COOKIE,
                    format!("bigshot_session={session}; bigshot_csrf={csrf}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/panels/{}/edit", panel.id))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(
                    header::COOKIE,
                    format!("bigshot_session={session}; bigshot_csrf={csrf}"),
                )
                .body(Body::from(format!("title=Hacked&csrf_token={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    let unchanged = db::find_panel(&state.pool, &panel.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.title, "Panel");
}

#[tokio::test]
async fn embedded_assets_are_served_from_the_binary() {
    let (app, _tmp, _state) = setup().await;

    // The page must reference the content-hashed URL, not a bare file path.
    let res = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let html = body_text(res).await;
    let css_href = bigshot_voter::assets::css_href();
    assert!(
        html.contains(css_href),
        "home page should link {css_href}, got: {html}"
    );

    let res = app
        .clone()
        .oneshot(Request::builder().uri(css_href).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let etag = res
        .headers()
        .get(header::ETAG)
        .expect("asset carries an ETag")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        res.headers()
            .get(header::CACHE_CONTROL)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    assert!(body_text(res).await.contains(".tile-grid"));

    // Conditional request revalidates instead of resending the body.
    let res = app
        .oneshot(
            Request::builder()
                .uri(css_href)
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn home_page_renders() {
    let (app, _tmp, _state) = setup().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_text(res).await;
    assert!(body.contains("Big-Shot Voting"));
}

#[tokio::test]
async fn register_first_user_via_http() {
    let (app, _tmp, state) = setup().await;

    // GET register to establish CSRF cookie
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/register")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let csrf = extract_set_cookie(&res, "bigshot_csrf")
        .expect("csrf cookie")
        .split_once('=')
        .unwrap()
        .1
        .to_string();
    let body = body_text(res).await;
    assert!(body.contains("overseeing admin") || body.contains("Overseeing") || body.contains("admin"));

    let form = format!(
        "username=FirstAdmin&password=password123&password_confirm=password123&csrf_token={csrf}"
    );

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("bigshot_csrf={csrf}"))
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = res.status();
    if status != StatusCode::SEE_OTHER && status != StatusCode::FOUND {
        let body = body_text(res).await;
        panic!("expected redirect, got {status}: {body}");
    }

    let user = db::find_user_by_username(&state.pool, "FirstAdmin")
        .await
        .unwrap()
        .expect("user created");
    assert!(user.is_admin());
    assert!(user.is_approved());
}
