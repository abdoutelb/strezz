mod coordinator;
mod crawl;
mod limits;
mod locust;
mod netaddr;
mod protocol;
mod stats;
mod worker;

#[cfg(test)]
mod testutil;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use rand::RngExt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use url::Url;

/// Distributed website stress testing: every device on your network joins as a load generator.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Crawl the target, serve the dashboard and coordinate the fleet.
    Coordinator(CoordinatorArgs),
    /// Join a coordinator and generate load.
    Worker {
        /// Coordinator URL, e.g. http://192.168.1.10:4000
        #[arg(long, env = "STREZZ_COORDINATOR", value_parser = http_url)]
        coordinator: Url,
        #[arg(long, env = "STREZZ_TOKEN")]
        token: String,
        /// Name shown on the dashboard (defaults to the hostname).
        #[arg(long)]
        name: Option<String>,
        /// Concurrent users this worker offers (default: 1000 per CPU core,
        /// at most 16000). The open-file limit is the ceiling either way.
        #[arg(long, env = "STREZZ_MAX_USERS", value_parser = max_users)]
        max_users: Option<MaxUsers>,
    },
    /// Legacy mode: crawl the target and write a Locust file (what strezz 0.1 did).
    Locustfile {
        #[arg(long, env = "url", value_parser = http_url)]
        url: Url,
        #[arg(long, short, default_value = "loc.py")]
        output: std::path::PathBuf,
    },
}

#[derive(Args)]
struct CoordinatorArgs {
    /// Site under test (only test sites you own or are allowed to test).
    #[arg(long, env = "url", value_parser = http_url)]
    url: Url,
    /// Address to listen on; 0.0.0.0 makes it reachable from the LAN.
    #[arg(long, default_value = "0.0.0.0:4000")]
    listen: SocketAddr,
    /// Join token devices need (random if omitted or empty).
    #[arg(long, env = "STREZZ_TOKEN")]
    token: Option<String>,
    /// Don't run a load-generating worker inside the coordinator process.
    #[arg(long)]
    no_local_worker: bool,
    /// Concurrent users the coordinator's own worker offers (default: 1000
    /// per CPU core, at most 16000).
    #[arg(long, env = "STREZZ_MAX_USERS", value_parser = max_users)]
    max_users: Option<MaxUsers>,
    /// Extra paths to hit besides the crawled ones (repeatable).
    #[arg(long = "path")]
    extra_paths: Vec<String>,
    /// Leave out crawled paths containing this text, e.g. /logout or .zip
    /// (repeatable). Paths given with --path are always kept.
    #[arg(long)]
    exclude: Vec<String>,
    /// Don't crawl the target: test only / and the --path values.
    #[arg(long)]
    no_crawl: bool,
    /// URL phones and other machines use to reach this coordinator, e.g.
    /// http://192.168.1.4:4000 (auto-detected from the LAN interface if omitted;
    /// set it when running in Docker or behind a tunnel).
    #[arg(long, env = "STREZZ_PUBLIC_URL", value_parser = http_url)]
    public_url: Option<Url>,
}

/// Parse an `http://` or `https://` URL.
///
/// Anything else would only fail later and far from its cause: `host:4000`
/// without a scheme, for instance, parses as a URL whose scheme is `host`.
fn http_url(s: &str) -> Result<Url, String> {
    let hint = "expected a URL starting with http:// or https://";
    let url = Url::parse(s).map_err(|e| format!("{e}; {hint}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(hint.into());
    }
    Ok(url)
}

/// `--max-users` / `STREZZ_MAX_USERS`: a number, or nothing for "the default".
#[derive(Clone, Copy, Debug, PartialEq)]
struct MaxUsers(Option<u32>);

/// An empty value counts as not set, like an empty token: a blank
/// `STREZZ_MAX_USERS=` line in `.env` should not stop the program.
fn max_users(s: &str) -> Result<MaxUsers, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(MaxUsers(None));
    }
    s.parse()
        .map(|n| MaxUsers(Some(n)))
        .map_err(|_| format!("expected a whole number of users, got '{s}'"))
}

/// After a crawl of `asked` ended at `landed`: the address to test, which is
/// `landed` when a redirect stayed on the site (http -> https, apex -> www).
///
/// A redirect to a different site is refused. The fleet would otherwise load
/// an address nobody named, a third-party login page for instance, and a
/// warning on a terminal that nobody is watching does not prevent that.
fn after_redirect(asked: &Url, landed: Url) -> Result<Url> {
    if landed.origin() == asked.origin() {
        return Ok(asked.clone());
    }
    if !crawl::same_site(asked, &landed) {
        bail!(
            "{asked} redirects to {landed}, which is a different site.\n\
             Strezz only loads the site you name. If that address is the one to test, pass it as --url."
        );
    }
    println!("note: {asked} redirects to {landed}; testing that address");
    Ok(landed)
}

fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

fn random_token() -> String {
    let mut rng = rand::rng();
    (0..12)
        .map(|_| format!("{:x}", rng.random::<u8>() % 16))
        .collect()
}

/// What the coordinator prints for people joining the fleet.
struct JoinInfo {
    dashboard: String,
    join: String,
    worker_command: String,
}

/// Build the links and the worker command for `token`, which the user may
/// have chosen freely: it is percent-encoded in URLs and quoted for a shell.
fn join_info(base: &str, token: &str) -> JoinInfo {
    let query: String = url::form_urlencoded::byte_serialize(token.as_bytes()).collect();
    JoinInfo {
        dashboard: format!("{base}/?token={query}"),
        join: format!("{base}/join?token={query}"),
        worker_command: format!(
            "strezz worker --coordinator {base} --token {}",
            shell_quote(token)
        ),
    }
}

/// Quote `s` for a POSIX shell, unless it is plainly safe as it stands.
fn shell_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.,:/@%+=".contains(&b));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Address the coordinator's own worker uses to reach it: the address it
/// listens on, or loopback of the same family when that is "all interfaces".
fn local_url(listening_on: SocketAddr) -> Result<Url> {
    let ip = match listening_on.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    let addr = SocketAddr::new(ip, listening_on.port());
    Url::parse(&format!("http://{addr}/")).with_context(|| format!("no usable URL for {addr}"))
}

/// Decide what to test: the address (which a redirect may change) and the paths.
async fn plan_target(args: &CoordinatorArgs) -> Result<(Url, Vec<String>)> {
    let mut target = args.url.clone();
    let crawled = if args.no_crawl {
        vec!["/".to_string()]
    } else {
        match crawl::discover_paths(&args.url).await {
            Ok(crawl) => {
                target = after_redirect(&args.url, crawl.final_url)?;
                crawl.paths
            }
            Err(e) => {
                eprintln!(
                    "warning: could not crawl {} ({e:#}); testing / only",
                    args.url
                );
                vec!["/".to_string()]
            }
        }
    };
    let mut paths = crawl::exclude(crawled, &args.exclude);
    for path in &args.extra_paths {
        if crawl::stays_on_site(&target, path) {
            paths.push(path.clone());
        } else {
            eprintln!(
                "warning: skipping --path {path}: it does not lead to {}",
                target.origin().ascii_serialization()
            );
        }
    }
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        eprintln!("warning: --exclude left no paths; testing / only");
        paths.push("/".to_string());
    }
    Ok((target, paths))
}

async fn run_coordinator(args: CoordinatorArgs) -> Result<()> {
    let (target, paths) = plan_target(&args).await?;
    println!("target {target}: {} paths", paths.len());

    // An empty token (a blank `STREZZ_TOKEN=` line in .env, say) would be no
    // protection at all, so it counts as "not set".
    if args.token.as_deref().is_some_and(|t| t.trim().is_empty()) {
        eprintln!("warning: the token given is empty; using a random one instead");
    }
    let token = args
        .token
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(random_token);
    let state = Arc::new(Mutex::new(coordinator::Coordinator::new(
        token.clone(),
        target.to_string(),
        paths,
    )));

    let listen = args.listen;
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    let local = listener.local_addr()?;
    let port = local.port();
    let detected: Vec<String> = netaddr::lan_addresses()
        .iter()
        .map(|ip| format!("http://{ip}:{port}"))
        .collect();
    let public_url = args.public_url;
    let base = match &public_url {
        Some(u) => u.as_str().trim_end_matches('/').to_string(),
        None => detected
            .first()
            .cloned()
            .unwrap_or_else(|| format!("http://127.0.0.1:{port}")),
    };
    if listen.ip().is_loopback() {
        eprintln!("warning: listening on {listen} only; use --listen 0.0.0.0:{port} so phones can connect");
    }
    if public_url.is_none() && netaddr::in_container() {
        eprintln!(
            "warning: running in a container, {base} is an internal address phones can't reach.\n         \
             Set STREZZ_PUBLIC_URL=http://<your-computer-LAN-IP>:{port} (or use host networking)."
        );
    } else if public_url.is_none() && detected.is_empty() {
        eprintln!("warning: no LAN address found; connect to Wi-Fi/Ethernet or pass --public-url");
    }
    {
        let mut c = state.lock().unwrap();
        c.public_base.clone_from(&base);
        c.lan_urls.clone_from(&detected);
        if !c.lan_urls.contains(&base) {
            c.lan_urls.insert(0, base.clone());
        }
    }

    let info = join_info(&base, &token);
    println!("\n  dashboard   {}", info.dashboard);
    println!("  phones      {}", info.join);
    println!("  machines    {}\n", info.worker_command);
    if let Ok(qr) = qrcode::QrCode::new(info.join.as_bytes()) {
        println!(
            "{}",
            qr.render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build()
        );
    }
    if detected.len() > 1 && public_url.is_none() {
        println!(
            "  other addresses of this machine: {}",
            detected[1..].join(", ")
        );
    }
    println!(
        "  phones must be on the same Wi-Fi; allow incoming connections if the firewall asks.\n"
    );

    tokio::spawn(coordinator::housekeeping(state.clone()));
    if !args.no_local_worker {
        let opts = worker::WorkerOptions {
            coordinator: local_url(local)?,
            token,
            name: format!("{} (coordinator)", hostname()),
            max_users: args.max_users.and_then(|m| m.0),
            lost_after: worker::LOST_AFTER,
        };
        tokio::spawn(async move {
            if let Err(e) = worker::run(opts).await {
                eprintln!("local worker stopped: {e:#}");
            }
        });
    }
    axum::serve(listener, coordinator::router(state)).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Coordinator(args) => run_coordinator(args).await?,
        Cmd::Worker {
            coordinator,
            token,
            name,
            max_users,
        } => {
            worker::run(worker::WorkerOptions {
                coordinator,
                token,
                name: name.unwrap_or_else(hostname),
                max_users: max_users.and_then(|m| m.0),
                lost_after: worker::LOST_AFTER,
            })
            .await?;
        }
        Cmd::Locustfile { url, output } => {
            let crawl = crawl::discover_paths(&url).await?;
            after_redirect(&url, crawl.final_url)?;
            let paths = crawl.paths;
            std::fs::write(&output, locust::generate_locustfile(&paths))?;
            println!("wrote {} tasks to {}", paths.len(), output.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn only_http_urls_are_accepted() {
        assert!(http_url("http://192.168.1.4:4000").is_ok());
        assert!(http_url("https://your-site.example/shop?x=1").is_ok());
        // A forgotten scheme must not slip through as a URL with scheme "localhost".
        assert!(http_url("localhost:4000").is_err());
        assert!(http_url("192.168.1.4:4000").is_err());
        assert!(http_url("ftp://example.com/").is_err());
        assert!(http_url("your-site.example").is_err());
    }

    #[test]
    fn an_empty_max_users_means_the_default() {
        assert_eq!(max_users(""), Ok(MaxUsers(None)));
        assert_eq!(max_users("  "), Ok(MaxUsers(None)));
        assert_eq!(max_users("2000"), Ok(MaxUsers(Some(2000))));
        assert!(max_users("lots").is_err());
        assert!(max_users("-5").is_err());
    }

    #[test]
    fn a_redirect_is_followed_within_the_site_and_refused_beyond_it() {
        let url = |s: &str| Url::parse(s).unwrap();
        let asked = url("http://example.com/");
        // No redirect, or one that stays on the same origin: test what was asked for.
        assert_eq!(
            after_redirect(&asked, url("http://example.com/en/")).unwrap(),
            asked
        );
        // http -> https and apex -> www: test where it landed.
        let landed = url("https://www.example.com/");
        assert_eq!(after_redirect(&asked, landed.clone()).unwrap(), landed);
        // Another site: refuse, and say how to test it on purpose.
        let refusal = after_redirect(&asked, url("https://sso.other.net/login")).unwrap_err();
        assert!(
            refusal.to_string().contains("pass it as --url"),
            "{refusal}"
        );
    }

    #[test]
    fn the_local_worker_dials_the_address_actually_listened_on() {
        let url = |addr: &str| local_url(addr.parse().unwrap()).unwrap().to_string();
        assert_eq!(url("0.0.0.0:4000"), "http://127.0.0.1:4000/");
        assert_eq!(url("[::]:4000"), "http://[::1]:4000/");
        assert_eq!(url("192.168.1.4:4001"), "http://192.168.1.4:4001/");
        assert_eq!(url("127.0.0.1:5000"), "http://127.0.0.1:5000/");
    }

    #[test]
    fn tokens_are_encoded_in_links_and_quoted_in_commands() {
        let plain = join_info("http://192.168.1.4:4000", "3f9a0c1d7e2b");
        assert_eq!(
            plain.join,
            "http://192.168.1.4:4000/join?token=3f9a0c1d7e2b"
        );
        assert_eq!(
            plain.worker_command,
            "strezz worker --coordinator http://192.168.1.4:4000 --token 3f9a0c1d7e2b"
        );

        let odd = join_info("http://192.168.1.4:4000", "p&ss#1 it's");
        // What a browser or the server reads back from the link is the token itself.
        let link = Url::parse(&odd.join).unwrap();
        let read_back = link.query_pairs().find(|(k, _)| k == "token").unwrap().1;
        assert_eq!(read_back, "p&ss#1 it's");
        assert_eq!(link.fragment(), None);
        assert!(odd.dashboard.ends_with("/?token=p%26ss%231+it%27s"));
        assert!(odd.worker_command.ends_with(r"--token 'p&ss#1 it'\''s'"));
    }

    #[test]
    fn generated_tokens_are_never_empty() {
        let token = random_token();
        assert_eq!(token.len(), 12);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
