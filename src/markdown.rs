//! Markdown rendering for panel descriptions.
//!
//! Output is trusted enough to embed unescaped (`|safe` in templates) because
//! this module — not a downstream sanitizer — is what makes it safe:
//!
//! * raw HTML in the source is **dropped**, so no tag an author writes ever
//!   reaches the page;
//! * link and image destinations are restricted to `http`, `https`, `mailto`,
//!   and same-document/relative URLs, which rules out `javascript:` and
//!   `data:` payloads;
//! * headings are demoted two levels so a description cannot introduce an
//!   `<h1>` that competes with the page title.

use pulldown_cmark::{CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};

fn options() -> Options {
    Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES
}

/// Validate a standalone URL typed into a form (the panel's related link).
/// Stricter than [`is_safe_url`]: an absolute `http`/`https` address only.
pub fn validate_link(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.len() > 2000 {
        return Err("Link is too long.".into());
    }
    if url.chars().any(char::is_whitespace) {
        return Err("Link must not contain spaces.".into());
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| "Link must start with http:// or https://".to_string())?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || !host.contains('.') || host.starts_with('.') || host.ends_with('.') {
        return Err("Link must include a valid domain.".into());
    }
    Ok(())
}

/// Schemes we are willing to emit in `href`/`src`.
fn is_safe_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Relative, root-relative, and fragment links carry no scheme at all.
    // A scheme must appear before any '/', '?' or '#' to count as one.
    let scheme_end = trimmed.find(':');
    let path_start = trimmed.find(['/', '?', '#']).unwrap_or(trimmed.len());
    match scheme_end {
        Some(i) if i < path_start => {
            let scheme = trimmed[..i].to_ascii_lowercase();
            matches!(scheme.as_str(), "http" | "https" | "mailto")
        }
        _ => true,
    }
}

fn demote(level: HeadingLevel) -> HeadingLevel {
    match level {
        HeadingLevel::H1 | HeadingLevel::H2 | HeadingLevel::H3 => HeadingLevel::H3,
        HeadingLevel::H4 => HeadingLevel::H4,
        HeadingLevel::H5 => HeadingLevel::H5,
        HeadingLevel::H6 => HeadingLevel::H6,
    }
}

/// Render Markdown to HTML that is safe to embed without escaping.
pub fn to_html(source: &str) -> String {
    let mut out = String::with_capacity(source.len() + source.len() / 2);

    let events = Parser::new_ext(source, options()).filter_map(|event| match event {
        // Never echo author-supplied HTML back to the browser.
        Event::Html(_) | Event::InlineHtml(_) => None,

        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Some(Event::Start(Tag::Link {
            link_type,
            dest_url: sanitize(dest_url),
            title,
            id,
        })),

        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Some(Event::Start(Tag::Image {
            link_type,
            dest_url: sanitize(dest_url),
            title,
            id,
        })),

        // Both ends carry the level; rewriting only the start would emit
        // mismatched tags like `<h3>…</h1>`.
        Event::Start(Tag::Heading {
            level,
            id,
            classes,
            attrs,
        }) => Some(Event::Start(Tag::Heading {
            level: demote(level),
            id,
            classes,
            attrs,
        })),
        Event::End(TagEnd::Heading(level)) => Some(Event::End(TagEnd::Heading(demote(level)))),

        other => Some(other),
    });

    html::push_html(&mut out, events);
    out
}

fn sanitize(url: CowStr<'_>) -> CowStr<'_> {
    if is_safe_url(&url) {
        url
    } else {
        CowStr::Borrowed("")
    }
}

/// Flatten Markdown to plain text for list-card excerpts, so `**bold**` and
/// link syntax don't leak into places that can't render them.
pub fn to_plain_text(source: &str, max_chars: usize) -> String {
    let mut out = String::new();

    for event in Parser::new_ext(source, options()) {
        match event {
            Event::Text(t) | Event::Code(t) => out.push_str(&t),
            Event::SoftBreak | Event::HardBreak => out.push(' '),
            // Block boundaries become spaces so words don't run together.
            Event::End(_) if !out.is_empty() && !out.ends_with(' ') => out.push(' '),
            _ => {}
        }
        if out.chars().count() > max_chars {
            break;
        }
    }

    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > max_chars {
        let clipped: String = collapsed.chars().take(max_chars).collect();
        // Avoid cutting mid-word when there's a nearby space to break on.
        match clipped.rsplit_once(' ') {
            Some((head, _)) if head.chars().count() > max_chars / 2 => format!("{head}…"),
            _ => format!("{clipped}…"),
        }
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_basic_markdown() {
        let html = to_html("**bold** and [a link](https://example.com)");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains(r#"<a href="https://example.com">a link</a>"#));
    }

    #[test]
    fn drops_raw_html() {
        let html = to_html("<script>alert(1)</script>\n\nhello <b>x</b>");
        assert!(!html.contains("<script"));
        assert!(!html.contains("alert"));
        assert!(!html.contains("<b>"));
        assert!(html.contains("hello"));
    }

    #[test]
    fn strips_dangerous_link_schemes() {
        let html = to_html("[click](javascript:alert(1))");
        assert!(!html.contains("javascript:"));

        let html = to_html("![x](data:text/html;base64,PHN2Zz4=)");
        assert!(!html.contains("data:"));
    }

    #[test]
    fn keeps_ordinary_url_shapes() {
        for url in [
            "https://example.com/a?b=1#c",
            "http://example.com",
            "mailto:someone@example.com",
            "/panels/abc",
            "#section",
            "relative/path.png",
        ] {
            assert!(is_safe_url(url), "{url} should be allowed");
        }
        for url in ["javascript:alert(1)", "data:text/html,x", "vbscript:x"] {
            assert!(!is_safe_url(url), "{url} should be rejected");
        }
    }

    #[test]
    fn escapes_text_that_looks_like_html() {
        // Not raw HTML (no tag), just text with angle brackets.
        let html = to_html("5 < 6 & 7 > 2");
        assert!(html.contains("&lt;"));
        assert!(html.contains("&amp;"));
    }

    #[test]
    fn demotes_headings_below_the_page_title() {
        let html = to_html("# Big\n## Smaller\n#### Deep");
        assert!(html.contains("<h3>Big</h3>"), "got {html}");
        assert!(html.contains("<h3>Smaller</h3>"), "got {html}");
        assert!(html.contains("<h4>Deep</h4>"), "got {html}");
        // No stray opening or closing tag at h1/h2 level.
        for tag in ["<h1>", "</h1>", "<h2>", "</h2>"] {
            assert!(!html.contains(tag), "{tag} leaked into {html}");
        }
    }

    #[test]
    fn plain_text_strips_syntax_and_truncates() {
        let text = to_plain_text("# Title\n\n**bold** [link](https://x.com) text", 200);
        assert_eq!(text, "Title bold link text");

        let long = to_plain_text(&"word ".repeat(100), 20);
        assert!(long.chars().count() <= 21, "got {long:?}");
        assert!(long.ends_with('…'));
        assert!(!long.contains("  "));
    }

    #[test]
    fn plain_text_handles_empty_input() {
        assert_eq!(to_plain_text("", 50), "");
    }

    #[test]
    fn related_link_must_be_an_absolute_web_url() {
        for good in [
            "https://github.com/Aspenini/jellywin32",
            "http://example.com",
            "https://example.com/path?q=1#frag",
        ] {
            assert!(validate_link(good).is_ok(), "{good} should be accepted");
        }
        for bad in [
            "javascript:alert(1)",
            "ftp://example.com",
            "example.com",
            "/relative",
            "https://",
            "https://nodot",
            "https://exa mple.com",
        ] {
            assert!(validate_link(bad).is_err(), "{bad} should be rejected");
        }
    }
}
