//! Rust port of `extracter.js` + `helpers.extractUrls`: fetch the target page
//! and collect the same-site links that virtual users will hit.

use anyhow::{Context, Result};
use scraper::{Html, Selector};
use std::collections::BTreeSet;
use std::time::Duration;
use url::Url;

/// How long the crawl may take. A site that accepts the connection and then
/// says nothing would otherwise hang the coordinator before it has started.
const CRAWL_TIMEOUT: Duration = Duration::from_secs(15);

/// What a crawl found.
#[derive(Debug)]
pub struct Crawl {
    /// Where the target ended up after redirects (http to https, apex to
    /// www...). The paths are relative to this address, not the one asked for.
    pub final_url: Url,
    /// Site-relative paths linked from the page. Always includes `/`.
    pub paths: Vec<String>,
}

/// Fetch `target` and return the site-relative paths linked from it.
pub async fn discover_paths(target: &Url) -> Result<Crawl> {
    crawl(target, CRAWL_TIMEOUT).await
}

async fn crawl(target: &Url, timeout: Duration) -> Result<Crawl> {
    let resp = reqwest::Client::builder()
        .user_agent(concat!("strezz/", env!("CARGO_PKG_VERSION")))
        .timeout(timeout)
        .build()?
        .get(target.clone())
        .send()
        .await
        .with_context(|| format!("fetching {target}"))?
        .error_for_status()?;
    // After a redirect the page's links belong to the address it was served
    // from. Resolving them against the address we asked for would drop every
    // absolute link as "another site".
    let final_url = resp.url().clone();
    let html = resp.text().await?;
    let paths = extract_paths(&html, &final_url);
    Ok(Crawl { final_url, paths })
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

/// Drop the paths that contain any of `patterns`, for links a load test
/// should leave alone: `/logout`, `.zip`, `/admin/`...
pub fn exclude(paths: Vec<String>, patterns: &[String]) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| !patterns.iter().any(|p| !p.is_empty() && path.contains(p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestServer;
    use axum::response::{Html as Page, Redirect};
    use axum::Router;

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

    #[test]
    fn exclude_drops_paths_containing_a_pattern() {
        let paths = |list: &[&str]| list.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let found = paths(&["/", "/about", "/logout", "/files/big.zip", "/admin/users"]);
        let skip = paths(&["/logout", ".zip", "/admin/", ""]);
        assert_eq!(exclude(found.clone(), &skip), vec!["/", "/about"]);
        assert_eq!(exclude(found.clone(), &[]), found);
    }

    #[tokio::test]
    async fn links_are_resolved_against_the_address_after_a_redirect() {
        // The real site links to itself with absolute URLs, as many sites do.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let site = format!("http://{}", listener.local_addr().unwrap());
        let page = format!(
            r#"<a href="{site}/pricing">Pricing</a> <a href="{site}/docs">Docs</a>
               <a href="/about">About</a>"#
        );
        tokio::spawn(async move {
            let router = Router::new().fallback(move || {
                let page = page.clone();
                async move { Page(page) }
            });
            axum::serve(listener, router).await.unwrap();
        });
        // The address the user typed only redirects there. It is a different
        // origin, as with http -> https or apex -> www.
        let destination = format!("{site}/");
        let entry = TestServer::start(Router::new().fallback(move || {
            let destination = destination.clone();
            async move { Redirect::temporary(&destination) }
        }))
        .await;

        let crawl = discover_paths(&Url::parse(&entry.url("/")).unwrap())
            .await
            .unwrap();
        assert_eq!(crawl.final_url.as_str(), format!("{site}/"));
        assert_eq!(crawl.paths, vec!["/", "/about", "/docs", "/pricing"]);
    }

    #[tokio::test]
    async fn a_silent_target_times_out_instead_of_hanging() {
        // Accepts the connection, never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            crawl(&target, Duration::from_millis(300)),
        )
        .await
        .expect("the crawl must give up by itself");
        assert!(outcome.is_err());
    }
}
