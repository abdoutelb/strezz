# Strezz

Distributed website stress testing, written in Rust. Any device on your network can join the
load-generating fleet: laptops, servers, Raspberry Pis, and even phones through their browser.

> Only stress test sites you own or have permission to test.

## Why a fleet

On a single machine, every concurrent virtual user holds an open socket, so the total load is
capped by the open-file limit (`ulimit -n`), and by that machine's CPU and network. Strezz splits
the load across every device that joins:

```
                     ┌──────────────────────────────┐
                     │  strezz coordinator          │
   dashboard  ◀──────│  crawl target · split users  │
                     │  merge stats                 │
                     └──────┬──────────┬────────┬───┘
               POST /api/sync (1/s): stats ⇄ assignment
          ┌─────────────────┘          │        └──────────────┐
   native worker                 native worker            phone browser
   (fd limit → capacity)         (Termux, Pi, …)          (/join page, no install)
          └──────────────┬─────────────┴───────────────────────┘
                         ▼
                  site under test
```

- **Capacity.** Each native worker raises its own `RLIMIT_NOFILE` as far as the OS allows (up to
  `kern.maxfilesperproc` on macOS, `fs.nr_open` on Linux). It then reports how many concurrent
  users it can hold. A browser worker reports what you type on its page, 6 by default.
- **Allocation.** The coordinator splits the total users with equal-share water-filling. Every
  device gets the same share, capped at its own capacity, and whatever a small device can't hold
  goes to the others. Devices that stop syncing for 5 seconds get their users reassigned.
- **Stats.** Every worker records latencies into the same 64-bucket log histogram and sends
  deltas once per second. The coordinator adds them up to compute fleet-wide RPS, failure rate
  and p50/p95/p99 latency.
- **Protocol.** The protocol is plain HTTP and JSON, polled by the worker. Anything that can run
  `fetch()` can join, and nothing needs an inbound port except the coordinator.

## Quick start

```sh
cargo build --release
./target/release/strezz coordinator --url https://your-site.example/
```

The coordinator prints:

```
  dashboard   http://192.168.1.4:4000/?token=3f9a…
  phones      http://192.168.1.4:4000/join?token=3f9a…     (also as a QR code)
  machines    strezz worker --coordinator http://192.168.1.4:4000 --token 3f9a…
```

1. Open the **dashboard**.
2. On each phone or tablet, scan the QR code and tap **Join fleet**. Keep the screen on; the page
   requests a wake lock.
3. On each extra machine, run the **worker** command.
4. On the dashboard, set total users, spawn rate, duration and think time, then press **Start**.

The coordinator also runs a local worker. Pass `--no-local-worker` to keep it as a pure
controller.

### Phone can't connect?

- The phone must be on the **same Wi-Fi** as the coordinator, not on mobile data or a guest
  network that isolates clients.
- The phone URL uses the machine's LAN address (for example `192.168.x.x`), never `localhost`.
  Strezz skips loopback, Docker, VPN and link-local interfaces when it picks this address. If it
  picks the wrong one, set it yourself:
  `strezz coordinator --url … --public-url http://192.168.1.4:4000` (or `STREZZ_PUBLIC_URL=…`).
- The dashboard has an **Address** picker if the machine has several LAN addresses. If you open
  the dashboard through a LAN address instead of `localhost`, the QR code uses that address.
- **Docker:** a container only sees its own internal IP, so put
  `STREZZ_PUBLIC_URL=http://<your LAN IP>:4000` in `.env`.
- **macOS:** click *Allow* when asked about incoming connections. Otherwise, check System
  Settings → Network → Firewall.
- **Devices outside your network** need a tunnel (for example `cloudflared tunnel --url
  http://localhost:4000`). Pass the tunnel URL as `--public-url`.

### Other ways to join

| Device | How |
|---|---|
| macOS / Linux / Windows | `strezz worker --coordinator … --token …` (optionally `--max-users N`) |
| Android (more power than the browser) | Install Termux, then `pkg install rust && cargo install --git https://github.com/abdoutelb/strezz`, then run `strezz worker …` |
| iPhone / any browser | Open the `/join?token=…` URL |
| Docker | `docker run --ulimit nofile=1048576:1048576 strezz worker --coordinator … --token …` |

### Browser worker limits

Browsers can't read cross-origin responses, so requests use `fetch(..., {mode: "no-cors"})`. The
target still receives real load, but a browser worker can only count network errors and timeouts,
not HTTP status codes. Browsers also open about 6 connections per host over HTTP/1.1. If the
target speaks HTTP/2 or HTTP/3, raise the capacity on the join page.

## Docker Compose

```sh
echo "url=https://your-site.example/" > .env
echo "STREZZ_PUBLIC_URL=http://<your LAN IP>:4000" >> .env   # e.g. from: ipconfig getifaddr en0
docker compose up
# dashboard: the URL printed in the logs (use your machine's LAN IP for phones)
```

## Legacy Locust mode

Strezz 0.1 only generated a Locust file. To get that file:

```sh
strezz locustfile --url https://your-site.example/ -o loc.py
locust -f loc.py --host https://your-site.example/
```

## HTTP API

All endpoints need the token, either in the `x-strezz-token` header or as `?token=`.

| Method | Path | Purpose |
|---|---|---|
| POST | `/api/register` | `{name, kind: native\|browser, capacity, platform}` → `{worker_id}` |
| POST | `/api/sync` | `{worker_id, run_id, active_users, delta}` → assignment |
| POST | `/api/run/start` | `{users, spawn_rate, duration_secs, wait_min_ms, wait_max_ms, timeout_ms}` |
| POST | `/api/run/stop` | stop the current run |
| GET | `/api/status` | fleet, run stats, per-second timeline |

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

| File | What it does |
|---|---|
| `src/coordinator.rs` | Fleet state, user allocation, stats aggregation, HTTP API |
| `src/worker.rs` | Native load engine: one connection pool per virtual user, ramp-up, think time |
| `src/limits.rs` | Open-file limit raising and capacity |
| `src/netaddr.rs` | Picks the LAN address that phones should use |
| `src/stats.rs` | Mergeable latency histogram (the same formula is in `web/join.html`) |
| `src/crawl.rs` | Link discovery (port of `extracter.js`) |
| `src/locust.rs` | Locust file generation (port of `helpers.js`) |
| `web/` | Dashboard and browser worker, embedded into the binary |
