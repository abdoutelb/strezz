//! Native load-generating worker. Runs on any machine that can execute the
//! binary (macOS, Linux, Windows, Android via Termux, Raspberry Pi...).

use crate::limits;
use crate::protocol::*;
use crate::stats::Stats;
use anyhow::{bail, Context, Result};
use rand::seq::IndexedRandom;
use rand::RngExt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use url::Url;

pub struct WorkerOptions {
    pub coordinator: Url,
    pub token: String,
    pub name: String,
    /// Optional hard cap below the fd-derived capacity.
    pub max_users: Option<u32>,
}

struct Shared {
    stats: Mutex<Stats>,
    active: AtomicU32,
}

/// One running virtual user.
struct User(JoinHandle<()>);

impl Drop for User {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn run(opts: WorkerOptions) -> Result<()> {
    let fd_limit = limits::raise_fd_limit();
    let mut capacity = limits::capacity_from_fd_limit(fd_limit);
    if let Some(max) = opts.max_users {
        capacity = capacity.min(max.max(1));
    }
    println!(
        "worker '{}': open-file limit {fd_limit} -> capacity {capacity} concurrent users",
        opts.name
    );

    let control = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let api = |path: &str| opts.coordinator.join(path).expect("valid api path");
    let platform = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);

    let shared = Arc::new(Shared {
        stats: Mutex::new(Stats::default()),
        active: AtomicU32::new(0),
    });
    let mut users: Vec<User> = Vec::new();
    let mut current = Assignment::idle();
    let mut worker_id: Option<String> = None;
    let mut last_tick = Instant::now();
    let mut spawn_budget = 0.0f64;
    let mut backoff = Duration::from_millis(500);

    loop {
        // (Re-)register whenever we don't have an id (startup, coordinator restart).
        let id = match &worker_id {
            Some(id) => id.clone(),
            None => {
                let req = RegisterRequest {
                    name: opts.name.clone(),
                    kind: WorkerKind::Native,
                    capacity,
                    platform: platform.clone(),
                };
                match post::<_, RegisterResponse>(&control, api("api/register"), &opts.token, &req)
                    .await
                {
                    Ok(resp) => {
                        println!("registered with {} as {}", opts.coordinator, resp.worker_id);
                        backoff = Duration::from_millis(500);
                        worker_id = Some(resp.worker_id.clone());
                        resp.worker_id
                    }
                    Err(e) => {
                        eprintln!("cannot reach coordinator: {e:#}; retrying in {backoff:?}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(10));
                        continue;
                    }
                }
            }
        };

        let delta = std::mem::take(&mut *shared.stats.lock().unwrap());
        let req = SyncRequest {
            worker_id: id,
            run_id: current.run_id,
            active_users: shared.active.load(Ordering::Relaxed),
            delta,
            capacity: None,
        };
        let next = match post::<_, Assignment>(&control, api("api/sync"), &opts.token, &req).await {
            Ok(a) => a,
            Err(e) => {
                if e.to_string().contains("404") {
                    worker_id = None;
                } else {
                    eprintln!("sync failed: {e:#}");
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        // New run (or back to idle): drop every user of the old one.
        if next.run_id != current.run_id {
            users.clear();
            spawn_budget = 0.0;
            if next.run_id != 0 {
                println!(
                    "run {}: {} users against {}",
                    next.run_id, next.users, next.target
                );
            } else {
                println!("idle");
            }
        }
        let dt = last_tick.elapsed().as_secs_f64();
        last_tick = Instant::now();

        let want = next.users as usize;
        if users.len() > want {
            users.truncate(want);
        } else if users.len() < want && next.run_id != 0 {
            spawn_budget = (spawn_budget + next.spawn_per_sec * dt).min(want as f64);
            let add = (spawn_budget.floor() as usize).min(want - users.len());
            spawn_budget -= add as f64;
            let plan = Arc::new(next.clone());
            let target = Url::parse(&next.target).context("bad target url from coordinator")?;
            for _ in 0..add {
                let handle =
                    tokio::spawn(virtual_user(target.clone(), plan.clone(), shared.clone()));
                users.push(User(handle));
            }
        }
        current = next;
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
}

async fn post<T: serde::Serialize, R: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: Url,
    token: &str,
    body: &T,
) -> Result<R> {
    let resp = client
        .post(url)
        .header(TOKEN_HEADER, token)
        .json(body)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        bail!(
            "{} {}",
            status.as_u16(),
            resp.text().await.unwrap_or_default()
        );
    }
    Ok(resp.json().await?)
}

/// Decrements the active counter even when the task is aborted.
struct ActiveGuard(Arc<Shared>);
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// One virtual user: its own connection pool (like a Locust `HttpUser`
/// session), hitting random paths with think time in between.
async fn virtual_user(target: Url, plan: Arc<Assignment>, shared: Arc<Shared>) {
    shared.active.fetch_add(1, Ordering::Relaxed);
    let _guard = ActiveGuard(shared.clone());
    let client = match reqwest::Client::builder()
        .user_agent(concat!("strezz/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_millis(plan.timeout_ms.max(100)))
        .pool_max_idle_per_host(1)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            shared
                .stats
                .lock()
                .unwrap()
                .record(0.0, 0, Some(format!("client: {e}")));
            return;
        }
    };
    let paths: Vec<Url> = plan
        .paths
        .iter()
        .filter_map(|p| target.join(p).ok())
        .collect();
    if paths.is_empty() {
        return;
    }

    // Stagger the first request so a ramp doesn't fire in lockstep.
    let jitter = rand::rng().random_range(0..=plan.wait_max_ms.min(1000));
    tokio::time::sleep(Duration::from_millis(jitter)).await;

    loop {
        let url = paths.choose(&mut rand::rng()).unwrap().clone();
        let started = Instant::now();
        let (bytes, error) = match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                // Read the body fully so the connection is reused, like a browser would.
                match resp.bytes().await {
                    Ok(b) if status.as_u16() < 400 => (b.len() as u64, None),
                    Ok(b) => (b.len() as u64, Some(format!("HTTP {}", status.as_u16()))),
                    Err(e) => (0, Some(classify(&e))),
                }
            }
            Err(e) => (0, Some(classify(&e))),
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        shared.stats.lock().unwrap().record(ms, bytes, error);

        let wait = if plan.wait_max_ms > plan.wait_min_ms {
            rand::rng().random_range(plan.wait_min_ms..=plan.wait_max_ms)
        } else {
            plan.wait_min_ms
        };
        if wait > 0 {
            tokio::time::sleep(Duration::from_millis(wait)).await;
        } else {
            tokio::task::yield_now().await;
        }
    }
}

fn classify(e: &reqwest::Error) -> String {
    let text = format!("{e:?}");
    if text.contains("Too many open files") || text.contains("os error 24") {
        "EMFILE (open-file limit)".into()
    } else if e.is_timeout() {
        "timeout".into()
    } else if e.is_connect() {
        "connect error".into()
    } else {
        // Report the innermost cause, e.g. "connection reset by peer".
        let mut root: &dyn std::error::Error = e;
        while let Some(next) = root.source() {
            root = next;
        }
        let mut msg = root.to_string();
        msg.truncate(80);
        msg
    }
}
