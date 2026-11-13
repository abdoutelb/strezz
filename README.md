# Strezz

[![Rust CI](https://github.com/abdoutelb/strezz/actions/workflows/rust.yml/badge.svg)](https://github.com/abdoutelb/strezz/actions/workflows/rust.yml)
[![CodeQL](https://github.com/abdoutelb/strezz/actions/workflows/codeql.yml/badge.svg)](https://github.com/abdoutelb/strezz/actions/workflows/codeql.yml)

**Distributed website stress testing, written in Rust.** Any device on your network can generate
load: laptops, servers, Raspberry Pis, Android phones running Termux, and any phone or tablet with
a web browser. Open a link or scan a QR code to join; there is nothing to install.

> ⚠️ **Only stress test sites you own or have written permission to test.** A load test is
> indistinguishable from a denial-of-service attack to the site being tested.

---

## Contents

- [Why Strezz](#why-strezz)
- [Features](#features)
- [How it works](#how-it-works)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Adding devices to the fleet](#adding-devices-to-the-fleet)
- [Running a test](#running-a-test)
- [Reading the results](#reading-the-results)
- [Capacity, allocation and auto-tune](#capacity-allocation-and-auto-tune)
- [Networking and troubleshooting](#networking-and-troubleshooting)
- [Docker](#docker)
- [CLI reference](#cli-reference)
- [HTTP API](#http-api)
- [Security](#security)
- [Limitations](#limitations)
- [Legacy Locust mode and migrating from 0.1](#legacy-locust-mode-and-migrating-from-01)
- [Development](#development)
- [FAQ](#faq)

---

## Why Strezz

When you load test from one machine, every concurrent virtual user holds an open socket, and
every socket is a file descriptor. The test hits a ceiling long before the site does:

- **The open-file limit** (`ulimit -n`, often 256 on macOS and 1024 on Linux by default) caps
  concurrent connections.
- **CPU and network** on that one machine cap requests per second.
- **A single source IP** looks nothing like real traffic, and rate limiters or CDNs may treat it
  differently.

Strezz spreads the test across a **fleet** of whatever devices you have. Each device adds its own
file descriptors, CPU and network, and the coordinator combines everything into one live view.

## Features

- 🚀 **One small binary** with no runtime dependencies. It runs as both coordinator and worker.
- 📱 **Zero-install phone workers.** Scan a QR code, tap *Join fleet*, and the browser becomes a
  load generator.
- 📈 **Self-sizing devices.**
  - Native workers raise their open-file limit to the kernel maximum and report capacity from it.
  - Browser workers start from the device's own CPU, memory and network, then **auto-tune** to
    the device's real limit during a run.
- ⚖️ **Fair allocation.** Users are split evenly across devices, capped by each device's capacity,
  and rebalanced live as devices join or leave.
- 🔋 **Full fleet capacity mode.** Every device runs as many users as it can.
- 📊 **Live dashboard.**
  - Fleet-wide requests/s, failure rate, p50/p95/p99 latency.
  - A per-second chart, per-device load, and top errors.
- 🕷️ **Automatic path discovery.** The coordinator crawls the target page for same-site links,
  and you can add extra paths by hand.
- 🔐 **Token-protected.** Only devices with the join token can take part or control runs.
- 🐍 **Locust compatible.** It can still generate a `locustfile.py`, which is all that
  Strezz 0.1 did.

## How it works

```
                         ┌───────────────────────────────────┐
     you (browser) ◀────▶│        strezz coordinator         │
       dashboard         │  crawl target · split users ·     │
                         │  merge stats · serve /join page   │
                         └──────┬───────────┬───────────┬────┘
                                │           │           │      POST /api/sync every 1 s
                                │           │           │      ↑ stats delta  ↓ assignment
                     ┌──────────┘           │           └───────────┐
                     ▼                      ▼                       ▼
             ┌───────────────┐     ┌────────────────┐     ┌──────────────────┐
             │ native worker │     │ native worker  │     │  browser worker  │
             │ laptop/server │     │ Termux / Pi    │     │  phone · tablet  │
             └───────┬───────┘     └───────┬────────┘     └────────┬─────────┘
                     └─────────────────────┼───────────────────────┘
                                           ▼  real HTTP traffic
                                  ┌──────────────────┐
                                  │  site under test │
                                  └──────────────────┘
```

1. **The coordinator** fetches the target URL, collects same-site links as the paths to test, and
   serves the dashboard, the `/join` page and the API on port 4000.
2. **Workers register** with their name, type (native or browser) and **capacity**, which is the
   number of concurrent virtual users they can hold.
3. **Every second**, each worker calls `POST /api/sync`. It sends the stats it collected since the
   last sync and receives its **assignment**: how many users to run, which paths to hit, think
   time and timeout.
4. **Workers generate the traffic themselves**, straight to the target over their own network. The
   coordinator never proxies test traffic; it only hands out work and adds up the numbers.
5. **The coordinator merges** every worker's latency histogram, so fleet-wide percentiles are
   exact to the histogram's resolution.

The protocol is pull-based HTTP + JSON, so:

- Anything that can run `fetch()` can be a worker.
- Only the coordinator needs a reachable port. Workers never accept connections.

### A virtual user

Each virtual user loops through these steps:

1. Pick a random path.
2. Send `GET` and read the whole response.
3. Record its latency and outcome.
4. Sleep for a random think time between *wait min* and *wait max*.

Native workers give every user **its own connection pool**, as each Locust `HttpUser` has its own
session, so 1 user ≈ 1 connection ≈ 1 file descriptor. Users ramp up gradually at the configured
spawn rate.

## Installation

### With Cargo (recommended)

You need a recent stable Rust. Install it with [rustup](https://rustup.rs):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then install Strezz:

```sh
cargo install --git https://github.com/abdoutelb/strezz
strezz --help
```

### Build from a clone

```sh
git clone https://github.com/abdoutelb/strezz
cd strezz
cargo build --release
./target/release/strezz --help
```

The dashboard and join page are embedded in the binary, so you can copy
`target/release/strezz` to any machine with the same OS and CPU type.

### Other platforms

| Platform | How |
|---|---|
| Linux x86_64 / ARM (Raspberry Pi) | `cargo install --git https://github.com/abdoutelb/strezz` on the device, or cross-compile with [`cross`](https://github.com/cross-rs/cross): `cross build --release --target aarch64-unknown-linux-gnu` |
| Windows | `cargo install --git https://github.com/abdoutelb/strezz` |
| Android | [Termux](https://termux.dev): `pkg install rust` then `cargo install --git https://github.com/abdoutelb/strezz` |
| iPhone / iPad | No install. Use the browser worker. |
| Docker | See [Docker](#docker) |

## Quick start

**1. Start the coordinator** on a machine connected to your network:

```sh
strezz coordinator --url https://your-site.example/
```

**2. It prints** where to connect, plus a QR code for phones:

```
target https://your-site.example/: 14 paths

  dashboard   http://192.168.1.4:4000/?token=3f9a0c1d7e2b
  phones      http://192.168.1.4:4000/join?token=3f9a0c1d7e2b
  machines    strezz worker --coordinator http://192.168.1.4:4000 --token 3f9a0c1d7e2b

  ██████████████████████████
  ██ ▄▄▄▄▄ █▀ █▀▀██ ▄▄▄▄▄ ██     ← scan with a phone camera
  ...
  phones must be on the same Wi-Fi; allow incoming connections if the firewall asks.
```

**3. Open the dashboard** link in your browser.

**4. Add devices:**
- Phones and tablets scan the QR code and tap **Join fleet**.
- Other machines run the `strezz worker …` command.

They appear in the **Devices** table within a second.

**5. Start a run.** Set the users and duration, or tick **Full fleet capacity**, and press
**Start**.

The coordinator also runs a worker of its own, so a test works even with no other devices.
Pass `--no-local-worker` to make it a pure controller.

## Adding devices to the fleet

### Phones, tablets, any browser

1. Connect the device to the **same Wi-Fi** as the coordinator.
2. Scan the QR code from the terminal or dashboard, or type the `phones` URL, including
   `http://` and the `?token=…` part.
3. Check the **This device** card. It shows the CPU cores, memory and network the browser
   reports, and a **recommended starting number of users**.
4. Leave **Auto-tune** on to let the page find the device's limit, or untick it and set
   *Max concurrent users* yourself.
5. Tap **Join fleet**. Keep the tab in the foreground with the screen on. The page requests a
   screen wake lock, but mobile browsers throttle background tabs heavily.

To leave, tap **Leave fleet** or close the tab. The coordinator moves that device's users to
other devices within 5 seconds.

### Laptops, servers, Raspberry Pi, Termux

```sh
strezz worker --coordinator http://192.168.1.4:4000 --token 3f9a0c1d7e2b
```

- The worker raises its open-file limit and prints its capacity:
  `open-file limit 61440 -> capacity 61376 concurrent users`.
- Use `--max-users N` to cap it below what the CPU or network can really drive.
- Use `--name "office-pc"` to label it on the dashboard.
- Workers reconnect automatically if the coordinator restarts.

You can also pass settings as environment variables:

```sh
export STREZZ_COORDINATOR=http://192.168.1.4:4000 STREZZ_TOKEN=3f9a0c1d7e2b
strezz worker
```

## Running a test

### Run settings

| Setting | Default | Meaning |
|---|---|---|
| **Total users** | 200 | Concurrent virtual users across the whole fleet. |
| **Full fleet capacity** | off | Ignore *Total users* and give every device as many users as it can hold (API: `users: 0`). |
| **Spawn rate /s** | 50 | Users added per second across the fleet while ramping up. Every device fills its own share within about 20 s, even next to much larger devices. |
| **Duration (s)** | 60 | Stop automatically after this many seconds. `0` runs until you press **Stop**. |
| **Timeout (ms)** | 30000 | A request taking longer than this counts as a failure (`timeout`). |
| **Wait min / max (ms)** | 1000 / 3000 | Think time between two requests from the same user. Set both to `0` for maximum pressure, where each user fires back-to-back. |

Starting a new run stops the current one. Workers drop all users from the old run immediately.

### Choosing numbers

Each user sends roughly `1 / (avg response time + avg think time)` requests per second, so:

- **200 users** with 1–3 s think time ≈ **100 req/s** when the site is fast.
- **To simulate real visitors**, keep a think time and raise the user count. This exercises
  concurrent connections.
- **To find the throughput ceiling**, set the think time to 0 and grow the user count until
  latency or errors climb.

### Which paths are tested

The coordinator loads `--url` and keeps every link on the same site (same scheme, host and port).
It drops fragments (`#…`), removes duplicates, and always includes `/`. Add more with `--path`:

```sh
strezz coordinator --url https://shop.example/ --path /api/products --path "/search?q=shoes"
```

The **Paths** card on the dashboard lists what will be hit. Each request picks a path at random
with equal probability.

## Reading the results

| Dashboard element | Meaning |
|---|---|
| **devices · user capacity** | Devices that synced in the last 5 s, and their combined capacity. |
| **active users** | Users actually running right now. This trails the target during ramp-up. |
| **requests / s** | Fleet-wide throughput, averaged over the last 5 complete seconds. |
| **failure rate** | Failed ÷ total requests for the run. |
| **p50 / p95 / p99** | Latency percentiles across the whole fleet, accurate to about 19% (histogram bucket width). |
| **Chart** | Per-second requests (blue), failures (red) and p95 latency (amber), for the last 3 minutes. |
| **Devices table** | Per-device capacity, load bar (active ÷ capacity), active / assigned users, req/s, totals. Grey dot = device went silent. |
| **Top errors** | Most frequent failure reasons, e.g. `HTTP 503`, `timeout`, `Connection reset by peer`, `EMFILE (open-file limit)`. |

The terminal prints a summary when each run ends:

```
■ run 3 stopped after 60s: 48211 requests, 12 failures, p50 84ms p95 310ms p99 920ms
```

### What counts as a failure

| Worker | Failure when |
|---|---|
| Native | HTTP status ≥ 400, timeout, connection or TLS error, body read error |
| Browser | Network error or timeout only. Browsers hide cross-origin status codes (see [Limitations](#limitations)). |

## Capacity, allocation and auto-tune

### Native capacity

On startup a native worker raises its soft `RLIMIT_NOFILE` as high as the kernel allows. The
per-process maximum is `kern.maxfilesperproc` on macOS and `fs.nr_open` on Linux. It keeps 64
descriptors in reserve:

```
capacity = open-file limit − 64        (optionally capped by --max-users)
```

This is an **upper bound from file descriptors only**. A laptop with a 61k-descriptor limit
cannot actually drive 61k users with zero think time. Use `--max-users` to keep a machine within
what its CPU and network can handle.

### Browser capacity and auto-tune

**Starting point.** The join page computes a recommendation from what the browser reports:

```
start = CPU cores × 8
        × 0.25 / 0.5 / 0.75 / 1   if memory ≤ 1 / ≤ 2 / ≤ 4 / > 4 GB  (Chromium only)
        capped at 12 on 3G, 4 on 2G, halved with Data Saver
        clamped to 6 … 256
```

**Auto-tune.** Auto-tune is on by default. It only acts while the device is running everything it
was given, and measures in 5-second windows:

| Signal in the window | Action |
|---|---|
| Event-loop lag > 80 ms (CPU saturated) | **Back off**: capacity × 0.8 |
| Failures > 5% | **Back off**: capacity × 0.8 |
| Median latency > 2.5× its best **and** +150 ms (requests queueing in the browser) | **Back off**: capacity × 0.8 |
| Requests/s still rising (> +5%) | **Raise**: capacity × 1.25 (at least +2) |
| Requests/s flat | **Hold**, then probe +10% after 4 flat windows |

Each change is reported to the coordinator on the next sync and takes effect within a second. A
status line under the capacity box explains each decision, for example
`↑ 48 → 60: 212 req/s, 95 ms, 3 ms lag` or
`Holding at 60 users: throughput has plateaued at 240 req/s`.

Auto-tune only raises a device's capacity when the run asks it for more than it is doing now. To
push **every** device to its limit, use **Full fleet capacity**.

### Allocation (water-filling)

The coordinator splits *Total users* evenly across live devices. Each device is capped at its
own capacity, and whatever a device can't hold is shared among the devices that still have room:

```
Total users: 1000          laptop (cap 10 000)  phone (cap 6)  server (cap 30 000)
Equal share ≈ 333 each  →  333                  6 (capped)     333
Leftover 328 re-shared  →  497                  6              497
```

Spreading load evenly also spreads CPU and network bottlenecks. Allocation is recomputed on every
sync:
- Devices that join mid-run get users immediately.
- Devices silent for **5 s** stop receiving users and their share is redistributed.
- Devices silent for **30 s** are removed from the list.

## Networking and troubleshooting

Only the **coordinator's port (4000) must be reachable**, and only on your LAN. Do **not** forward
it on your router. To reach devices outside your network, use a tunnel (see below).

```
phone ──Wi-Fi──▶ router ──▶ coordinator's LAN IP :4000   (join page + sync API)
phone ─────────── internet ─────────▶ site under test    (the actual load)
```

### The phone can't open the join page

Work through these in order:

1. **Check the phone's IP.** In Settings → Wi-Fi → your network, it must be in the same range as
   the coordinator, e.g. `192.168.1.x`.
   - If it isn't, the phone is on mobile data, a guest network, or a different SSID.
   - Turn off VPN apps on the phone.
2. **Type the full URL** with `http://`, for example `http://192.168.1.4:4000/join?token=…`.
   - Strezz serves plain HTTP, so `https://…:4000` will never load.
   - Some browsers silently upgrade to HTTPS. Typing `http://` explicitly avoids that.
3. **Check for router client isolation.** If the phone is on the right network but the page times
   out, the router is probably blocking device-to-device traffic.
   - The setting is called *AP isolation*, *client isolation* or *guest mode*.
   - Disable it in the router's admin page, or use the coordinator's **Personal Hotspot** for
     the phones instead.
4. **Check the firewall.** On macOS, click *Allow* when asked about incoming connections, or check
   System Settings → Network → Firewall. On Linux, `sudo ufw allow 4000/tcp`. On Windows, allow
   `strezz.exe` in Windows Defender Firewall.

### The URL shows the wrong address

Strezz picks the coordinator's LAN address automatically. It skips loopback, Docker bridges,
VPN tunnels and link-local addresses, and prefers Wi-Fi/Ethernet interfaces.

- **Several LAN addresses:** the dashboard shows an **Address** picker.
- **Opened through the LAN IP:** if you open the dashboard via the LAN IP instead of `localhost`,
  the QR code uses that address.
- **Override:**

  ```sh
  strezz coordinator --url … --public-url http://192.168.1.4:4000
  # or STREZZ_PUBLIC_URL=http://192.168.1.4:4000
  ```

  Find your IP with `ipconfig getifaddr en0` (macOS), `hostname -I` (Linux) or `ipconfig`
  (Windows).

### Devices outside your network

Expose the coordinator through a tunnel and advertise the tunnel URL:

```sh
cloudflared tunnel --url http://localhost:4000      # prints https://<random>.trycloudflare.com
strezz coordinator --url … --public-url https://<random>.trycloudflare.com
```

### Other symptoms

| Symptom | Cause / fix |
|---|---|
| `EMFILE (open-file limit)` in errors | A worker ran out of descriptors. Lower its users with `--max-users`, or raise the OS limit (Linux: `/etc/security/limits.conf`, Docker: `--ulimit nofile=…`). |
| Lots of `timeout` / `Connection reset by peer` | The **site** (or a rate limiter or WAF in front of it) is saturated. This is often the result you're looking for. |
| Phone req/s stops growing at a low number | Browsers allow about 6 connections per host on HTTP/1.1. Auto-tune detects this and holds there. Use a native worker (Termux on Android) for more. |
| Phone drops out mid-run | The screen locked or the tab went to the background. Keep it in the foreground and plugged in. |
| `bad token` / 401 | The `?token=` in the URL doesn't match the coordinator's token. Rescan the QR code. Set `STREZZ_TOKEN` to keep the token the same across restarts. |
| Dashboard says *coordinator unreachable* | The coordinator process stopped, or the address changed. |

## Docker

```sh
cat > .env <<'EOF'
url=https://your-site.example/
STREZZ_PUBLIC_URL=http://192.168.1.4:4000   # this computer's LAN IP; containers can't detect it
STREZZ_TOKEN=pick-a-long-random-string      # optional: keeps the QR code stable
EOF
docker compose up --build
```

The compose file publishes port 4000 and raises `nofile` to 1 048 576.

Run a worker in a container on another machine:

```sh
docker build -t strezz .
docker run --rm --ulimit nofile=1048576:1048576 strezz \
  worker --coordinator http://192.168.1.4:4000 --token pick-a-long-random-string
```

## CLI reference

### `strezz coordinator`

| Flag | Env | Default | Description |
|---|---|---|---|
| `--url <URL>` | `url` | required | Site under test, crawled for paths. |
| `--listen <ADDR>` | | `0.0.0.0:4000` | Address to listen on. `0.0.0.0` = reachable from the LAN. |
| `--token <TOKEN>` | `STREZZ_TOKEN` | random | Join/control token. |
| `--public-url <URL>` | `STREZZ_PUBLIC_URL` | auto-detected | URL other devices use to reach the coordinator. |
| `--path <PATH>` | | | Extra path to test (repeatable). |
| `--no-local-worker` | | off | Don't generate load from the coordinator itself. |

### `strezz worker`

| Flag | Env | Default | Description |
|---|---|---|---|
| `--coordinator <URL>` | `STREZZ_COORDINATOR` | required | Coordinator base URL. |
| `--token <TOKEN>` | `STREZZ_TOKEN` | required | Join token. |
| `--name <NAME>` | | hostname | Label shown on the dashboard. |
| `--max-users <N>` | | fd-based capacity | Upper limit on concurrent users. |

### `strezz locustfile`

| Flag | Env | Default | Description |
|---|---|---|---|
| `--url <URL>` | `url` | required | Site to crawl. |
| `-o, --output <FILE>` | | `loc.py` | Where to write the Locust file. |

## HTTP API

**Auth.** Every endpoint except `/`, `/join` and `/api/qr.svg` needs the token, either as an
`x-strezz-token` header or a `?token=` query parameter.

**Scripting.** Use the API to automate runs, for example from CI.

### Endpoints

| Method | Path | Body → Response |
|---|---|---|
| `GET` | `/` | Dashboard |
| `GET` | `/join` | Browser worker page |
| `GET` | `/api/qr.svg?data=…` | QR code SVG |
| `POST` | `/api/register` | `RegisterRequest` → `{ "worker_id", "sync_interval_ms" }` |
| `POST` | `/api/sync` | `SyncRequest` → `Assignment`. Returns 404 if the worker is unknown; register again. |
| `POST` | `/api/run/start` | `StartRun` → `{ "run_id" }` |
| `POST` | `/api/run/stop` | → `204` |
| `GET` | `/api/status` | Fleet and run status (see below) |

### Start, watch, stop

```sh
C=http://192.168.1.4:4000; T=3f9a0c1d7e2b

curl -X POST "$C/api/run/start" -H "x-strezz-token: $T" -H 'content-type: application/json' \
  -d '{"users":500,"spawn_rate":50,"duration_secs":120,"wait_min_ms":1000,"wait_max_ms":3000,"timeout_ms":30000}'

curl -s "$C/api/status?token=$T" | jq '.run | {rps, p95_ms, failures, requests}'

curl -X POST "$C/api/run/stop" -H "x-strezz-token: $T"
```

`StartRun` fields: `users` is required (`0` = full fleet capacity). The others default to
`spawn_rate` 50, `duration_secs` 0, `wait_min_ms` 1000, `wait_max_ms` 3000 and
`timeout_ms` 30000.

### Worker protocol

Use this to write your own worker.

```jsonc
// POST /api/register
{ "name": "my-device", "kind": "native", "capacity": 500, "platform": "linux aarch64" }

// POST /api/sync, once per second
{
  "worker_id": "a1b2c3d4",
  "run_id": 3,                 // run the delta belongs to (0 = none)
  "active_users": 120,
  "capacity": 500,             // optional: update capacity live
  "delta": {                   // stats since the previous sync
    "requests": 240, "failures": 2, "bytes": 1843200, "latency_sum_ms": 21600.5,
    "hist": [/* 64 bucket counts */],
    "errors": { "HTTP 503": 2 }
  }
}

// ← Assignment
{
  "run_id": 3, "target": "https://your-site.example/", "paths": ["/", "/about"],
  "users": 120, "spawn_per_sec": 12.5,
  "wait_min_ms": 1000, "wait_max_ms": 3000, "timeout_ms": 30000
}
```

**Histogram.** Bucket `i` holds latencies whose `floor(log2(ms + 1) × 4) = i`, capped at 63
(about 55 s). The upper bound of bucket `i` is `2^((i + 1) / 4) − 1` ms.

**Sync rules:**
- Sync about once per second.
- Drop all users when `run_id` changes.
- Converge to `users`, adding at most `spawn_per_sec` per second.

## Security

- **Token.** Every API call and the dashboard require the join token.
  - Without `--token`, a random 12-character token is generated on each start.
  - Use a long `STREZZ_TOKEN` if you want it stable.
- **Plain HTTP.** Traffic to the coordinator, including the token, is unencrypted. Anyone sniffing
  the same Wi-Fi could read it. Use trusted networks, or put the coordinator behind an HTTPS
  tunnel or reverse proxy.
- **LAN only.** Don't port-forward 4000 to the internet. Anyone with the token could point your
  fleet at a run and start it.
- **Fixed target.** The target is set when the coordinator starts and can't be changed over the
  API. Workers only ever hit that one site.
- **Shut it down.** Stop the coordinator when you're not testing.

## Limitations

- **Browser workers can't see HTTP status codes.** Cross-origin requests use
  `fetch(…, { mode: "no-cors" })`, which still sends a real request but returns an opaque
  response. Browser workers therefore only count network errors and timeouts, and report
  `bytes = 0`.
- **Browsers cap connections** at about 6 per host over HTTP/1.1. HTTP/2 and HTTP/3 targets allow
  more concurrent requests per phone.
- **Background tabs are throttled** by mobile browsers. Keep the join page in the foreground.
- **Native capacity reflects file descriptors only**, not CPU or bandwidth. Cap big machines
  with `--max-users`.
- **GET only.** Virtual users request the discovered and configured paths with `GET`. There are
  no logins, forms or scripted user journeys (yet).
- **One coordinator, one run at a time.** Run history is kept in memory only, and the per-second
  timeline keeps the last 10 minutes.

## Legacy Locust mode and migrating from 0.1

Strezz 0.1 was a Node.js script that crawled a page and generated a Locust file. It was then run
in a single Docker container. That behaviour is still available:

```sh
strezz locustfile --url https://your-site.example/ -o loc.py
locust -f loc.py --host https://your-site.example/
```

Differences from 0.1:

- The first discovered link is no longer skipped.
- Protocol-relative links (`//other-host/…`) are no longer mistaken for site paths.
- Same-site absolute links are now included.
- Duplicate links are removed.

| 0.1 (Node.js) | 0.2 (Rust) |
|---|---|
| `url=… node index.js` → `loc.py` | `strezz locustfile --url …` |
| `docker-compose up` → Locust on `:4000` | `docker compose up` → Strezz dashboard on `:4000` |
| One machine, Locust's engine | Fleet of devices, built-in engine |
| `jest` | `cargo test` |

## Development

```sh
cargo test                                  # unit tests
cargo clippy --all-targets -- -D warnings   # lints (CI fails on warnings)
cargo run -- coordinator --url http://localhost:8080/
```

A quick local target to test against:

```sh
mkdir -p /tmp/site && cd /tmp/site && echo '<a href="/a">a</a>' > index.html && python3 -m http.server 8080
```

### Layout

| Path | Responsibility |
|---|---|
| `src/main.rs` | CLI, coordinator startup (crawl, address detection, QR, local worker) |
| `src/coordinator.rs` | Fleet state, water-filling allocation, stats aggregation, HTTP API |
| `src/worker.rs` | Native load engine: register/sync loop, ramp-up, virtual users, error classification |
| `src/protocol.rs` | Wire types shared by coordinator and workers |
| `src/stats.rs` | Mergeable log-bucket latency histogram and percentiles |
| `src/limits.rs` | Raising `RLIMIT_NOFILE` to the kernel maximum; capacity |
| `src/netaddr.rs` | Ranking network interfaces to find the LAN address |
| `src/crawl.rs` | Same-site link discovery |
| `src/locust.rs` | Locust file generation |
| `web/dashboard.html` | Dashboard (embedded with `include_str!`) |
| `web/join.html` | Browser worker: device profiling, auto-tune, load loop |

**Histogram constant.** `src/stats.rs` and `web/join.html` share the histogram formula. If you
change it, change both.

### CI

GitHub Actions runs `cargo clippy -D warnings` and `cargo test` on Linux, macOS and Windows for
every push and pull request to `master`. CodeQL scans the Rust code.

## FAQ

**Do the phones need an app?**
No. Any modern browser works. Android users who want more load can run the native worker in
Termux.

**Does test traffic go through the coordinator?**
No. Each device sends requests straight to the target over its own connection. The coordinator
only carries a small JSON sync per device per second.

**How many users can a phone handle?**
It depends on the phone, the network and the target. Turn on auto-tune and watch its status line.
Typical phones level off between a few dozen and a few hundred users with think time.

**Can I test `localhost` or a staging server on my LAN?**
Yes, as long as every worker can reach that address.

- Use the machine's LAN IP in `--url`, not `localhost`, so phones resolve it to the right
  machine.
- Browser workers loaded over `http://` can call `http://` and `https://` targets.

**Can I run more than one test at a time?**
Not from one coordinator. Start a second coordinator on another port (`--listen 0.0.0.0:4001`)
with its own token.
