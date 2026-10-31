use super::*;
use models::{Entry, MediaType, Panel, PanelStatus};
use templates::*;

fn rows(statement: &str, params: Value) -> Result<Vec<Value>> {
    Ok(sql(statement, params)?.rows)
}
fn string(row: &Value, key: &str) -> String {
    row[key].as_str().unwrap_or("").into()
}
fn optional(row: &Value, key: &str) -> Option<String> {
    row[key].as_str().map(str::to_string)
}
fn first(statement: &str, params: Value) -> Result<Value> {
    rows(statement, params)?
        .into_iter()
        .next()
        .ok_or_else(|| "Not found.".into())
}
pub fn nav(context: &Context) -> Option<NavUser> {
    context.user.as_ref().map(|user| NavUser {
        username: user.username.clone(),
        is_admin: user.has_role("admin"),
        is_approved: true,
    })
}
fn is_admin(context: &Context) -> bool {
    context
        .user
        .as_ref()
        .is_some_and(|user| user.has_role("admin"))
}
fn panel(row: &Value) -> Result<Panel> {
    Ok(Panel {
        id: string(row, "id"),
        title: string(row, "title"),
        description: string(row, "description"),
        link: string(row, "link"),
        media_type: MediaType::parse(row["media_type"].as_str().unwrap_or(""))
            .ok_or("Invalid panel media type")?,
        status: PanelStatus::parse(row["status"].as_str().unwrap_or(""))
            .ok_or("Invalid panel status")?,
        created_by: string(row, "created_by"),
        created_at: string(row, "created_at"),
    })
}
fn find_panel(id: &str) -> Result<Panel> {
    panel(&first("SELECT * FROM panels WHERE id=?", json!([id]))?)
}
fn summaries() -> Result<Vec<PanelSummary>> {
    rows("SELECT p.*, (SELECT count(*) FROM entries e WHERE e.panel_id=p.id) AS entry_count, (SELECT count(*) FROM votes v WHERE v.panel_id=p.id) AS vote_count FROM panels p ORDER BY (status='open') DESC,created_at DESC LIMIT 1000",json!([]))?.iter().map(|row|{let p=panel(row)?;Ok(PanelSummary{id:p.id,title:p.title,excerpt:markdown::to_plain_text(&p.description,160),media_label:p.media_type.label().into(),status:p.status.as_str().into(),is_open:p.status==PanelStatus::Open,entry_count:row["entry_count"].as_i64().unwrap_or(0),vote_count:row["vote_count"].as_i64().unwrap_or(0)})}).collect()
}
fn entries(panel_id: &str) -> Result<Vec<Entry>> {
    Ok(rows("SELECT e.*,u.username,COUNT(v.id) AS vote_count FROM entries e JOIN users u ON u.id=e.user_id LEFT JOIN votes v ON v.entry_id=e.id WHERE e.panel_id=? GROUP BY e.id ORDER BY vote_count DESC,e.created_at ASC LIMIT 1000",json!([panel_id]))?.iter().map(|r|Entry{id:string(r,"id"),panel_id:string(r,"panel_id"),user_id:string(r,"user_id"),username:string(r,"username"),title:string(r,"title"),original_filename:string(r,"original_filename"),stored_path:string(r,"stored_path"),content_type:string(r,"content_type"),master_original_filename:optional(r,"master_original_filename"),master_stored_path:optional(r,"master_stored_path"),created_at:string(r,"created_at"),vote_count:r["vote_count"].as_i64().unwrap_or(0)}).collect())
}
fn init() -> Result<()> {
    let statements = [
        "CREATE TABLE IF NOT EXISTS users(id TEXT PRIMARY KEY,username TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS panels(id TEXT PRIMARY KEY,title TEXT NOT NULL,description TEXT NOT NULL DEFAULT '',link TEXT NOT NULL DEFAULT '',media_type TEXT NOT NULL CHECK(media_type IN('image','audio','video','other')),status TEXT NOT NULL CHECK(status IN('open','closed')),created_by TEXT NOT NULL REFERENCES users(id),created_at TEXT NOT NULL)",
        "CREATE TABLE IF NOT EXISTS entries(id TEXT PRIMARY KEY,panel_id TEXT NOT NULL REFERENCES panels(id) ON DELETE CASCADE,user_id TEXT NOT NULL REFERENCES users(id),title TEXT NOT NULL,original_filename TEXT NOT NULL,stored_path TEXT NOT NULL,content_type TEXT NOT NULL,master_original_filename TEXT,master_stored_path TEXT,created_at TEXT NOT NULL,UNIQUE(panel_id,user_id))",
        "CREATE TABLE IF NOT EXISTS votes(id TEXT PRIMARY KEY,panel_id TEXT NOT NULL REFERENCES panels(id) ON DELETE CASCADE,entry_id TEXT NOT NULL REFERENCES entries(id) ON DELETE CASCADE,user_id TEXT NOT NULL REFERENCES users(id),created_at TEXT NOT NULL,updated_at TEXT NOT NULL,UNIQUE(panel_id,user_id))",
        "CREATE INDEX IF NOT EXISTS idx_entries_panel ON entries(panel_id)",
        "CREATE INDEX IF NOT EXISTS idx_votes_panel ON votes(panel_id)",
        "CREATE INDEX IF NOT EXISTS idx_votes_entry ON votes(entry_id)",
    ];
    Database.transaction(
        &statements
            .iter()
            .map(|sql| Statement::new(*sql, json!([])))
            .collect::<Result<Vec<_>>>()?,
    )?;
    Ok(())
}
pub fn handle(method: &Method, path: &str, context: &Context) -> Result<Response> {
    let get = matches!(method, Method::Get | Method::Head);
    if get && path == "/health" {
        init()?;
        return Ok(Response::text("ready").with_status(200));
    }
    if get && path.starts_with("/static/") {
        return loki_sdk::asset(path.trim_start_matches('/'));
    }
    if get && path == "/favicon.ico" {
        return Ok(Response::text("").with_status(204));
    }
    if path == "/settings" || path == "/register" {
        return redirect(&format!("{}/account", context.dashboard_url.as_str()));
    }
    if get && path == "/login" {
        return redirect(if context.user.is_none() {
            "/_loki/start"
        } else {
            "/panels"
        });
    }
    let csrf = context.csrf.clone();
    if path == "/" && get {
        return html(HomePage {
            user: nav(context),
            flash: None,
            csrf_token: csrf,
            panels: summaries()?,
        });
    }
    let Some(user_id) = context.user.as_ref().map(|u| u.id.as_str()) else {
        return redirect("/_loki/start");
    };
    sql(
        "INSERT INTO users(id,username) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET username=excluded.username",
        json!([user_id, context.require_user()?.username]),
    )?;
    if get && (path == "/panels" || path == "/pending") {
        return html(PanelsListPage {
            user: nav(context),
            flash: None,
            csrf_token: csrf,
            panels: summaries()?,
        });
    }
    if path.starts_with("/admin") {
        if !is_admin(context) {
            return Ok(
                Response::text("Application administrator access is required.").with_status(403),
            );
        }
        return admin(method, path, context);
    }
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    if parts.len() == 2 && parts[0] == "panels" && get {
        let p = find_panel(parts[1])?;
        let entries = entries(&p.id)?;
        let my_entry_id = entries
            .iter()
            .find(|e| e.user_id == user_id)
            .map(|e| e.id.clone());
        let my_vote_entry_id = rows(
            "SELECT entry_id FROM votes WHERE panel_id=? AND user_id=?",
            json!([p.id, user_id]),
        )?
        .first()
        .and_then(|r| optional(r, "entry_id"));
        let views = EntryView::build(
            &entries,
            Some(user_id),
            my_vote_entry_id.as_deref(),
            p.is_open(),
        );
        return html(PanelDetailPage {
            user: nav(context),
            flash: None,
            csrf_token: csrf,
            panel: PanelView::from(&p),
            entries: views,
            my_entry_id: my_entry_id.clone(),
            my_vote_entry_id,
            can_upload: p.is_open() && my_entry_id.is_none(),
            can_vote: p.is_open() && !entries.is_empty(),
            total_votes: entries.iter().map(|e| e.vote_count).sum(),
            max_upload_label: models::format_bytes(context.config::<Settings>()?.max_upload_bytes),
            master_accept: media::master_accept_attr(),
            is_admin: is_admin(context),
        });
    }
    if parts.len() == 3 && parts[0] == "panels" && matches!(method, Method::Post) {
        let p = find_panel(parts[1])?;
        if !p.is_open() {
            return Err("This panel is closed; entries and votes cannot be changed.".into());
        }
        let form = context.form()?;
        match parts[2] {
            "entries" => upload_entry(&p, user_id, &form),
            "votes" => {
                let entry_id = form.required("entry_id")?;
                let owner = first(
                    "SELECT user_id FROM entries WHERE id=? AND panel_id=?",
                    json!([entry_id, p.id]),
                )?;
                if owner["user_id"] == user_id {
                    return Err("You cannot vote for your own entry.".into());
                }
                let now = now();
                let result = sql(
                    "INSERT INTO votes(id,panel_id,entry_id,user_id,created_at,updated_at) SELECT ?,?,?,?,?,? WHERE EXISTS(SELECT 1 FROM panels WHERE id=? AND status='open') AND EXISTS(SELECT 1 FROM entries WHERE id=? AND panel_id=? AND user_id<>?) ON CONFLICT(panel_id,user_id) DO UPDATE SET entry_id=excluded.entry_id,updated_at=excluded.updated_at WHERE EXISTS(SELECT 1 FROM panels WHERE id=? AND status='open')",
                    json!([
                        id(),
                        p.id,
                        entry_id,
                        user_id,
                        now,
                        now,
                        p.id,
                        entry_id,
                        p.id,
                        user_id,
                        p.id
                    ]),
                )?;
                if result.affected == 0 {
                    return Err("This panel is closed or the entry is unavailable.".into());
                }
                redirect(&format!("/panels/{}", p.id))
            }
            _ => not_found(),
        }
    } else if parts.len() >= 2 && parts[0] == "media" {
        let entry = first(
            "SELECT e.*,p.media_type FROM entries e JOIN panels p ON p.id=e.panel_id WHERE e.id=?",
            json!([parts[1]]),
        )?;
        let master = parts.len() == 3 && parts[2] == "master";
        if parts.len() > 3 || (!master && parts.len() > 2) {
            return not_found();
        }
        if get {
            let (path, name) = if master {
                (
                    entry["master_stored_path"]
                        .as_str()
                        .ok_or("No master file is attached.")?,
                    entry["master_original_filename"]
                        .as_str()
                        .unwrap_or("master"),
                )
            } else {
                (
                    entry["stored_path"].as_str().ok_or("Missing media.")?,
                    entry["original_filename"].as_str().unwrap_or("download"),
                )
            };
            let file = StoredFile::at(path);
            if master || entry["media_type"] == "other" {
                file.download(name)
            } else {
                file.inline(name)
            }
        } else if master && matches!(method, Method::Post) {
            if entry["user_id"] != user_id {
                return Err("Only the uploader can add a master file.".into());
            }
            if entry["master_stored_path"]
                .as_str()
                .is_some_and(|p| !p.is_empty())
            {
                return Err("This entry already has a master file.".into());
            }
            let form = context.form()?;
            let upload = form.required_file("master")?;
            let name = upload.name.clone();
            let validated = media::validate_master_upload(&name)?;
            let path = format!(
                "{}/{}.{}",
                string(&entry, "panel_id"),
                id(),
                validated.extension
            );
            upload.retain(path.clone())?;
            let result = sql(
                "UPDATE entries SET master_original_filename=?,master_stored_path=? WHERE id=? AND user_id=? AND (master_stored_path IS NULL OR master_stored_path='')",
                json!([name, path, parts[1], user_id]),
            );
            if !result.as_ref().is_ok_and(|r| r.affected == 1) {
                let _ = StoredFile::at(path).delete();
                return Err("The master file could not be attached.".into());
            }
            redirect(&format!("/panels/{}", string(&entry, "panel_id")))
        } else {
            not_found()
        }
    } else {
        not_found()
    }
}
fn not_found() -> Result<Response> {
    Ok(Response::text("Not found.").with_status(404))
}
fn upload_entry(panel: &Panel, user_id: &str, form: &Form) -> Result<Response> {
    if !rows(
        "SELECT id FROM entries WHERE panel_id=? AND user_id=?",
        json!([panel.id, user_id]),
    )?
    .is_empty()
    {
        return Err("You already uploaded an entry to this panel.".into());
    }
    let upload = form.required_file("file")?;
    let name = upload.name.clone();
    let validated =
        media::validate_upload(panel.media_type, &name, Some(upload.content_type.as_str()))?;
    let path = format!("{}/{}.{}", panel.id, id(), validated.extension);
    let mut master_name = None;
    let mut master_path = None;
    if let Ok(master) = form.required_file("master") {
        let name = master.name.clone();
        let validated = media::validate_master_upload(&name)?;
        master_name = Some(name);
        master_path = Some(format!("{}/{}.{}", panel.id, id(), validated.extension));
    }
    let title = form
        .field("title")
        .unwrap_or("")
        .to_string()
        .trim()
        .to_string();
    let title = if title.is_empty() {
        std::path::Path::new(&name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Entry")
            .chars()
            .take(120)
            .collect()
    } else {
        title
    };
    if title.chars().count() > 120 {
        return Err("Entry title must be at most 120 characters.".into());
    }
    upload.retain(path.clone())?;
    let result = (|| {
        if let Some(path) = &master_path {
            let upload = form.required_file("master")?;
            upload.retain(path.clone())?;
        }
        let result = sql(
            "INSERT INTO entries(id,panel_id,user_id,title,original_filename,stored_path,content_type,master_original_filename,master_stored_path,created_at) SELECT ?,?,?,?,?,?,?,?,?,? WHERE EXISTS(SELECT 1 FROM panels WHERE id=? AND status='open')",
            json!([
                id(),
                panel.id,
                user_id,
                title,
                name,
                path,
                validated.content_type,
                master_name,
                master_path,
                now(),
                panel.id
            ]),
        )?;
        if result.affected != 1 {
            return Err("The panel closed before the entry was saved.".into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = StoredFile::at(path).delete();
        if let Some(path) = master_path {
            let _ = StoredFile::at(path).delete();
        }
        return Err(error);
    }
    redirect(&format!("/panels/{}", panel.id))
}
fn admin(method: &Method, path: &str, context: &Context) -> Result<Response> {
    let csrf = context.csrf.clone();
    let user = nav(context);
    let get = matches!(method, Method::Get | Method::Head);
    if path == "/admin" && get {
        return html(AdminPage {
            user,
            flash: None,
            csrf_token: csrf,
            panels: summaries()?,
        });
    }
    if path == "/admin/panels/new" && get {
        return html(AdminPanelFormPage::new_panel(user, csrf, None));
    }
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    let p = if parts.len() == 4 && parts[0] == "admin" && parts[1] == "panels" {
        Some(find_panel(parts[2])?)
    } else {
        None
    };
    if get {
        if let Some(p) = p.filter(|_| parts[3] == "edit") {
            return html(AdminPanelFormPage {
                user,
                flash: None,
                csrf_token: csrf,
                error: None,
                panel_id: Some(p.id),
                is_edit: true,
                title: p.title,
                description: p.description,
                link: p.link,
                media_label: p.media_type.label().into(),
            });
        }
        return not_found();
    }
    if !matches!(method, Method::Post) {
        return not_found();
    }
    let form = context.form()?;
    let fields = &form;
    if let Some(p) = &p {
        if matches!(parts[3], "open" | "close") {
            sql(
                "UPDATE panels SET status=? WHERE id=?",
                json!([if parts[3] == "open" { "open" } else { "closed" }, p.id]),
            )?;
            return redirect("/admin");
        }
        if parts[3] != "edit" {
            return not_found();
        }
    } else if path != "/admin/panels/new" {
        return not_found();
    }
    let title = fields
        .field("title")
        .unwrap_or("")
        .to_string()
        .trim()
        .to_string();
    let description = fields
        .field("description")
        .unwrap_or("")
        .to_string()
        .trim()
        .chars()
        .take(4000)
        .collect::<String>();
    let link = fields
        .field("link")
        .unwrap_or("")
        .to_string()
        .trim()
        .to_string();
    let validation = validate_panel_fields(&title, &link);
    let media_type = if let Some(p) = &p {
        Some(p.media_type)
    } else {
        MediaType::parse(fields.field("media_type").unwrap_or(""))
    };
    if let Err(error) = validation.and_then(|_| {
        media_type
            .map(|_| ())
            .ok_or_else(|| "Choose a media type.".into())
    }) {
        return html(AdminPanelFormPage {
            user,
            flash: None,
            csrf_token: csrf,
            error: Some(error.to_string()),
            panel_id: p.as_ref().map(|p| p.id.clone()),
            is_edit: p.is_some(),
            title,
            description,
            link,
            media_label: p
                .as_ref()
                .map(|p| p.media_type.label().into())
                .unwrap_or_default(),
        });
    }
    let panel_id = if let Some(p) = p {
        sql(
            "UPDATE panels SET title=?,description=?,link=? WHERE id=?",
            json!([title, description, link, p.id]),
        )?;
        p.id
    } else {
        let panel_id = id();
        sql(
            "INSERT INTO panels(id,title,description,link,media_type,status,created_by,created_at) VALUES(?,?,?,?,?,'open',?,?)",
            json!([
                panel_id,
                title,
                description,
                link,
                media_type.unwrap().as_str(),
                context.require_user()?.id,
                now()
            ]),
        )?;
        panel_id
    };
    redirect(&format!("/panels/{panel_id}"))
}
fn validate_panel_fields(title: &str, link: &str) -> Result<()> {
    if title.is_empty() || title.chars().count() > 120 {
        return Err("Title is required (max 120 characters).".into());
    }
    if !link.is_empty() {
        markdown::validate_link(link)?;
    }
    Ok(())
}
