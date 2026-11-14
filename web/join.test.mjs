// Tests for the browser worker page.
//
// They run the page's real <script> against a stubbed browser: a fake coordinator
// answers /api/*, and every other fetch counts as a request to the site under test.
//
//   node --test web/join.test.mjs

import assert from "node:assert/strict";
import { getEventListeners } from "node:events";
import { readFileSync } from "node:fs";
import { describe, test } from "node:test";
import vm from "node:vm";

const html = readFileSync(new URL("./join.html", import.meta.url), "utf8");
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
const LOST_MS = Number(script.match(/LOST_MS = (\d+)/)[1]);

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Timers the page creates must not keep the test process alive once the tests are done.
const quiet = (schedule) => (...args) => {
  const timer = schedule(...args);
  timer.unref?.();
  return timer;
};
const pageTimeout = quiet(setTimeout);
const pageInterval = quiet(setInterval);

const RUN = {
  run_id: 5, target: "http://target.test/", paths: ["/"], users: 20, spawn_per_sec: 100,
  wait_min_ms: 0, wait_max_ms: 0, timeout_ms: 5000,
};
const IDLE = { ...RUN, run_id: 0, users: 0 };

/** Open the page. Returns handles to drive it and to see what it did. */
function open({ replyAfterMs = 10, assignment = RUN } = {}) {
  const elements = {};
  const element = (id) =>
    (elements[id] ??= { id, textContent: "", innerHTML: "", className: "", value: "", checked: true, disabled: false });

  // What the fake coordinator and the fake site saw.
  const seen = { hits: 0, calls: 0, registers: 0, syncs: 0, reported: 0 };
  // How the fake coordinator behaves. Tests change this as they go.
  const coordinator = { mode: "ok", replyAfterMs, assignment };
  let waitingForSync = [];

  const pause = (ms, signal) =>
    new Promise((resolve, reject) => {
      const timer = pageTimeout(resolve, ms);
      signal?.addEventListener("abort", () => { clearTimeout(timer); reject(new Error("aborted")); }, { once: true });
    });
  const reply = (status, body) => ({ ok: status < 400, status, json: async () => body, text: async () => String(body) });

  const fetch = async (url, options = {}) => {
    if (!url.startsWith("/api/")) {
      seen.hits++;
      await pause(5, options.signal);
      return { ok: true, status: 0 };
    }
    seen.calls++;
    if (url === "/api/sync") waitingForSync.splice(0).forEach((resolve) => resolve());
    if (coordinator.mode === "silent") return pause(1e9, options.signal);
    await pause(coordinator.replyAfterMs, options.signal);
    if (coordinator.mode === "down") throw new TypeError("Failed to fetch");
    if (coordinator.mode === "bad-token") return reply(401, "missing or wrong strezz token");
    if (url === "/api/register") {
      seen.registers++;
      return reply(200, { worker_id: "w" + seen.registers, sync_interval_ms: 200 });
    }
    const body = JSON.parse(options.body);
    if (coordinator.mode === "restarted" && body.worker_id === "w1") return reply(404, "unknown worker, register again");
    seen.syncs++;
    seen.reported += body.delta.requests;
    return reply(200, { ...coordinator.assignment });
  };

  const context = vm.createContext({
    document: { getElementById: element, addEventListener() {}, visibilityState: "visible" },
    navigator: { hardwareConcurrency: 8, userAgent: "test", platform: "test" },
    location: { search: "?token=t" },
    fetch, performance, URL, URLSearchParams, AbortController, console,
    setTimeout: pageTimeout, clearTimeout, setInterval: pageInterval, clearInterval,
  });
  vm.runInContext(script, context);

  const peek = (expression) => vm.runInContext(expression, context);
  return {
    elements, seen, coordinator, peek,
    /** Tap the Join / Leave button. */
    tap: () => elements.toggle.onclick(),
    users: () => peek("users.length"),
    /** Resolves when the page's next sync request reaches the coordinator. */
    nextSync: () => new Promise((resolve) => waitingForSync.push(resolve)),
    /** Requests the site under test receives during the next `ms`. */
    async hitsDuring(ms) {
      const before = seen.hits;
      await wait(ms);
      return seen.hits - before;
    },
    async until(what, ready) {
      const deadline = Date.now() + 10000;
      while (!ready()) {
        assert.ok(Date.now() < deadline, "timed out waiting for " + what);
        await wait(20);
      }
    },
  };
}

/** Open the page, join, and wait until it is generating load. */
async function joined(options) {
  const page = open(options);
  page.tap();
  await page.until("load on the target", () => page.users() > 0 && page.seen.hits > 20);
  return page;
}

describe("browser worker page", { concurrency: true }, () => {
  test("leaving while a sync is in flight starts no users", async () => {
    const page = await joined({ replyAfterMs: 300 });
    await page.nextSync();
    page.tap(); // Leave, while the answer to that sync is still on its way.
    await wait(1200);

    assert.equal(page.users(), 0);
    assert.equal(await page.hitsDuring(500), 0, "requests kept going after Leave");
    assert.equal(page.elements.toggle.textContent, "Join fleet");
    assert.equal(page.elements.state.textContent, "not joined");
  });

  test("leaving and joining again does not run two loops", async () => {
    const page = await joined({ replyAfterMs: 100 });
    await page.nextSync();
    page.tap(); // Leave...
    page.tap(); // ...and straight back in, with the old tick still waiting.
    await wait(500);

    const before = page.seen.syncs;
    await wait(3000);
    // One loop syncs about every 300 ms here (100 ms answer + 200 ms interval), so about
    // 10 times in 3 s. Two loops would make it 20.
    const syncs = page.seen.syncs - before;
    assert.ok(syncs >= 4 && syncs <= 14, `expected about 10 syncs in 3 s, saw ${syncs}`);
    assert.equal(page.users(), RUN.users);
    page.tap();
  });

  test("it syncs as often as the coordinator asks and reports what it sends", async () => {
    const page = await joined();
    const before = page.seen.syncs;
    await wait(2000);
    // The fake coordinator asks for 200 ms, so about 9 syncs; at the one second the page
    // used to hardcode it would be 2.
    assert.ok(page.seen.syncs - before >= 4, `only ${page.seen.syncs - before} syncs in 2 s`);
    assert.ok(page.seen.reported > 0);
    page.tap();
  });

  test("a virtual user's abort signal does not collect listeners", async () => {
    const page = await joined();
    const sent = page.seen.hits;
    await wait(1500);
    assert.ok(page.seen.hits - sent > 200, "the users should have been busy");
    // One for the request in flight or the pause after it; never one per request made.
    const listeners = getEventListeners(page.peek("users[0].signal"), "abort").length;
    assert.ok(listeners <= 2, `${listeners} listeners piled up on one user's signal`);
    page.tap();
  });

  test("users are started at the rate the coordinator gives", async () => {
    const page = open({ assignment: { ...RUN, spawn_per_sec: 1 } });
    page.tap();
    await wait(3200);
    // One per second. The page used to enforce a floor of two.
    assert.ok(page.users() >= 2 && page.users() <= 4, `${page.users()} users after 3 s at 1/s`);
    page.tap();
  });

  for (const mode of ["down", "silent"]) {
    test(`users stop by themselves when the coordinator is ${mode}`, async () => {
      const page = await joined();
      page.coordinator.mode = mode; // refuses connections / accepts them and never answers
      await wait(LOST_MS + 2500);

      assert.equal(page.users(), 0);
      assert.equal(await page.hitsDuring(700), 0, "requests kept going with no coordinator");
      assert.match(page.elements.state.textContent, /load stopped/);

      // It keeps trying, and picks the run up again when the coordinator is back.
      page.coordinator.mode = "ok";
      await page.until("the page to resume", () => page.users() > 0);
      page.tap();
    });
  }

  test("a restarted coordinator ends the run it no longer has", async () => {
    const page = await joined();
    page.coordinator.assignment = IDLE;
    page.coordinator.mode = "restarted"; // same address, but it has forgotten worker w1
    await page.until("the page to register again", () => page.seen.registers === 2);
    await wait(600);

    assert.equal(page.users(), 0);
    assert.equal(await page.hitsDuring(500), 0);
    page.tap();
  });

  test("a rejected token stops the page instead of retrying forever", async () => {
    const page = open();
    page.coordinator.mode = "bad-token";
    page.tap();
    await wait(800);

    assert.match(page.elements.state.textContent, /bad token/);
    assert.equal(page.elements.toggle.textContent, "Join fleet");
    const calls = page.seen.calls;
    await wait(1500);
    assert.equal(page.seen.calls, calls, "it kept calling the coordinator");
  });

  test("a failed sync does not lose its numbers", async () => {
    const page = await joined();
    page.coordinator.mode = "down"; // a blip, much shorter than the cut-off
    await page.nextSync();
    await page.nextSync(); // at least one sync has failed with requests in it
    page.coordinator.mode = "ok";
    await page.nextSync();
    await page.nextSync();
    page.tap(); // Leave: no more requests, no more syncs.
    await wait(400);

    // Every completed request was either reported or is still waiting to be.
    const waiting = page.peek("delta.requests");
    assert.equal(page.seen.reported + waiting, page.peek("totals.sent"));
  });
});
