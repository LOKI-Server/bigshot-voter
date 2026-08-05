//! Static assets compiled into the executable.
//!
//! The server ships as a *single* file: CSS, JS, and the favicon are baked in
//! with `include_bytes!`, so there is no `static/` folder to copy next to the
//! binary and nothing to go missing at runtime.
//!
//! Each asset is exposed at a content-hashed URL (`/static/style.<hash>.css`)
//! so browsers can cache it forever and still pick up changes after an upgrade.
//! Templates link to it via [`css_href`] / [`js_href`].

use std::sync::LazyLock;

use axum::extract::Path;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// Hashed URLs can never go stale, so cache them for a year.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Un-hashed URLs must be revalidated (cheap: the ETag makes it a 304).
const REVALIDATE: &str = "public, no-cache";

struct Asset {
    /// Logical name, e.g. `style.css`.
    name: &'static str,
    body: &'static [u8],
    content_type: HeaderValue,
    /// Content-hashed public URL, e.g. `/static/style.Ab3xQ.css`.
    href: String,
    etag: HeaderValue,
}

impl Asset {
    fn new(name: &'static str, body: &'static [u8], content_type: &'static str) -> Self {
        // 8 bytes of SHA-256 is ample for cache busting and keeps URLs short.
        let hash = URL_SAFE_NO_PAD.encode(&Sha256::digest(body)[..8]);
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name, "bin"));
        Self {
            name,
            body,
            content_type: HeaderValue::from_static(content_type),
            href: format!("/static/{stem}.{hash}.{ext}"),
            etag: HeaderValue::try_from(format!("\"{hash}\"")).expect("base64 hash is ASCII"),
        }
    }
}

static ASSETS: LazyLock<[Asset; 2]> = LazyLock::new(|| {
    [
        Asset::new(
            "style.css",
            include_bytes!("../static/style.css"),
            "text/css; charset=utf-8",
        ),
        Asset::new(
            "app.js",
            include_bytes!("../static/app.js"),
            "text/javascript; charset=utf-8",
        ),
    ]
});

fn asset(name: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|a| a.name == name)
}

fn href(name: &str) -> &'static str {
    asset(name).map(|a| a.href.as_str()).unwrap_or("/static/")
}

// Called from templates: `{{ crate::assets::css_href() }}`.
pub fn css_href() -> &'static str {
    href("style.css")
}

pub fn js_href() -> &'static str {
    href("app.js")
}

/// The site deliberately has no icon. Answer `/favicon.ico` with "no content"
/// rather than a 404 so browsers stop asking and logs stay quiet.
pub async fn no_favicon() -> Response {
    (
        StatusCode::NO_CONTENT,
        [(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=604800"),
        )],
    )
        .into_response()
}

/// Look up an asset by either its logical or content-hashed file name.
/// The bool reports whether the request used the hashed (immutable) URL.
fn lookup(requested: &str) -> Option<(&'static Asset, bool)> {
    if let Some(a) = asset(requested) {
        return Some((a, false));
    }
    let hashed = ASSETS
        .iter()
        .find(|a| a.href.rsplit('/').next() == Some(requested))?;
    Some((hashed, true))
}

/// `GET /static/{file}` — serves an embedded asset with ETag revalidation.
pub async fn serve(Path(file): Path<String>, headers: HeaderMap) -> Response {
    let Some((asset, hashed)) = lookup(&file) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };

    if if_none_match_hits(&headers, &asset.etag) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, asset.etag.clone())]).into_response();
    }

    let cache = if hashed { IMMUTABLE } else { REVALIDATE };
    (
        [
            (header::CONTENT_TYPE, asset.content_type.clone()),
            (header::ETAG, asset.etag.clone()),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
        ],
        asset.body,
    )
        .into_response()
}

fn if_none_match_hits(headers: &HeaderMap, etag: &HeaderValue) -> bool {
    let Some(value) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let etag = etag.to_str().unwrap_or_default();
    value
        .split(',')
        .map(|t| t.trim().trim_start_matches("W/"))
        .any(|t| t == "*" || t == etag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_href_resolves_back_to_the_asset() {
        let file = css_href().rsplit('/').next().unwrap();
        let (asset, hashed) = lookup(file).expect("hashed name resolves");
        assert_eq!(asset.name, "style.css");
        assert!(hashed);
    }

    #[test]
    fn plain_name_resolves_without_immutable_caching() {
        let (asset, hashed) = lookup("style.css").expect("plain name resolves");
        assert_eq!(asset.name, "style.css");
        assert!(!hashed);
    }

    #[test]
    fn unknown_asset_is_missing() {
        assert!(lookup("nope.css").is_none());
    }
}
