//! The coordinator: owns the test plan, splits virtual users across every
//! connected device according to its capacity, and merges their stats.

use crate::protocol::*;
use crate::stats::Stats;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A worker that hasn't synced for this long gets no users.
const ALIVE_AFTER: Duration = Duration::from_secs(5);
/// ...and is forgotten after this long.
const FORGET_AFTER: Duration = Duration::from_secs(30);
const SYNC_INTERVAL_MS: u64 = 1000;
/// Seconds of per-second history kept for the chart.
const TIMELINE_SECS: usize = 600;

#[derive(Debug)]
struct Worker {
    id: String,
    name: String,
    kind: WorkerKind,
    platform: String,
    capacity: u32,
    joined: Instant,
    last_seen: Instant,
    last_sync: Instant,
    active_users: u32,
    rps: f64,
    totals: Stats,
}

#[derive(Debug, Serialize, Clone, Copy, Default)]
struct Point {
    t: u64,
    requests: u64,
    failures: u64,
    /// `None` for a second in which no request completed.
    p95_ms: Option<f64>,
}

#[derive(Debug)]
struct Run {
    id: u64,
    config: StartRun,
    started: Instant,
    ended: Option<Instant>,
    stats: Stats,
    /// The same totals split by worker type. Native workers time a request
    /// until the whole body has arrived; browser workers can only time it
    /// until the response headers arrive, and that time includes waiting for
    /// one of the browser's few connections per host. Merged, the two would
    /// pass for one measurement, so the dashboard also shows them apart.
    native: Stats,
    browser: Stats,
    /// Per-second buckets, keyed by seconds since start.
    per_sec: BTreeMap<u64, Stats>,
}

impl Run {
    fn active(&self) -> bool {
        self.ended.is_none()
    }
    fn elapsed(&self) -> Duration {
        self.ended.unwrap_or_else(Instant::now) - self.started
    }
}

#[derive(Default)]
pub struct Coordinator {
    pub token: String,
    pub target: String,
    pub paths: Vec<String>,
    /// LAN URL of the coordinator, shown on the dashboard for joining devices.
    pub public_base: String,
    /// Every LAN URL this machine is reachable on, best first.
    pub lan_urls: Vec<String>,
    workers: HashMap<String, Worker>,
    run: Option<Run>,
    next_run_id: u64,
}

pub type Shared = Arc<Mutex<Coordinator>>;

impl Coordinator {
    pub fn new(token: String, target: String, paths: Vec<String>) -> Self {
        Self {
            token,
            target,
            paths,
            next_run_id: 1,
            ..Default::default()
        }
    }

    fn alive(&self) -> impl Iterator<Item = &Worker> {
        let now = Instant::now();
        self.workers
            .values()
            .filter(move |w| now - w.last_seen < ALIVE_AFTER)
    }

    fn fleet_capacity(&self) -> u64 {
        self.alive().map(|w| w.capacity as u64).sum()
    }

    /// Users per alive worker for the current run.
    fn allocation(&self) -> HashMap<String, u32> {
        let Some(run) = self.run.as_ref().filter(|r| r.active()) else {
            return HashMap::new();
        };
        let mut fleet: Vec<(String, u32)> =
            self.alive().map(|w| (w.id.clone(), w.capacity)).collect();
        fleet.sort();
        let total = if run.config.users == 0 {
            u32::MAX
        } else {
            run.config.users
        };
        allocate(total, &fleet)
    }

    fn assignment_for(&self, worker_id: &str) -> Assignment {
        let Some(run) = self.run.as_ref().filter(|r| r.active()) else {
            return Assignment::idle();
        };
        let alloc = self.allocation();
        let users = alloc.get(worker_id).copied().unwrap_or(0);
        let allocated: u64 = alloc.values().map(|n| *n as u64).sum();
        let share = if allocated == 0 {
            0.0
        } else {
            users as f64 / allocated as f64
        };
        Assignment {
            run_id: run.id,
            target: self.target.clone(),
            paths: self.paths.clone(),
            users,
            // Its share of the fleet-wide spawn rate, so the fleet as a whole
            // ramps at the rate that was asked for and every device finishes
            // ramping at the same moment. The floor of one user per second
            // only keeps a device with a tiny share from crawling.
            spawn_per_sec: (run.config.spawn_rate * share).max(1.0),
            wait_min_ms: run.config.wait_min_ms,
            wait_max_ms: run.config.wait_max_ms.max(run.config.wait_min_ms),
            timeout_ms: run.config.timeout_ms,
        }
    }

    pub fn register(&mut self, req: RegisterRequest) -> RegisterResponse {
        let id: String = {
            let mut rng = rand::rng();
            (0..8)
                .map(|_| format!("{:x}", rng.random::<u8>() % 16))
                .collect()
        };
        let now = Instant::now();
        println!(
            "+ worker {} joined: {} ({:?}, {}) capacity {} users",
            id, req.name, req.kind, req.platform, req.capacity
        );
        self.workers.insert(
            id.clone(),
            Worker {
                id: id.clone(),
                name: req.name,
                kind: req.kind,
                platform: req.platform,
                capacity: req.capacity.max(1),
                joined: now,
                last_seen: now,
                last_sync: now,
                active_users: 0,
                rps: 0.0,
                totals: Stats::default(),
            },
        );
        RegisterResponse {
            worker_id: id,
            sync_interval_ms: SYNC_INTERVAL_MS,
        }
    }

    /// Returns `None` for unknown workers (they must re-register).
    pub fn sync(&mut self, req: SyncRequest) -> Option<Assignment> {
        let now = Instant::now();
        let w = self.workers.get_mut(&req.worker_id)?;
        let kind = w.kind;
        let dt = (now - w.last_sync).as_secs_f64().max(0.001);
        w.last_seen = now;
        w.last_sync = now;
        w.active_users = req.active_users;
        if let Some(cap) = req.capacity.filter(|c| *c > 0) {
            w.capacity = cap;
        }
        w.rps = req.delta.requests as f64 / dt;
        w.totals.merge(&req.delta);

        if let Some(run) = self.run.as_mut() {
            // Late deltas from a just-stopped run still count; stale run ids don't.
            if req.run_id == run.id {
                run.stats.merge(&req.delta);
                match kind {
                    WorkerKind::Native => run.native.merge(&req.delta),
                    WorkerKind::Browser => run.browser.merge(&req.delta),
                }
                // Record the second even when nothing completed in it. An
                // empty second is information too: leaving it out would make
                // a slow test look faster than it is (see `status`).
                let sec = (now - run.started).as_secs();
                run.per_sec.entry(sec).or_default().merge(&req.delta);
                while run.per_sec.len() > TIMELINE_SECS {
                    run.per_sec.pop_first();
                }
            }
        }
        Some(self.assignment_for(&req.worker_id))
    }

    pub fn start(&mut self, cfg: StartRun) -> u64 {
        let id = self.next_run_id;
        self.next_run_id += 1;
        println!(
            "> run {id}: {} users @ {}/s across {} workers (capacity {}), duration {}",
            if cfg.users == 0 {
                "max".to_string()
            } else {
                cfg.users.to_string()
            },
            cfg.spawn_rate,
            self.alive().count(),
            self.fleet_capacity(),
            if cfg.duration_secs == 0 {
                "until stopped".into()
            } else {
                format!("{}s", cfg.duration_secs)
            }
        );
        self.run = Some(Run {
            id,
            config: cfg,
            started: Instant::now(),
            ended: None,
            stats: Stats::default(),
            native: Stats::default(),
            browser: Stats::default(),
            per_sec: BTreeMap::new(),
        });
        id
    }

    pub fn stop(&mut self) {
        if let Some(run) = self.run.as_mut().filter(|r| r.active()) {
            run.ended = Some(Instant::now());
            let s = &run.stats;
            println!(
                "■ run {} stopped after {:.0}s: {} requests, {} failures, p50 {:.0}ms p95 {:.0}ms p99 {:.0}ms",
                run.id,
                run.elapsed().as_secs_f64(),
                s.requests,
                s.failures,
                s.percentile(50.0),
                s.percentile(95.0),
                s.percentile(99.0)
            );
        }
    }

    /// Periodic housekeeping: enforce run duration, forget dead workers.
    pub fn tick(&mut self) {
        let now = Instant::now();
        if let Some(run) = self.run.as_ref().filter(|r| r.active()) {
            let d = run.config.duration_secs;
            if d > 0 && run.elapsed() >= Duration::from_secs(d) {
                self.stop();
            }
        }
        self.workers.retain(|_, w| {
            let keep = now - w.last_seen < FORGET_AFTER;
            if !keep {
                println!("- worker {} ({}) left", w.id, w.name);
            }
            keep
        });
    }

    pub fn status(&self) -> serde_json::Value {
        let now = Instant::now();
        let alloc = self.allocation();
        let mut workers: Vec<_> = self.workers.values().collect();
        workers.sort_by_key(|w| w.joined);
        let workers: Vec<_> = workers
            .iter()
            .map(|w| {
                json!({
                    "id": w.id, "name": w.name, "kind": w.kind, "platform": w.platform,
                    "capacity": w.capacity, "assigned": alloc.get(&w.id).copied().unwrap_or(0),
                    "active_users": w.active_users, "rps": w.rps,
                    "alive": now - w.last_seen < ALIVE_AFTER,
                    "requests": w.totals.requests, "failures": w.totals.failures,
                })
            })
            .collect();

        let run = self.run.as_ref().map(|r| {
            let elapsed = r.elapsed().as_secs();
            // Average over the last 5 complete seconds (the current one is
            // partial), counting every second a worker reported in, including
            // those in which no request completed.
            let window: Vec<&Stats> = r
                .per_sec
                .range(elapsed.saturating_sub(5)..elapsed.max(1))
                .map(|(_, s)| s)
                .collect();
            let span = window.len().max(1) as f64;
            let rps = window.iter().map(|s| s.requests).sum::<u64>() as f64 / span;
            let fps = window.iter().map(|s| s.failures).sum::<u64>() as f64 / span;
            let timeline: Vec<Point> = r
                .per_sec
                .iter()
                .map(|(t, s)| Point {
                    t: *t,
                    requests: s.requests,
                    failures: s.failures,
                    p95_ms: (s.requests > 0).then(|| s.percentile(95.0)),
                })
                .collect();
            let mut errors: Vec<_> = r.stats.errors.iter().collect();
            errors.sort_by(|a, b| b.1.cmp(a.1));
            errors.truncate(10);
            let latency = |s: &Stats| {
                json!({
                    "requests": s.requests,
                    "p50_ms": s.percentile(50.0), "p95_ms": s.percentile(95.0),
                    "p99_ms": s.percentile(99.0),
                })
            };
            json!({
                "id": r.id, "active": r.active(), "elapsed_secs": elapsed, "config": r.config,
                "requests": r.stats.requests, "failures": r.stats.failures, "bytes": r.stats.bytes,
                "rps": rps, "failures_per_sec": fps,
                "mean_ms": r.stats.mean_ms(),
                "p50_ms": r.stats.percentile(50.0), "p95_ms": r.stats.percentile(95.0),
                "p99_ms": r.stats.percentile(99.0),
                "by_kind": { "native": latency(&r.native), "browser": latency(&r.browser) },
                "active_users": self.alive().map(|w| w.active_users as u64).sum::<u64>(),
                "errors": errors, "timeline": timeline,
            })
        });

        json!({
            "target": self.target, "paths": self.paths,
            "public_base": self.public_base,
            "lan_urls": self.lan_urls,
            "fleet_capacity": self.fleet_capacity(),
            "workers": workers, "run": run,
        })
    }
}

/// Split `total` users across the fleet by water-filling: every device gets an
/// equal share, capped at its own capacity, and whatever a small device (a
/// phone) can't hold flows to devices with room left. Spreading load evenly
/// also spreads CPU and network bottlenecks, not just file descriptors.
/// Deterministic for a given input, so assignments are stable between syncs.
pub fn allocate(total: u32, fleet: &[(String, u32)]) -> HashMap<String, u32> {
    let mut out: HashMap<String, u32> = fleet.iter().map(|(id, _)| (id.clone(), 0)).collect();
    let mut left = total as u64;
    let mut open: Vec<&(String, u32)> = fleet.iter().filter(|(_, c)| *c > 0).collect();
    while left > 0 && !open.is_empty() {
        let share = (left / open.len() as u64).max(1);
        let mut still_open = Vec::new();
        for w in open {
            if left == 0 {
                break;
            }
            let got = out.get_mut(&w.0).unwrap();
            let give = share.min(w.1 as u64 - *got as u64).min(left);
            *got += give as u32;
            left -= give;
            if *got < w.1 {
                still_open.push(w);
            }
        }
        open = still_open;
    }
    out
}

// ---------------------------------------------------------------- HTTP layer

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

fn authorized(state: &Shared, headers: &HeaderMap, q: &TokenQuery) -> bool {
    let expected = state.lock().unwrap().token.clone();
    let given = headers
        .get(TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| q.token.clone());
    // An empty token must never grant access, however the coordinator was
    // started: it is exactly what a browser sends when the dashboard is
    // opened without `?token=`.
    !expected.is_empty() && given.as_deref() == Some(expected.as_str())
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "missing or wrong strezz token").into_response()
}

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/", get(dashboard))
        .route("/join", get(join_page))
        .route("/api/qr.svg", get(qr_svg))
        .route("/api/register", post(register))
        .route("/api/sync", post(sync))
        .route("/api/status", get(status))
        .route("/api/run/start", post(start))
        .route("/api/run/stop", post(stop))
        .with_state(state)
}

async fn dashboard() -> Html<&'static str> {
    Html(include_str!("../web/dashboard.html"))
}

async fn join_page() -> Html<&'static str> {
    Html(include_str!("../web/join.html"))
}

#[derive(Deserialize)]
struct QrQuery {
    data: String,
}

async fn qr_svg(Query(q): Query<QrQuery>) -> Response {
    match qrcode::QrCode::new(q.data.as_bytes()) {
        Ok(code) => {
            let svg = code
                .render::<qrcode::render::svg::Color>()
                .min_dimensions(220, 220)
                .quiet_zone(true)
                .build();
            ([(header::CONTENT_TYPE, "image/svg+xml")], svg).into_response()
        }
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}

async fn register(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
    Json(req): Json<RegisterRequest>,
) -> Response {
    if !authorized(&state, &headers, &q) {
        return unauthorized();
    }
    Json(state.lock().unwrap().register(req)).into_response()
}

async fn sync(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
    Json(req): Json<SyncRequest>,
) -> Response {
    if !authorized(&state, &headers, &q) {
        return unauthorized();
    }
    match state.lock().unwrap().sync(req) {
        Some(a) => Json(a).into_response(),
        None => (StatusCode::NOT_FOUND, "unknown worker, register again").into_response(),
    }
}

async fn status(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Response {
    if !authorized(&state, &headers, &q) {
        return unauthorized();
    }
    Json(state.lock().unwrap().status()).into_response()
}

async fn start(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
    Json(cfg): Json<StartRun>,
) -> Response {
    if !authorized(&state, &headers, &q) {
        return unauthorized();
    }
    let cfg = match cfg.validated() {
        Ok(cfg) => cfg,
        Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
    };
    let mut c = state.lock().unwrap();
    c.stop();
    let id = c.start(cfg);
    Json(json!({ "run_id": id })).into_response()
}

async fn stop(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Response {
    if !authorized(&state, &headers, &q) {
        return unauthorized();
    }
    state.lock().unwrap().stop();
    StatusCode::NO_CONTENT.into_response()
}

/// Run the housekeeping loop forever.
pub async fn housekeeping(state: Shared) {
    let mut every = tokio::time::interval(Duration::from_millis(500));
    loop {
        every.tick().await;
        state.lock().unwrap().tick();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fleet(caps: &[u32]) -> Vec<(String, u32)> {
        caps.iter()
            .enumerate()
            .map(|(i, c)| (format!("w{i}"), *c))
            .collect()
    }

    #[test]
    fn allocation_spreads_evenly_and_respects_small_devices() {
        let a = allocate(1000, &fleet(&[10_000, 6, 30_000]));
        assert_eq!(a.values().sum::<u32>(), 1000);
        assert_eq!(a["w1"], 6);
        assert_eq!(a["w0"], 497);
        assert_eq!(a["w2"], 497);
    }

    #[test]
    fn allocation_never_exceeds_capacity() {
        let a = allocate(1_000_000, &fleet(&[100, 6, 250]));
        assert_eq!(a["w0"], 100);
        assert_eq!(a["w1"], 6);
        assert_eq!(a["w2"], 250);
    }

    #[test]
    fn allocation_handles_small_and_empty() {
        let a = allocate(2, &fleet(&[6, 6, 6]));
        assert_eq!(a.values().sum::<u32>(), 2);
        assert!(allocate(10, &[]).is_empty());
    }

    #[test]
    fn sync_flow_assigns_and_aggregates() {
        let mut c = Coordinator::new("t".into(), "http://x/".into(), vec!["/".into()]);
        let r = |name: &str, cap| RegisterRequest {
            name: name.into(),
            kind: WorkerKind::Native,
            capacity: cap,
            platform: "test".into(),
        };
        let a = c.register(r("laptop", 300)).worker_id;
        let b = c.register(r("phone", 100)).worker_id;
        let sync = |c: &mut Coordinator, id: &str, run_id, n| {
            let mut delta = Stats::default();
            for _ in 0..n {
                delta.record(12.0, 10, None);
            }
            c.sync(SyncRequest {
                worker_id: id.into(),
                run_id,
                active_users: 0,
                delta,
                capacity: None,
            })
            .unwrap()
        };
        assert_eq!(sync(&mut c, &a, 0, 0).run_id, 0);

        let run = c.start(StartRun {
            users: 200,
            spawn_rate: 20.0,
            duration_secs: 0,
            wait_min_ms: 0,
            wait_max_ms: 0,
            timeout_ms: 1000,
        });
        let asg_a = sync(&mut c, &a, 0, 0);
        let asg_b = sync(&mut c, &b, 0, 0);
        assert_eq!((asg_a.users, asg_b.users), (100, 100));
        assert_eq!(asg_a.run_id, run);

        sync(&mut c, &a, run, 7);
        sync(&mut c, &b, run, 3);
        sync(&mut c, &b, run - 1, 99); // stale run, ignored for run stats
        assert_eq!(c.run.as_ref().unwrap().stats.requests, 10);

        assert!(c
            .sync(SyncRequest {
                worker_id: "nope".into(),
                run_id: 0,
                active_users: 0,
                delta: Stats::default(),
                capacity: None
            })
            .is_none());
        c.stop();
        assert_eq!(sync(&mut c, &a, run, 0).users, 0);
    }

    #[test]
    fn full_capacity_mode_follows_live_capacity() {
        let mut c = Coordinator::new("t".into(), "http://x/".into(), vec!["/".into()]);
        let phone = c
            .register(RegisterRequest {
                name: "phone".into(),
                kind: WorkerKind::Browser,
                capacity: 12,
                platform: "test".into(),
            })
            .worker_id;
        let laptop = c
            .register(RegisterRequest {
                name: "laptop".into(),
                kind: WorkerKind::Native,
                capacity: 500,
                platform: "test".into(),
            })
            .worker_id;
        c.start(StartRun {
            users: 0,
            spawn_rate: 100.0,
            duration_secs: 0,
            wait_min_ms: 0,
            wait_max_ms: 0,
            timeout_ms: 1000,
        });
        let mut sync = |id: &str, capacity| {
            c.sync(SyncRequest {
                worker_id: id.into(),
                run_id: 1,
                active_users: 0,
                delta: Stats::default(),
                capacity,
            })
            .unwrap()
        };
        assert_eq!(sync(&phone, None).users, 12);
        assert_eq!(sync(&laptop, None).users, 500);
        // The phone's auto-tune raises its capacity; it gets all of it.
        let a = sync(&phone, Some(400));
        assert_eq!(a.users, 400);
        assert!(a.spawn_per_sec >= 20.0, "spawn={}", a.spawn_per_sec);
    }

    fn coordinator() -> Coordinator {
        Coordinator::new("t".into(), "http://x/".into(), vec!["/".into()])
    }

    fn join(c: &mut Coordinator, kind: WorkerKind, capacity: u32) -> String {
        let req = RegisterRequest {
            name: format!("{kind:?}"),
            kind,
            capacity,
            platform: "test".into(),
        };
        c.register(req).worker_id
    }

    fn run_with(users: u32, spawn_rate: f64) -> StartRun {
        StartRun {
            users,
            spawn_rate,
            duration_secs: 0,
            wait_min_ms: 0,
            wait_max_ms: 0,
            timeout_ms: 1000,
        }
    }

    /// Stats for `requests` requests that each took `latency_ms`.
    fn sent(requests: u64, latency_ms: f64) -> Stats {
        let mut stats = Stats::default();
        for _ in 0..requests {
            stats.record(latency_ms, 0, None);
        }
        stats
    }

    fn report(c: &mut Coordinator, id: &str, run_id: u64, delta: Stats) -> Assignment {
        c.sync(SyncRequest {
            worker_id: id.into(),
            run_id,
            active_users: 0,
            delta,
            capacity: None,
        })
        .unwrap()
    }

    #[test]
    fn the_configured_spawn_rate_is_shared_out_not_overridden() {
        let mut c = coordinator();
        let big = join(&mut c, WorkerKind::Native, 100_000);
        let run = c.start(run_with(10_000, 50.0));
        // One device: the whole rate, however many users are waiting to start.
        let a = report(&mut c, &big, run, Stats::default());
        assert_eq!((a.users, a.spawn_per_sec), (10_000, 50.0));

        // Two equal devices: half each, so the fleet still adds 50 per second.
        let other = join(&mut c, WorkerKind::Native, 100_000);
        assert_eq!(
            report(&mut c, &big, run, Stats::default()).spawn_per_sec,
            25.0
        );
        assert_eq!(
            report(&mut c, &other, run, Stats::default()).spawn_per_sec,
            25.0
        );
    }

    #[test]
    fn a_device_with_a_tiny_share_still_ramps() {
        let mut c = coordinator();
        let phone = join(&mut c, WorkerKind::Browser, 6);
        join(&mut c, WorkerKind::Native, 100_000);
        let run = c.start(run_with(10_000, 50.0));
        let a = report(&mut c, &phone, run, Stats::default());
        assert_eq!((a.users, a.spawn_per_sec), (6, 1.0));
    }

    #[test]
    fn slow_tests_are_not_reported_faster_than_they_are() {
        let mut c = coordinator();
        let w = join(&mut c, WorkerKind::Native, 100);
        let run = c.start(run_with(1, 1.0));
        // A sync that carries no completed request still records its second.
        report(&mut c, &w, run, Stats::default());
        assert_eq!(c.run.as_ref().unwrap().per_sec.len(), 1);

        // Ten seconds in, with one request in the last five reported seconds.
        let r = c.run.as_mut().unwrap();
        r.started = Instant::now() - Duration::from_secs(10);
        r.per_sec = (5..10)
            .map(|sec| (sec, sent(u64::from(sec == 7), 20.0)))
            .collect();
        let status = c.status();
        assert_eq!(status["run"]["rps"], 0.2);
        // No latency is made up for the seconds in which nothing completed.
        let timeline = status["run"]["timeline"].as_array().unwrap();
        assert!(timeline[0]["p95_ms"].is_null());
        assert!(timeline[2]["p95_ms"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn native_and_browser_latency_are_also_reported_apart() {
        let mut c = coordinator();
        let native = join(&mut c, WorkerKind::Native, 100);
        let browser = join(&mut c, WorkerKind::Browser, 100);
        let run = c.start(run_with(10, 10.0));
        report(&mut c, &native, run, sent(10, 400.0));
        report(&mut c, &browser, run, sent(30, 40.0));

        let status = c.status();
        let by_kind = &status["run"]["by_kind"];
        assert_eq!(by_kind["native"]["requests"], 10);
        assert_eq!(by_kind["browser"]["requests"], 30);
        assert!(by_kind["native"]["p50_ms"].as_f64().unwrap() > 300.0);
        assert!(by_kind["browser"]["p50_ms"].as_f64().unwrap() < 100.0);
        assert_eq!(status["run"]["requests"], 40);
    }

    // ------------------------------------------------------------ HTTP layer

    use crate::testutil::TestServer;

    async fn served(token: &str) -> (TestServer, Shared) {
        let state: Shared = Arc::new(Mutex::new(Coordinator::new(
            token.into(),
            "http://x/".into(),
            vec!["/".into()],
        )));
        (TestServer::start(router(state.clone())).await, state)
    }

    async fn code(request: reqwest::RequestBuilder) -> u16 {
        request.send().await.unwrap().status().as_u16()
    }

    #[tokio::test]
    async fn the_api_needs_the_token_but_the_pages_do_not() {
        let (server, _state) = served("secret").await;
        let http = reqwest::Client::new();
        let status = server.url("/api/status");

        assert_eq!(code(http.get(&status)).await, 401);
        assert_eq!(
            code(http.get(&status).header(TOKEN_HEADER, "guess")).await,
            401
        );
        assert_eq!(
            code(http.get(server.url("/api/status?token=guess"))).await,
            401
        );
        assert_eq!(code(http.post(server.url("/api/run/stop"))).await, 401);

        assert_eq!(
            code(http.get(&status).header(TOKEN_HEADER, "secret")).await,
            200
        );
        assert_eq!(
            code(http.get(server.url("/api/status?token=secret"))).await,
            200
        );

        // The pages carry no data; they ask the API for it with the token.
        assert_eq!(code(http.get(server.url("/"))).await, 200);
        assert_eq!(code(http.get(server.url("/join"))).await, 200);
    }

    #[tokio::test]
    async fn an_empty_token_never_opens_the_api() {
        // However a coordinator ends up with an empty token, "no token" must
        // not match it: that is what a browser sends without `?token=`.
        let (server, _state) = served("").await;
        let http = reqwest::Client::new();
        let status = server.url("/api/status");

        assert_eq!(code(http.get(&status).header(TOKEN_HEADER, "")).await, 401);
        assert_eq!(code(http.get(server.url("/api/status?token="))).await, 401);
        assert_eq!(code(http.get(&status)).await, 401);
    }

    #[tokio::test]
    async fn starting_a_run_checks_its_settings() {
        let (server, state) = served("secret").await;
        let http = reqwest::Client::new();
        let start = |body: serde_json::Value| {
            http.post(server.url("/api/run/start"))
                .header(TOKEN_HEADER, "secret")
                .json(&body)
        };

        let refused = start(json!({ "users": 10, "spawn_rate": 0 }))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 400);
        assert!(refused.text().await.unwrap().contains("spawn_rate"));
        assert_eq!(
            code(start(json!({ "users": 10, "timeout_ms": 0 }))).await,
            400
        );
        assert!(
            state.lock().unwrap().run.is_none(),
            "nothing may have started"
        );

        let started = start(json!({ "users": 10 })).send().await.unwrap();
        assert_eq!(started.status(), 200);
        let body: serde_json::Value = started.json().await.unwrap();
        assert_eq!(body["run_id"], 1);
        assert!(state.lock().unwrap().run.as_ref().unwrap().active());
    }
}
