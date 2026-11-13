mod coordinator;
mod crawl;
mod limits;
mod locust;
mod protocol;
mod stats;
mod worker;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rand::Rng;
use std::net::SocketAddr;
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
    Coordinator {
        /// Site under test (only test sites you own or are allowed to test).
        #[arg(long, env = "url")]
        url: Url,
        /// Address to listen on; 0.0.0.0 makes it reachable from the LAN.
        #[arg(long, default_value = "0.0.0.0:4000")]
        listen: SocketAddr,
        /// Join token devices need (random if omitted).
        #[arg(long, env = "STREZZ_TOKEN")]
        token: Option<String>,
        /// Don't run a load-generating worker inside the coordinator process.
        #[arg(long)]
        no_local_worker: bool,
        /// Extra paths to hit besides the crawled ones (repeatable).
        #[arg(long = "path")]
        extra_paths: Vec<String>,
    },
    /// Join a coordinator and generate load.
    Worker {
        /// Coordinator URL, e.g. http://192.168.1.10:4000
        #[arg(long, env = "STREZZ_COORDINATOR")]
        coordinator: Url,
        #[arg(long, env = "STREZZ_TOKEN")]
        token: String,
        /// Name shown on the dashboard (defaults to the hostname).
        #[arg(long)]
        name: Option<String>,
        /// Cap concurrent users below what the open-file limit allows.
        #[arg(long)]
        max_users: Option<u32>,
    },
    /// Legacy mode: crawl the target and write a Locust file (what strezz 0.1 did).
    Locustfile {
        #[arg(long, env = "url")]
        url: Url,
        #[arg(long, short, default_value = "loc.py")]
        output: std::path::PathBuf,
    },
}

fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Coordinator { url, listen, token, no_local_worker, extra_paths } => {
            let mut paths = match crawl::discover_paths(&url).await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("warning: could not crawl {url} ({e:#}); testing / only");
                    vec!["/".to_string()]
                }
            };
            paths.extend(extra_paths);
            paths.sort();
            paths.dedup();
            println!("target {url}: {} paths", paths.len());

            let token = token.unwrap_or_else(|| {
                let mut rng = rand::thread_rng();
                (0..12).map(|_| format!("{:x}", rng.gen::<u8>() % 16)).collect()
            });
            let state = Arc::new(Mutex::new(coordinator::Coordinator::new(
                token.clone(),
                url.to_string(),
                paths,
            )));

            let listener = tokio::net::TcpListener::bind(listen)
                .await
                .with_context(|| format!("binding {listen}"))?;
            let port = listener.local_addr()?.port();
            let lan = local_ip_address::local_ip()
                .map(|ip| ip.to_string())
                .unwrap_or_else(|_| "127.0.0.1".into());
            let base = format!("http://{lan}:{port}");
            let join = format!("{base}/join?token={token}");
            state.lock().unwrap().public_base = base.clone();

            println!("\n  dashboard   {base}/?token={token}");
            println!("  phones      {join}");
            println!("  machines    strezz worker --coordinator {base} --token {token}\n");
            if let Ok(qr) = qrcode::QrCode::new(join.as_bytes()) {
                println!("{}", qr.render::<qrcode::render::unicode::Dense1x2>().quiet_zone(true).build());
            }

            tokio::spawn(coordinator::housekeeping(state.clone()));
            if !no_local_worker {
                let opts = worker::WorkerOptions {
                    coordinator: Url::parse(&format!("http://127.0.0.1:{port}/"))?,
                    token: token.clone(),
                    name: format!("{} (coordinator)", hostname()),
                    max_users: None,
                };
                tokio::spawn(async move {
                    if let Err(e) = worker::run(opts).await {
                        eprintln!("local worker stopped: {e:#}");
                    }
                });
            }
            axum::serve(listener, coordinator::router(state)).await?;
        }
        Cmd::Worker { coordinator, token, name, max_users } => {
            worker::run(worker::WorkerOptions {
                coordinator,
                token,
                name: name.unwrap_or_else(hostname),
                max_users,
            })
            .await?;
        }
        Cmd::Locustfile { url, output } => {
            let paths = crawl::discover_paths(&url).await?;
            std::fs::write(&output, locust::generate_locustfile(&paths))?;
            println!("wrote {} tasks to {}", paths.len(), output.display());
        }
    }
    Ok(())
}
