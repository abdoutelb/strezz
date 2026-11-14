//! Native load-generating worker. Runs on any machine that can execute the
//! binary (macOS, Linux, Windows, Android via Termux, Raspberry Pi...).

use crate::limits;
use crate::protocol::*;
use crate::stats::Stats;
use anyhow::{bail, Context, Result};
use rand::seq::IndexedRandom;
use rand::RngExt;
use reqwest::StatusCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use url::Url;

/// A worker that has not completed a sync for this long stops all its virtual
/// users. Without this, a coordinator that crashes, is closed or drops off the
/// network would leave the fleet loading the target with nobody able to stop it.
pub const LOST_AFTER: Duration = Duration::from_secs(10);

/// Timeout for calls to the coordinator. Well below `LOST_AFTER`, so a call
/// that hangs cannot hold back the cut-off for long.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounds on the sync interval the coordinator asks for.
const SYNC_INTERVAL_MS: std::ops::RangeInclusive<u64> = 100..=5_000;

const USER_AGENT: &str = concat!("strezz/", env!("CARGO_PKG_VERSION"));

/// Failure labels longer than this are cut, so one odd error message cannot
/// flood the dashboard.
const MAX_ERROR_LABEL: usize = 80;

/// OS error codes that mean "out of file descriptors".
#[cfg(unix)]
const FD_EXHAUSTED: &[i32] = &[libc::EMFILE, libc::ENFILE];
#[cfg(not(unix))]
const FD_EXHAUSTED: &[i32] = &[10024]; // WSAEMFILE

pub struct WorkerOptions {
    pub coordinator: Url,
    pub token: String,
    pub name: String,
    /// Concurrent users this worker offers. `None` uses a default sized from
    /// the CPU count. Either way the open-file limit is the ceiling.
    pub max_users: Option<u32>,
    /// Stop every user after this long without a successful sync.
    pub lost_after: Duration,
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

/// What every virtual user of a run needs, parsed once and shared by all of
/// them instead of copied into each.
struct Plan {
    urls: Vec<Url>,
    wait_min_ms: u64,
    wait_max_ms: u64,
    timeout: Duration,
}

impl Plan {
    fn new(assignment: &Assignment) -> Result<Self> {
        let target = Url::parse(&assignment.target).context("bad target url from coordinator")?;
        let urls: Vec<Url> = assignment
            .paths
            .iter()
            .filter_map(|p| target.join(p).ok())
            .collect();
        if urls.is_empty() {
            bail!("the coordinator sent no usable paths for {target}");
        }
        Ok(Self {
            urls,
            wait_min_ms: assignment.wait_min_ms,
            wait_max_ms: assignment.wait_max_ms.max(assignment.wait_min_ms),
            timeout: Duration::from_millis(assignment.timeout_ms.max(MIN_TIMEOUT_MS)),
        })
    }
}

/// The users this worker is running and the assignment they belong to.
#[derive(Default)]
struct Running {
    assignment: Assignment,
    plan: Option<Arc<Plan>>,
    users: Vec<User>,
    /// Users the spawn rate has earned but that have not been started yet.
    spawn_budget: f64,
}

impl Running {
    /// Drop every user and forget the run.
    fn stop(&mut self) {
        self.users.clear();
        self.plan = None;
        self.spawn_budget = 0.0;
        self.assignment = Assignment::idle();
    }

    /// Move towards `next`: drop the users of a finished run, shed surplus
    /// users, and start new ones no faster than the assignment's spawn rate.
    /// `dt` is the time since the previous call, in seconds.
    fn apply(&mut self, next: Assignment, dt: f64, shared: &Arc<Shared>) -> Result<()> {
        if next.run_id != self.assignment.run_id {
            self.stop();
            if next.run_id != 0 {
                println!(
                    "run {}: {} users against {}",
                    next.run_id, next.users, next.target
                );
            } else {
                println!("idle");
            }
        }
        let want = next.users as usize;
        if self.users.len() > want {
            self.users.truncate(want);
        } else if self.users.len() < want && next.run_id != 0 {
            let plan = match &self.plan {
                Some(plan) => plan.clone(),
                None => {
                    let plan = Arc::new(Plan::new(&next)?);
                    self.plan = Some(plan.clone());
                    plan
                }
            };
            self.spawn_budget = (self.spawn_budget + next.spawn_per_sec * dt).min(want as f64);
            let add = (self.spawn_budget.floor() as usize).min(want - self.users.len());
            self.spawn_budget -= add as f64;
            for _ in 0..add {
                let user = tokio::spawn(virtual_user(plan.clone(), shared.clone()));
                self.users.push(User(user));
            }
        }
        self.assignment = next;
        Ok(())
    }
}

/// Why a call to the coordinator failed.
#[derive(Debug)]
enum ApiError {
    /// The coordinator answered, but not with success.
    Status(StatusCode, String),
    /// No usable answer: connection refused, timeout, unreadable body...
    Transport(reqwest::Error),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Status(status, body) => write!(f, "{} {body}", status.as_u16()),
            Self::Transport(e) => {
                // reqwest's own message stops at "error sending request"; the
                // causes below it say why (refused, timed out, DNS...).
                write!(f, "{e}")?;
                let mut source = std::error::Error::source(e);
                while let Some(cause) = source {
                    write!(f, ": {cause}")?;
                    source = cause.source();
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        Self::Transport(e)
    }
}

const TOKEN_REJECTED: &str = "the coordinator rejected the join token (401). \
     Stopping; start this worker again with the token the coordinator printed";

pub async fn run(opts: WorkerOptions) -> Result<()> {
    let fd_limit = limits::raise_fd_limit();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let capacity = limits::native_capacity(fd_limit, cores, opts.max_users);
    let by_fds = limits::capacity_from_fd_limit(fd_limit);
    let reason = if capacity == by_fds {
        format!("all that the open-file limit of {fd_limit} allows")
    } else if opts.max_users.is_some() {
        format!("--max-users; the open-file limit allows {by_fds}")
    } else {
        format!("default for {cores} CPU cores; --max-users can raise it to {by_fds}")
    };
    println!(
        "worker '{}': capacity {capacity} concurrent users ({reason})",
        opts.name
    );

    let control = reqwest::Client::builder()
        .timeout(CONTROL_TIMEOUT)
        .build()?;
    let api = |path: &str| opts.coordinator.join(path).expect("valid api path");
    let platform = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);

    let shared = Arc::new(Shared {
        stats: Mutex::new(Stats::default()),
        active: AtomicU32::new(0),
    });
    let mut running = Running::default();
    let mut worker_id: Option<String> = None;
    let mut sync_interval = Duration::from_secs(1);
    let mut last_tick = Instant::now();
    let mut last_contact = Instant::now();
    let mut backoff = Duration::from_millis(500);

    loop {
        // Dead man's switch: never keep loading the target on the strength of
        // an assignment the coordinator can no longer take back.
        if !running.users.is_empty() && last_contact.elapsed() >= opts.lost_after {
            eprintln!(
                "no answer from the coordinator for {:.0?}: stopping all {} users",
                last_contact.elapsed(),
                running.users.len()
            );
            running.stop();
        }

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
                        last_contact = Instant::now();
                        sync_interval = Duration::from_millis(
                            resp.sync_interval_ms
                                .clamp(*SYNC_INTERVAL_MS.start(), *SYNC_INTERVAL_MS.end()),
                        );
                        worker_id = Some(resp.worker_id.clone());
                        resp.worker_id
                    }
                    Err(ApiError::Status(StatusCode::UNAUTHORIZED, _)) => bail!(TOKEN_REJECTED),
                    Err(e) => {
                        // While users are running, come back quickly so the
                        // cut-off above is checked on time.
                        let pause = if running.users.is_empty() {
                            backoff
                        } else {
                            backoff.min(Duration::from_secs(1))
                        };
                        eprintln!(
                            "cannot register with the coordinator: {e}; retrying in {pause:?}"
                        );
                        tokio::time::sleep(pause).await;
                        backoff = (backoff * 2).min(Duration::from_secs(10));
                        continue;
                    }
                }
            }
        };

        let delta = std::mem::take(&mut *shared.stats.lock().unwrap());
        let req = SyncRequest {
            worker_id: id,
            run_id: running.assignment.run_id,
            active_users: shared.active.load(Ordering::Relaxed),
            delta,
            capacity: None,
        };
        let next = match post::<_, Assignment>(&control, api("api/sync"), &opts.token, &req).await {
            Ok(next) => next,
            Err(e) => {
                // Keep the stats for the next sync instead of losing them. If
                // the coordinator did get this request and only its answer was
                // lost they are counted twice, which is rarer and does less
                // harm than dropping every interval that hits a network blip.
                shared.stats.lock().unwrap().merge(&req.delta);
                match e {
                    ApiError::Status(StatusCode::UNAUTHORIZED, _) => bail!(TOKEN_REJECTED),
                    ApiError::Status(StatusCode::NOT_FOUND, _) => {
                        // It restarted, or dropped us after a long silence.
                        // Either way it is not tracking what we are running.
                        println!("the coordinator no longer knows this worker: registering again");
                        running.stop();
                        worker_id = None;
                    }
                    e => eprintln!("sync failed: {e}"),
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        last_contact = Instant::now();

        // After a gap (failed syncs, a stop) do not release the users the
        // spawn rate "earned" meanwhile in one burst: ramp again instead.
        let dt = last_tick.elapsed().min(sync_interval * 3).as_secs_f64();
        last_tick = Instant::now();
        running.apply(next, dt, &shared)?;
        tokio::time::sleep(sync_interval).await;
    }
}

async fn post<T: serde::Serialize, R: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: Url,
    token: &str,
    body: &T,
) -> Result<R, ApiError> {
    let resp = client
        .post(url)
        .header(TOKEN_HEADER, token)
        .json(body)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ApiError::Status(
            status,
            resp.text().await.unwrap_or_default(),
        ));
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
async fn virtual_user(plan: Arc<Plan>, shared: Arc<Shared>) {
    shared.active.fetch_add(1, Ordering::Relaxed);
    let _guard = ActiveGuard(shared.clone());
    let client = match reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(plan.timeout)
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

    // Stagger the first request so a ramp doesn't fire in lockstep.
    let jitter = rand::rng().random_range(0..=plan.wait_max_ms.min(1000));
    tokio::time::sleep(Duration::from_millis(jitter)).await;

    loop {
        let url = plan
            .urls
            .choose(&mut rand::rng())
            .expect("a plan has at least one url")
            .clone();
        let started = Instant::now();
        let (bytes, error) = fetch(&client, url).await;
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

/// One GET. Returns the bytes received and, if it failed, why.
async fn fetch(client: &reqwest::Client, url: Url) -> (u64, Option<String>) {
    let mut resp = match client.get(url).send().await {
        Ok(resp) => resp,
        Err(e) => return (0, Some(classify(&e))),
    };
    let status = resp.status();
    // Read the body to the end so the connection is reused, like a browser
    // would, but count it chunk by chunk rather than holding it: a crawled
    // link to a large download would otherwise cost its full size in memory
    // for every user fetching it.
    let mut bytes = 0u64;
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => bytes += chunk.len() as u64,
            Ok(None) => break,
            Err(e) => return (bytes, Some(classify(&e))),
        }
    }
    let error = (status.as_u16() >= 400).then(|| format!("HTTP {}", status.as_u16()));
    (bytes, error)
}

/// A short, stable label for a failed request, for the dashboard's top errors.
fn classify(e: &reqwest::Error) -> String {
    classify_parts(e.is_timeout(), e.is_connect(), e)
}

fn classify_parts(
    is_timeout: bool,
    is_connect: bool,
    error: &(dyn std::error::Error + 'static),
) -> String {
    // Walk to the innermost cause, e.g. "connection reset by peer", checking
    // on the way whether an OS error says we ran out of file descriptors.
    let mut root = error;
    let mut out_of_fds = false;
    loop {
        if let Some(io) = root.downcast_ref::<std::io::Error>() {
            out_of_fds |= io.raw_os_error().is_some_and(|c| FD_EXHAUSTED.contains(&c));
        }
        match root.source() {
            Some(cause) => root = cause,
            None => break,
        }
    }
    if out_of_fds {
        "EMFILE (open-file limit)".into()
    } else if is_timeout {
        "timeout".into()
    } else if is_connect {
        "connect error".into()
    } else {
        truncated(root.to_string(), MAX_ERROR_LABEL)
    }
}

/// Cut `s` to at most `max` bytes without splitting a character (OS error
/// text is localized on some systems, so it is not always ASCII).
fn truncated(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::{self, Coordinator};
    use crate::testutil::TestServer;
    use axum::Router;
    use std::sync::atomic::AtomicU64;

    /// An error with a cause, to build the chains reqwest produces.
    #[derive(Debug)]
    struct Wrapped(&'static str, Box<dyn std::error::Error + 'static>);
    impl std::fmt::Display for Wrapped {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for Wrapped {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(self.1.as_ref())
        }
    }

    #[test]
    fn classify_finds_fd_exhaustion_anywhere_in_the_chain() {
        let os = std::io::Error::from_raw_os_error(FD_EXHAUSTED[0]);
        let e = Wrapped(
            "client error (Connect)",
            Box::new(Wrapped("tcp open error", Box::new(os))),
        );
        // It wins over the generic labels, because it means the worker, not
        // the site, is the bottleneck.
        assert_eq!(classify_parts(false, true, &e), "EMFILE (open-file limit)");
    }

    #[test]
    fn classify_labels_timeouts_connects_and_the_root_cause() {
        let reset = || std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset");
        let e = Wrapped("error sending request", Box::new(reset()));
        assert_eq!(classify_parts(true, false, &e), "timeout");
        assert_eq!(classify_parts(false, true, &e), "connect error");
        assert_eq!(classify_parts(false, false, &e), "connection reset");
    }

    #[test]
    fn long_labels_are_cut_without_splitting_a_character() {
        // 3-byte characters: byte 80 falls inside one, where String::truncate panics.
        let text = "接".repeat(40);
        let e = std::io::Error::other(text);
        let label = classify_parts(false, false, &e);
        assert_eq!(label, "接".repeat(26));
        assert!(label.len() <= MAX_ERROR_LABEL);
        assert_eq!(truncated("short".into(), MAX_ERROR_LABEL), "short");
    }

    fn assignment(run_id: u64, users: u32, spawn_per_sec: f64) -> Assignment {
        Assignment {
            run_id,
            // Port 9 (discard) on loopback: nothing listens, so users fail fast.
            target: "http://127.0.0.1:9/".into(),
            paths: vec!["/".into()],
            users,
            spawn_per_sec,
            wait_min_ms: 1000,
            wait_max_ms: 1000,
            timeout_ms: 1000,
        }
    }

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            stats: Mutex::new(Stats::default()),
            active: AtomicU32::new(0),
        })
    }

    #[tokio::test]
    async fn users_are_added_at_the_spawn_rate_and_dropped_at_once() {
        let shared = shared();
        let mut running = Running::default();

        // 2.5 users per second: 2 after the first second, 5 after the second.
        running.apply(assignment(1, 10, 2.5), 1.0, &shared).unwrap();
        assert_eq!(running.users.len(), 2);
        running.apply(assignment(1, 10, 2.5), 1.0, &shared).unwrap();
        assert_eq!(running.users.len(), 5);
        // Never beyond the assignment, however much time has passed.
        running
            .apply(assignment(1, 10, 2.5), 60.0, &shared)
            .unwrap();
        assert_eq!(running.users.len(), 10);

        // Fewer users wanted: shed immediately.
        running.apply(assignment(1, 4, 2.5), 1.0, &shared).unwrap();
        assert_eq!(running.users.len(), 4);
        // A new run starts from zero and ramps again.
        running.apply(assignment(2, 10, 3.0), 1.0, &shared).unwrap();
        assert_eq!(running.users.len(), 3);
        // Idle: everything stops.
        running.apply(Assignment::idle(), 1.0, &shared).unwrap();
        assert!(running.users.is_empty());
    }

    #[tokio::test]
    async fn an_assignment_without_usable_paths_is_an_error() {
        let mut bad = assignment(1, 5, 5.0);
        bad.target = "mailto:someone@example.com".into();
        assert!(Running::default().apply(bad, 1.0, &shared()).is_err());
    }

    /// A site under test that counts the requests it receives.
    async fn target() -> (TestServer, Arc<AtomicU64>) {
        let hits = Arc::new(AtomicU64::new(0));
        let counter = hits.clone();
        let router = Router::new().fallback(move || {
            counter.fetch_add(1, Ordering::Relaxed);
            async { "ok" }
        });
        (TestServer::start(router).await, hits)
    }

    fn fast_run(users: u32) -> StartRun {
        StartRun {
            users,
            spawn_rate: 100.0,
            duration_secs: 0,
            wait_min_ms: 10,
            wait_max_ms: 30,
            timeout_ms: 2000,
        }
    }

    fn options(coordinator: &TestServer, token: &str) -> WorkerOptions {
        WorkerOptions {
            coordinator: Url::parse(&coordinator.url("/")).unwrap(),
            token: token.into(),
            name: "test-worker".into(),
            max_users: Some(50),
            lost_after: Duration::from_millis(1500),
        }
    }

    /// Wait until `ready` holds, failing the test after ten seconds.
    async fn until(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn worker_count(state: &coordinator::Shared) -> usize {
        state.lock().unwrap().status()["workers"]
            .as_array()
            .map_or(0, Vec::len)
    }

    /// Wait, for at most `within`, until the target has gone a full second
    /// without a request. (How long stopping takes varies: Windows, for one,
    /// needs two seconds to refuse a connection to a closed local port.)
    async fn goes_quiet(hits: &AtomicU64, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            let before = hits.load(Ordering::Relaxed);
            tokio::time::sleep(Duration::from_secs(1)).await;
            if hits.load(Ordering::Relaxed) == before {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn users_stop_when_the_coordinator_goes_away() {
        let (site, hits) = target().await;
        let state: coordinator::Shared = Arc::new(Mutex::new(Coordinator::new(
            "secret".into(),
            site.url("/"),
            vec!["/".into()],
        )));
        let coord = TestServer::start(coordinator::router(state.clone())).await;
        let worker = tokio::spawn(run(options(&coord, "secret")));

        until("the worker to register", || worker_count(&state) == 1).await;
        state.lock().unwrap().start(fast_run(5));
        until("load on the target", || hits.load(Ordering::Relaxed) > 20).await;

        // The coordinator dies mid-run. The worker must give up on its own.
        coord.shutdown().await;
        assert!(
            goes_quiet(&hits, Duration::from_secs(20)).await,
            "the worker kept sending requests after losing the coordinator"
        );
        assert!(
            !worker.is_finished(),
            "the worker should keep trying to reconnect"
        );
        worker.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_restarted_coordinator_stops_the_old_run() {
        let (site, hits) = target().await;
        let fresh = || Coordinator::new("secret".into(), site.url("/"), vec!["/".into()]);
        let state: coordinator::Shared = Arc::new(Mutex::new(fresh()));
        let coord = TestServer::start(coordinator::router(state.clone())).await;
        let worker = tokio::spawn(run(options(&coord, "secret")));

        until("the worker to register", || worker_count(&state) == 1).await;
        state.lock().unwrap().start(fast_run(5));
        until("load on the target", || hits.load(Ordering::Relaxed) > 20).await;

        // Same address and token, but all state gone: what a restart looks like.
        *state.lock().unwrap() = fresh();
        until("the worker to register again", || worker_count(&state) == 1).await;
        assert!(
            goes_quiet(&hits, Duration::from_secs(20)).await,
            "the worker kept running a run the coordinator no longer has"
        );
        worker.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rejected_token_ends_the_worker() {
        let state: coordinator::Shared = Arc::new(Mutex::new(Coordinator::new(
            "secret".into(),
            "http://127.0.0.1:9/".into(),
            vec!["/".into()],
        )));
        let coord = TestServer::start(coordinator::router(state)).await;
        let outcome = tokio::time::timeout(Duration::from_secs(10), run(options(&coord, "wrong")))
            .await
            .expect("a wrong token must not be retried forever");
        let message = outcome.unwrap_err().to_string();
        assert!(message.contains("rejected the join token"), "{message}");
    }
}
