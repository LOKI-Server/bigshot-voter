//! SQLite access layer.

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

use crate::models::{
    Entry, MediaType, Panel, PanelListing, PanelStatus, PendingUser, Role, User, UserStatus,
};

pub async fn connect(db_path: &std::path::Path) -> Result<SqlitePool> {
    let url = format!("sqlite:{}?mode=rwc", db_path.display());
    let options = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        // Wait out brief writer contention under WAL instead of failing fast.
        .busy_timeout(Duration::from_secs(5))
        // ~20 MiB page cache (negative = KiB units in SQLite).
        .pragma("cache_size", "-20000")
        .pragma("temp_store", "MEMORY")
        .pragma("mmap_size", "134217728");

    // SQLite is single-writer; a small pool is enough and avoids oversubscription.
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await
        .with_context(|| format!("connect sqlite {}", db_path.display()))?;

    migrate(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY NOT NULL,
            username TEXT NOT NULL COLLATE NOCASE,
            username_key TEXT NOT NULL UNIQUE,
            email TEXT,
            email_key TEXT,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL CHECK (role IN ('admin', 'user')),
            status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'rejected')),
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS panels (
            id TEXT PRIMARY KEY NOT NULL,
            title TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            link TEXT NOT NULL DEFAULT '',
            media_type TEXT NOT NULL CHECK (media_type IN ('image', 'audio', 'video', 'other')),
            status TEXT NOT NULL CHECK (status IN ('open', 'closed')),
            created_by TEXT NOT NULL REFERENCES users(id),
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS entries (
            id TEXT PRIMARY KEY NOT NULL,
            panel_id TEXT NOT NULL REFERENCES panels(id) ON DELETE CASCADE,
            user_id TEXT NOT NULL REFERENCES users(id),
            title TEXT NOT NULL,
            original_filename TEXT NOT NULL,
            stored_path TEXT NOT NULL,
            content_type TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE (panel_id, user_id)
        );

        CREATE TABLE IF NOT EXISTS votes (
            id TEXT PRIMARY KEY NOT NULL,
            panel_id TEXT NOT NULL REFERENCES panels(id) ON DELETE CASCADE,
            entry_id TEXT NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
            user_id TEXT NOT NULL REFERENCES users(id),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE (panel_id, user_id)
        );

        CREATE INDEX IF NOT EXISTS idx_entries_panel ON entries(panel_id);
        CREATE INDEX IF NOT EXISTS idx_votes_panel ON votes(panel_id);
        CREATE INDEX IF NOT EXISTS idx_votes_entry ON votes(entry_id);
        CREATE INDEX IF NOT EXISTS idx_users_status ON users(status);
        "#,
    )
    .execute(pool)
    .await?;

    // `CREATE TABLE IF NOT EXISTS` leaves databases created before optional
    // sign-in emails untouched, so add the columns explicitly when missing.
    add_column_if_missing(pool, "users", "email", "email TEXT").await?;
    add_column_if_missing(pool, "users", "email_key", "email_key TEXT").await?;
    add_column_if_missing(pool, "panels", "link", "link TEXT NOT NULL DEFAULT ''").await?;

    // Partial index: unique across the accounts that set an email, while any
    // number of accounts may leave it NULL.
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email_key
         ON users(email_key) WHERE email_key IS NOT NULL",
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Idempotent `ALTER TABLE … ADD COLUMN` (SQLite has no `IF NOT EXISTS` here).
/// All arguments are compile-time constants, never user input.
async fn add_column_if_missing(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let existing = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await?;
    let present = existing
        .iter()
        .any(|r| r.get::<String, _>("name") == column);
    if !present {
        sqlx::query(&format!("ALTER TABLE {table} ADD COLUMN {definition}"))
            .execute(pool)
            .await?;
    }
    Ok(())
}

#[inline]
fn now_rfc3339() -> String {
    // time crate RFC3339 without pulling chrono
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"))
}

fn map_user(row: &sqlx::sqlite::SqliteRow) -> Result<User, sqlx::Error> {
    let role = Role::parse(row.get("role")).ok_or_else(|| {
        sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad role",
        )))
    })?;
    let status = UserStatus::parse(row.get("status")).ok_or_else(|| {
        sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad status",
        )))
    })?;
    Ok(User {
        id: row.get("id"),
        username: row.get("username"),
        email: row.get("email"),
        password_hash: row.get("password_hash"),
        role,
        status,
        created_at: row.get("created_at"),
    })
}

/// Every column [`map_user`] expects.
const USER_COLUMNS: &str = "id, username, email, password_hash, role, status, created_at";

pub async fn user_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS c FROM users")
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("c"))
}

pub async fn find_user_by_id(pool: &SqlitePool, id: &str) -> Result<Option<User>, sqlx::Error> {
    let row = sqlx::query(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.map(|r| map_user(&r)).transpose()
}

pub async fn find_user_by_username(
    pool: &SqlitePool,
    username: &str,
) -> Result<Option<User>, sqlx::Error> {
    let row = sqlx::query(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE username_key = ?"
    ))
    .bind(username.to_ascii_lowercase())
    .fetch_optional(pool)
    .await?;
    row.map(|r| map_user(&r)).transpose()
}

pub async fn find_user_by_email(
    pool: &SqlitePool,
    email: &str,
) -> Result<Option<User>, sqlx::Error> {
    let row = sqlx::query(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE email_key = ?"
    ))
    .bind(email.to_ascii_lowercase())
    .fetch_optional(pool)
    .await?;
    row.map(|r| map_user(&r)).transpose()
}

/// Sign-in lookup: members may type either their username or their email.
pub async fn find_user_by_login(
    pool: &SqlitePool,
    identifier: &str,
) -> Result<Option<User>, sqlx::Error> {
    let key = identifier.to_ascii_lowercase();
    let row = sqlx::query(&format!(
        "SELECT {USER_COLUMNS} FROM users
         WHERE username_key = ? OR (email_key IS NOT NULL AND email_key = ?)"
    ))
    .bind(&key)
    .bind(&key)
    .fetch_optional(pool)
    .await?;
    row.map(|r| map_user(&r)).transpose()
}

/// Update the account's sign-in identity. `email` of `None` clears it.
pub async fn update_user_identity(
    pool: &SqlitePool,
    user_id: &str,
    username: &str,
    email: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users SET username = ?, username_key = ?, email = ?, email_key = ? WHERE id = ?",
    )
    .bind(username)
    .bind(username.to_ascii_lowercase())
    .bind(email)
    .bind(email.map(|e| e.to_ascii_lowercase()))
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_user_password(
    pool: &SqlitePool,
    user_id: &str,
    password_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
    role: Role,
    status: UserStatus,
) -> Result<User, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let created_at = now_rfc3339();
    let username_key = username.to_ascii_lowercase();

    sqlx::query(
        r#"
        INSERT INTO users (id, username, username_key, password_hash, role, status, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(username)
    .bind(&username_key)
    .bind(password_hash)
    .bind(role.as_str())
    .bind(status.as_str())
    .bind(&created_at)
    .execute(pool)
    .await?;

    Ok(User {
        id,
        username: username.to_string(),
        email: None,
        password_hash: password_hash.to_string(),
        role,
        status,
        created_at,
    })
}

pub async fn set_user_status(
    pool: &SqlitePool,
    user_id: &str,
    status: UserStatus,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET status = ? WHERE id = ? AND role = 'user'")
        .bind(status.as_str())
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_pending_users(pool: &SqlitePool) -> Result<Vec<PendingUser>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, username, created_at FROM users WHERE status = 'pending' ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| PendingUser {
            id: r.get("id"),
            username: r.get("username"),
            created_at: r.get("created_at"),
        })
        .collect())
}

pub async fn create_panel(
    pool: &SqlitePool,
    title: &str,
    description: &str,
    link: &str,
    media_type: MediaType,
    created_by: &str,
) -> Result<Panel, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let created_at = now_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO panels (id, title, description, link, media_type, status, created_by, created_at)
        VALUES (?, ?, ?, ?, ?, 'open', ?, ?)
        "#,
    )
    .bind(&id)
    .bind(title)
    .bind(description)
    .bind(link)
    .bind(media_type.as_str())
    .bind(created_by)
    .bind(&created_at)
    .execute(pool)
    .await?;

    Ok(Panel {
        id,
        title: title.to_string(),
        description: description.to_string(),
        link: link.to_string(),
        media_type,
        status: PanelStatus::Open,
        created_by: created_by.to_string(),
        created_at,
    })
}

/// Edit a panel's presentation. Media type is intentionally immutable — entries
/// were already validated against it.
pub async fn update_panel(
    pool: &SqlitePool,
    panel_id: &str,
    title: &str,
    description: &str,
    link: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE panels SET title = ?, description = ?, link = ? WHERE id = ?")
        .bind(title)
        .bind(description)
        .bind(link)
        .bind(panel_id)
        .execute(pool)
        .await?;
    Ok(())
}

fn map_panel(row: &sqlx::sqlite::SqliteRow) -> Result<Panel, sqlx::Error> {
    let media_type = MediaType::parse(row.get("media_type")).ok_or_else(|| {
        sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad media_type",
        )))
    })?;
    let status = PanelStatus::parse(row.get("status")).ok_or_else(|| {
        sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad panel status",
        )))
    })?;
    Ok(Panel {
        id: row.get("id"),
        title: row.get("title"),
        description: row.get("description"),
        link: row.get("link"),
        media_type,
        status,
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
    })
}

/// Every column [`map_panel`] expects.
const PANEL_COLUMNS: &str =
    "id, title, description, link, media_type, status, created_by, created_at";

/// Panels for the index pages, newest first with open panels on top.
///
/// Entry/vote totals come from correlated subqueries rather than two joins so
/// the counts stay independent (a join would multiply rows against each other).
pub async fn list_panel_summaries(pool: &SqlitePool) -> Result<Vec<PanelListing>, sqlx::Error> {
    let rows = sqlx::query(&format!(
        r#"
        SELECT {PANEL_COLUMNS},
               (SELECT COUNT(*) FROM entries e WHERE e.panel_id = panels.id) AS entry_count,
               (SELECT COUNT(*) FROM votes v WHERE v.panel_id = panels.id) AS vote_count
        FROM panels
        ORDER BY (status = 'open') DESC, created_at DESC
        "#
    ))
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|r| {
            Ok(PanelListing {
                panel: map_panel(r)?,
                entry_count: r.get("entry_count"),
                vote_count: r.get("vote_count"),
            })
        })
        .collect()
}

pub async fn find_panel(pool: &SqlitePool, id: &str) -> Result<Option<Panel>, sqlx::Error> {
    let row = sqlx::query(&format!("SELECT {PANEL_COLUMNS} FROM panels WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(map_panel).transpose()
}

pub async fn set_panel_status(
    pool: &SqlitePool,
    panel_id: &str,
    status: PanelStatus,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE panels SET status = ? WHERE id = ?")
        .bind(status.as_str())
        .bind(panel_id)
        .execute(pool)
        .await?;
    Ok(())
}

fn map_entry(r: sqlx::sqlite::SqliteRow) -> Entry {
    Entry {
        id: r.get("id"),
        panel_id: r.get("panel_id"),
        user_id: r.get("user_id"),
        username: r.get("username"),
        title: r.get("title"),
        original_filename: r.get("original_filename"),
        stored_path: r.get("stored_path"),
        content_type: r.get("content_type"),
        created_at: r.get("created_at"),
        vote_count: r.get("vote_count"),
    }
}

pub async fn list_entries_for_panel(
    pool: &SqlitePool,
    panel_id: &str,
) -> Result<Vec<Entry>, sqlx::Error> {
    // Aggregate votes with a single LEFT JOIN instead of a correlated subquery per row.
    let rows = sqlx::query(
        r#"
        SELECT e.id, e.panel_id, e.user_id, u.username, e.title, e.original_filename,
               e.stored_path, e.content_type, e.created_at,
               COUNT(v.id) AS vote_count
        FROM entries e
        JOIN users u ON u.id = e.user_id
        LEFT JOIN votes v ON v.entry_id = e.id
        WHERE e.panel_id = ?
        GROUP BY e.id
        ORDER BY vote_count DESC, e.created_at ASC
        "#,
    )
    .bind(panel_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(map_entry).collect())
}

/// Lightweight lookup used by media serving — no vote count / join needed.
pub async fn find_entry_media(
    pool: &SqlitePool,
    entry_id: &str,
) -> Result<Option<(String, String, String)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT stored_path, content_type, original_filename FROM entries WHERE id = ?",
    )
    .bind(entry_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| {
        (
            r.get("stored_path"),
            r.get("content_type"),
            r.get("original_filename"),
        )
    }))
}

pub async fn user_entry_for_panel(
    pool: &SqlitePool,
    panel_id: &str,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT id FROM entries WHERE panel_id = ? AND user_id = ?")
        .bind(panel_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get("id")))
}

pub async fn create_entry(
    pool: &SqlitePool,
    panel_id: &str,
    user_id: &str,
    title: &str,
    original_filename: &str,
    stored_path: &str,
    content_type: &str,
) -> Result<String, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let created_at = now_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO entries (id, panel_id, user_id, title, original_filename, stored_path, content_type, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(panel_id)
    .bind(user_id)
    .bind(title)
    .bind(original_filename)
    .bind(stored_path)
    .bind(content_type)
    .bind(&created_at)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn user_vote_entry_id(
    pool: &SqlitePool,
    panel_id: &str,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT entry_id FROM votes WHERE panel_id = ? AND user_id = ?")
        .bind(panel_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get("entry_id")))
}

/// Insert or update the voter's single vote for a panel (change while open).
pub async fn upsert_vote(
    pool: &SqlitePool,
    panel_id: &str,
    entry_id: &str,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    let now = now_rfc3339();
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO votes (id, panel_id, entry_id, user_id, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(panel_id, user_id) DO UPDATE SET
            entry_id = excluded.entry_id,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(&id)
    .bind(panel_id)
    .bind(entry_id)
    .bind(user_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Fast ownership check for vote casting without loading a full Entry.
pub async fn entry_vote_target(
    pool: &SqlitePool,
    entry_id: &str,
    panel_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT user_id FROM entries WHERE id = ? AND panel_id = ?")
        .bind(entry_id)
        .bind(panel_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get("user_id")))
}

