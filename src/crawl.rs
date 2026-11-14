//! Rust port of `extracter.js` + `helpers.extractUrls`: fetch the target page
//! and collect the same-site links that virtual users will hit.

use anyhow::{Context, Result};
use scraper::{Html, Selector};
use std::collections::BTreeSet;
use url::Url;

/// Fetch `target` and return the site-relative paths linked from it.
/// Always includes `/` so a run has at least one path.
pub async fn discover_paths(target: &Url) -> Result<Vec<String>> {
    let html = reqwest::Client::builder()
        .user_agent(concat!("strezz/", env!("CARGO_PKG_VERSION")))
        .build()?
        .get(target.clone())
        .send()
        .await
        .with_context(|| format!("fetching {target}"))?
        .error_for_status()?
        .text()
        .await?;
    Ok(extract_paths(&html, target))
}

/// Pure link extraction, kept separate so it is unit-testable.
///
/// Compared to the JS version this also keeps same-origin absolute links,
/// drops protocol-relative `//other-host` links (which `startsWith("/")`
/// wrongly accepted), strips `#fragments` and de-duplicates.
pub fn extract_paths(html: &str, base: &Url) -> Vec<String> {
    let doc = Html::parse_document(html);
    let anchors = Selector::parse("a[href]").expect("static selector");
    let mut paths = BTreeSet::new();
    paths.insert("/".to_string());

    for a in doc.select(&anchors) {
        let Some(href) = a.value().attr("href").map(str::trim) else {
            continue;
        };
        if href.is_empty() || href.starts_with('#') {
            continue;
        }
        let Ok(mut url) = base.join(href) else {
            continue;
        };
        if url.origin() != base.origin() {
            continue;
        }
        url.set_fragment(None);
        let mut path = url.path().to_string();
        if let Some(q) = url.query() {
            path.push('?');
            path.push_str(q);
        }
        paths.insert(path);
    }
    paths.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_same_site_links() {
        let base = Url::parse("https://example.com/").unwrap();
        let html = r##"
            <a href="/about">About</a>
            <a href="/about#team">Team</a>
            <a href="/search?q=a b">Search</a>
            <a href="https://example.com/pricing">Pricing</a>
            <a href="//cdn.other.com/x.js">CDN</a>
            <a href="https://google.com/">Google</a>
            <a href="#top">Top</a>
            <a href="mailto:hi@example.com">Mail</a>
            <a>No href</a>
        "##;
        assert_eq!(
            extract_paths(html, &base),
            vec!["/", "/about", "/pricing", "/search?q=a%20b"]
        );
    }

    #[test]
    fn empty_page_still_has_root() {
        let base = Url::parse("http://localhost:8080/").unwrap();
        assert_eq!(extract_paths("", &base), vec!["/"]);
    }
}
