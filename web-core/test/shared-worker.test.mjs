import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import vm from "node:vm";

function harness({ welcome = true, extra = {}, connection = {} } = {}) {
  let now = 1000,
    tick;
  const calls = [],
    sessions = [];
  const config = {
    origin: "https://example.test",
    rpId: "example.test",
    workspace: "test",
    username: "user",
    ...connection,
  };
  // Timers run on the injected clock: `advance` fires those now due.
  const timers = new Map();
  let timerId = 0;
  const context = {
    Date: { now: () => now },
    location: { origin: config.origin },
    fetch: async () => ({ ok: true, json: async () => config }),
    setInterval: (fn) => {
      tick = fn;
    },
    setTimeout: (fn, ms = 0) => {
      timers.set(++timerId, { fn, at: now + ms });
      return timerId;
    },
    clearTimeout: (id) => timers.delete(id),
  };
  const source = stripTypeScriptTypes(
    readFileSync(
      new URL("../src/session-worker.ts", import.meta.url),
      "utf8",
    ).replace(/^import \{ Session \} from .*;\n/m, ""),
    { mode: "transform" },
  );
  vm.runInNewContext(source, context);
  const attach = () => {
    const messages = [];
    const session = { closed: false };
    const port = {
      postMessage: (value) => {
        messages.push(structuredClone(value));
        if (value.type === "host_start") {
          sessions.push(session);
          if (welcome)
            queueMicrotask(() =>
              port.onmessage({
                data: {
                  type: "host_event",
                  event: {
                    Stream: {
                      type: "welcome",
                      client: 7,
                      capabilities: [],
                      ...extra,
                    },
                  },
                },
              }),
            );
        } else if (value.type === "host_send")
          calls.push({ type: value.request, params: value.params });
        else if (value.type === "host_rpc") {
          calls.push({ method: value.method, params: value.params });
          queueMicrotask(() =>
            port.onmessage({
              data: { type: "host_result", id: value.id, result: {} },
            }),
          );
        } else if (value.type === "host_stop") session.closed = true;
      },
      start() {},
      close() {},
    };
    context.onconnect({ ports: [port] });
    return { port, messages };
  };
  const send = async (view, data) => {
    view.port.onmessage({ data });
    await new Promise(setImmediate);
  };
  const join = (view, extra = {}) =>
    send(view, {
      type: "join",
      config,
      identity: { origin: config.origin, rpId: config.rpId },
      ...extra,
    });
  return {
    attach,
    send,
    join,
    calls,
    sessions,
    config,
    advance: (ms, { fire = true } = {}) => {
      now += ms;
      if (!fire) return;
      for (const [id, timer] of [...timers].sort((a, b) => a[1].at - b[1].at))
        if (timers.has(id) && timer.at <= now) {
          timers.delete(id);
          timer.fn();
        }
    },
    tick: () => tick(),
    /** Evaluates an expression in the worker's global scope. */
    inspect: (expression) => vm.runInContext(expression, context),
  };
}

test("worker reuses one connection, validates configuration before disclosure and logs out every view", async () => {
  const h = harness(),
    a = h.attach(),
    b = h.attach();
  await h.join(a, {
    identity: {
      origin: h.config.origin,
      rpId: h.config.rpId,
      credentialId: "desktop",
      label: "Desktop",
    },
  });
  await h.join(b, {
    resumeOnly: true,
    identity: {
      origin: h.config.origin,
      rpId: h.config.rpId,
      credentialId: "laptop",
      label: "Laptop",
    },
  });
  assert.equal(
    b.messages.find((message) => message.type === "signed_in_key").key
      .credentialId,
    "desktop",
  );
  assert.equal(h.sessions.length, 1);
  assert.ok(
    b.messages.some((message) => message.event?.Stream.type === "welcome"),
  );
  const invalid = h.attach();
  await h.join(invalid, {
    config: { ...h.config, workspace: "different-user" },
  });
  assert.equal(
    invalid.messages.some((message) => message.type === "event"),
    false,
  );
  assert.ok(invalid.messages.some((message) => message.type === "closed"));
  await h.send(a, { type: "logout" });
  assert.equal(h.sessions[0].closed, true);
  assert.ok(b.messages.some((message) => message.type === "closed"));
  const fresh = h.attach();
  await h.join(fresh, { resumeOnly: true });
  assert.equal(
    h.sessions.length,
    1,
    "resume cannot authenticate a new connection",
  );
});

test("worker rejects input and resize from a window that lost the pane", async () => {
  const h = harness(),
    a = h.attach(),
    b = h.attach();
  await h.join(a);
  await h.join(b);
  await h.send(a, { type: "request", request: "claim", params: { pane: 1 } });
  await h.send(b, { type: "request", request: "claim", params: { pane: 1 } });
  const count = h.calls.length;
  for (const request of ["input", "paste", "resize", "theme", "release"])
    await h.send(a, {
      type: "request",
      request,
      params: { pane: 1, bytes: [65] },
    });
  assert.equal(h.calls.length, count);
  await h.send(b, {
    type: "request",
    request: "input",
    params: { pane: 1, bytes: [66] },
  });
  assert.equal(h.calls.at(-1).type, "input");
});

const minute = 60000;
const idleNotices = (view) =>
  view.messages.filter((message) => message.type === "idle");
const lockMessage = (view) =>
  view.messages.find((message) => message.type === "closed")?.message;

test("passive polls and heartbeats do not prevent the idle warning and lock", async () => {
  const h = harness(),
    a = h.attach();
  await h.join(a);
  h.advance(59 * minute);
  await h.send(a, { type: "rpc", id: 1, method: "status" });
  await h.send(a, { type: "pulse" });
  h.tick();
  assert.deepEqual(idleNotices(a), []);
  h.advance(minute);
  assert.deepEqual(idleNotices(a), [
    {
      type: "idle",
      warning: true,
      idleFor: "60 minutes",
      lockAt: 1000 + 75 * minute,
    },
  ]);
  await h.send(a, { type: "rpc", id: 2, method: "history" });
  await h.send(a, { type: "pulse" });
  h.advance(15 * minute - 1);
  assert.notEqual(h.sessions[0].closed, true);
  assert.equal(idleNotices(a).length, 1, "polls do not lift the warning");
  h.advance(1);
  assert.equal(h.sessions[0].closed, true);
  assert.equal(
    lockMessage(a),
    "Session locked after 75 minutes without activity. Authenticate again to reconnect.",
  );
});

test("the warning reaches every window, and activity in any one lifts it everywhere", async () => {
  const h = harness(),
    a = h.attach(),
    b = h.attach();
  await h.join(a);
  await h.join(b, { resumeOnly: true });
  h.advance(60 * minute);
  for (const view of [a, b])
    assert.equal(idleNotices(view).at(-1)?.warning, true);
  // A window opened during the warning shows it too.
  const c = h.attach();
  await h.join(c, { resumeOnly: true });
  assert.equal(idleNotices(c).at(-1)?.warning, true);
  h.advance(10 * minute);
  await h.send(b, { type: "activity" });
  for (const view of [a, b, c])
    assert.deepEqual(idleNotices(view).at(-1), {
      type: "idle",
      warning: false,
    });
  // The clock restarted: no lock at the old deadline, a new warning later.
  h.advance(5 * minute + 1);
  assert.notEqual(h.sessions[0].closed, true);
  for (const view of [a, b, c]) await h.send(view, { type: "pulse" });
  h.advance(55 * minute - 1);
  assert.equal(idleNotices(a).at(-1)?.warning, true);
  assert.equal(idleNotices(a).at(-1)?.lockAt, 1000 + 145 * minute);
  // Terminal input counts as activity too.
  await h.send(a, { type: "request", request: "claim", params: { pane: 1 } });
  assert.deepEqual(idleNotices(c).at(-1), { type: "idle", warning: false });
  h.advance(60 * minute - 1);
  for (const view of [a, b, c]) await h.send(view, { type: "pulse" });
  h.advance(15 * minute);
  await h.send(a, {
    type: "request",
    request: "input",
    params: { pane: 1, bytes: [65] },
  });
  assert.deepEqual(idleNotices(a).at(-1), { type: "idle", warning: false });
  assert.notEqual(h.sessions[0].closed, true);
});

test("activity from a window that has not joined is ignored", async () => {
  const h = harness(),
    a = h.attach(),
    stranger = h.attach();
  await h.join(a);
  h.advance(60 * minute);
  await h.send(stranger, { type: "activity" });
  assert.equal(idleNotices(a).at(-1)?.warning, true);
  assert.equal(
    stranger.messages.some((message) => message.type === "event"),
    false,
  );
});

test("idle durations come from the relay's connection.json", async () => {
  const h = harness({
      connection: { idle_warning_minutes: 2, idle_lock_minutes: 1 },
    }),
    a = h.attach();
  await h.join(a);
  h.advance(2 * minute - 1);
  assert.deepEqual(idleNotices(a), []);
  h.advance(1);
  assert.equal(idleNotices(a).at(-1)?.idleFor, "2 minutes");
  h.advance(minute);
  assert.equal(h.sessions[0].closed, true);
  assert.equal(
    lockMessage(a),
    "Session locked after 3 minutes without activity. Authenticate again to reconnect.",
  );
});

test("a worker that slept past both durations locks at once, unwarned", async () => {
  for (const wake of ["tick", "message", "timer"]) {
    const h = harness(),
      a = h.attach();
    await h.join(a);
    // Suspended: no timer or interval ran while the clock moved on.
    h.advance(75 * minute + 1, { fire: false });
    if (wake === "tick") h.tick();
    else if (wake === "message") await h.send(a, { type: "activity" });
    else h.advance(0);
    assert.equal(h.sessions[0].closed, true, wake);
    assert.deepEqual(idleNotices(a), [], wake);
    assert.match(lockMessage(a), /75 minutes/, wake);
  }
});

test("a shared view cannot impersonate the SSH host and closing it leaves SSH open", async () => {
  const h = harness(),
    a = h.attach(),
    b = h.attach();
  await h.join(a);
  await h.join(b);
  await h.send(b, {
    type: "host_event",
    event: { Stream: { type: "welcome", client: 999 } },
  });
  assert.ok(
    b.messages.some((message) =>
      message.event?.Stream.message?.includes("Only the connection tab"),
    ),
  );
  assert.equal(
    a.messages.some((message) => message.event?.Stream.client === 999),
    false,
  );
  await h.send(b, { type: "leave" });
  assert.notEqual(h.sessions[0].closed, true);
  await h.send(a, { type: "leave" });
  assert.equal(h.sessions[0].closed, true);
});

test("first input after suspension cannot revive an expired session", async () => {
  const h = harness(),
    a = h.attach();
  await h.join(a);
  await h.send(a, { type: "request", request: "claim", params: { pane: 1 } });
  const count = h.calls.length;
  h.advance(75 * 60000 + 1, { fire: false });
  await h.send(a, {
    type: "request",
    request: "input",
    params: { pane: 1, bytes: [65] },
  });
  assert.equal(h.sessions[0].closed, true);
  assert.equal(h.calls.length, count, "expired input must not reach SSH");
});

test("a delayed release acknowledgement cannot erase a newer view's pending claim", async () => {
  const h = harness(),
    a = h.attach(),
    b = h.attach();
  await h.join(a);
  await h.join(b);
  const native = (pane, owner) =>
    h.send(a, {
      type: "host_event",
      event: { Stream: { type: "lease", pane, owner } },
    });
  await h.send(a, { type: "request", request: "claim", params: { pane: 1 } });
  await native(1, 7);
  await h.send(a, { type: "request", request: "release", params: { pane: 1 } });
  await h.send(b, { type: "request", request: "claim", params: { pane: 1 } });
  await native(1, null);
  await h.send(b, {
    type: "request",
    request: "input",
    params: { pane: 1, bytes: [65] },
  });
  assert.equal(h.calls.at(-1).type, "input");
  await native(1, 7);
  await h.send(b, {
    type: "request",
    request: "input",
    params: { pane: 1, bytes: [66] },
  });
  assert.deepEqual(h.calls.at(-1).params.bytes, [66]);
});

test("a joining view gets a baseline before metadata-only updates, then receives sparse patches", async () => {
  const h = harness(),
    a = h.attach();
  await h.join(a);
  await h.send(a, {
    type: "request",
    request: "subscribe",
    params: { panes: [1] },
  });
  const screen = {
    cols: 2,
    rows: 1,
    cells: [
      { text: "a", width: 1 },
      { text: " ", width: 1 },
    ],
    cursor: null,
    modes: {},
  };
  const update = (snapshots = [], patches = []) =>
    h.send(a, {
      type: "host_event",
      event: {
        Stream: {
          type: "update",
          subscribed: [1],
          panes: [{ id: 1 }],
          leases: [],
          snapshots,
          patches,
        },
      },
    });
  await update([{ pane: 1, revision: 1, screen }]);
  const b = h.attach();
  await h.join(b, { resumeOnly: true });
  await h.send(b, {
    type: "request",
    request: "subscribe",
    params: { panes: [1] },
  });
  await update();
  let last = b.messages
    .filter((message) => message.event?.Stream.type === "update")
    .at(-1).event.Stream;
  assert.equal(last.snapshots[0].pane, 1);
  await update(
    [],
    [
      {
        pane: 1,
        base: 1,
        revision: 2,
        cursor: null,
        modes: {},
        changes: [[0, { text: "b", width: 1 }]],
      },
    ],
  );
  last = b.messages
    .filter((message) => message.event?.Stream.type === "update")
    .at(-1).event.Stream;
  assert.equal(last.snapshots.length, 0);
  assert.equal(last.patches[0].revision, 2);
});

test("application clipboard writes reach only the owning window and are not replayed", async () => {
  const h = harness(),
    a = h.attach(),
    b = h.attach();
  await h.join(a);
  await h.join(b, { resumeOnly: true });
  await h.send(b, {
    type: "request",
    request: "claim",
    params: { pane: 1, takeover: true },
  });
  await h.send(a, {
    type: "host_event",
    event: { Stream: { type: "clipboard", pane: 1, text: "private copy" } },
  });
  assert.equal(
    a.messages.filter((m) => m.event?.Stream?.type === "clipboard").length,
    0,
  );
  assert.equal(
    b.messages.filter((m) => m.event?.Stream?.type === "clipboard").length,
    1,
  );
  const c = h.attach();
  await h.join(c, { resumeOnly: true });
  assert.equal(
    c.messages.filter((m) => m.event?.Stream?.type === "clipboard").length,
    0,
  );
});

test("notification acknowledgements are forwarded, not refused as pane operations", async () => {
  const h = harness(),
    a = h.attach();
  await h.join(a);
  await h.send(a, {
    type: "request",
    request: "acknowledge",
    params: { notification: 12 },
  });
  assert.equal(
    JSON.stringify(h.calls.at(-1)),
    JSON.stringify({ type: "acknowledge", params: { notification: 12 } }),
  );
  const count = h.calls.length;
  await h.send(a, {
    type: "request",
    request: "acknowledge",
    params: { notification: "12; rm" },
  });
  assert.equal(h.calls.length, count);
  assert.equal(
    a.messages.some((m) =>
      /Unsupported pane operation/.test(m.event?.Stream?.message || ""),
    ),
    false,
  );
});

test("a stopped workspace reaches every view and allows only state and start", async () => {
  const h = harness({ welcome: false }),
    a = h.attach(),
    b = h.attach();
  await h.join(a);
  const stopped = { Workspace: { running: false, workspace: "test" } };
  await h.send(a, { type: "host_event", event: stopped });
  assert.ok(a.messages.some((m) => m.event?.Workspace?.running === false));
  // A window joining later learns it too.
  await h.join(b, { resumeOnly: true });
  assert.ok(b.messages.some((m) => m.event?.Workspace?.running === false));
  await h.send(b, {
    type: "rpc",
    id: 1,
    method: "start",
    params: { confirmed: true },
  });
  assert.deepEqual(h.calls.at(-1), {
    method: "start",
    params: { confirmed: true },
  });
  await h.send(b, { type: "rpc", id: 2, method: "new", params: {} });
  assert.match(
    b.messages.find((m) => m.id === 2)?.error || "",
    /still in progress/,
  );
  await h.send(b, { type: "request", request: "input", params: { pane: 1 } });
  assert.notEqual(h.calls.at(-1).type, "input");
});

test("uploads go to the window that began them, within one connection-wide window", async () => {
  const h = harness({
    extra: {
      capabilities: ["uploads"],
      upload_limits: { max_chunks_in_flight: 2 },
    },
  });
  const a = h.attach(),
    b = h.attach();
  await h.join(a);
  await h.join(b);
  const events = (view, type) =>
    view.messages.filter((m) => m.event?.Stream?.type === type);
  const host = (event) =>
    h.send(a, { type: "host_event", event: { Stream: event } });
  const sent = (type) => h.calls.filter((call) => call.type === type);
  const request = (view, type, params) =>
    h.send(view, { type: "request", request: type, params });

  await request(a, "upload_begin", {
    request: "upload-a-begin",
    files: [{ name: "a", size: 9 }],
  });
  assert.equal(sent("upload_begin").length, 1);
  await host({
    type: "upload_started",
    request: "upload-a-begin",
    batch: 1,
    dir: "/d",
  });
  assert.equal(events(a, "upload_started").length, 1);
  assert.equal(events(b, "upload_started").length, 0, "another window's batch");

  // Another window cannot send chunks for this batch.
  await request(b, "upload_chunk", {
    batch: 1,
    file_index: 0,
    offset: 0,
    data: "YQ==",
  });
  assert.equal(sent("upload_chunk").length, 0);
  for (const offset of [0, 1, 2])
    await request(a, "upload_chunk", {
      batch: 1,
      file_index: 0,
      offset,
      data: "YQ==",
    });
  assert.equal(sent("upload_chunk").length, 2, "the connection's window");
  await host({ type: "upload_progress", batch: 1, file_index: 0, received: 1 });
  assert.equal(events(a, "upload_progress").length, 1);
  assert.equal(events(b, "upload_progress").length, 0);
  assert.equal(sent("upload_chunk").length, 3, "one more after each progress");

  // A failure ends the batch: its unacknowledged chunks no longer count.
  await host({ type: "upload_failed", batch: 1, message: "disk full" });
  assert.equal(events(a, "upload_failed").length, 1);
  await request(a, "upload_begin", {
    request: "upload-a-second",
    files: [{ name: "a", size: 9 }],
  });
  await host({
    type: "upload_started",
    request: "upload-a-second",
    batch: 2,
    dir: "/d",
  });
  for (const offset of [0, 1])
    await request(a, "upload_chunk", {
      batch: 2,
      file_index: 0,
      offset,
      data: "YQ==",
    });
  assert.equal(sent("upload_chunk").length, 5);

  // An upload refusal reaches only its window and keeps pane leases.
  await request(a, "claim", { pane: 3 });
  await request(b, "upload_begin", {
    request: "upload-b-begin",
    files: [{ name: "b", size: 1 }],
  });
  await host({
    type: "error",
    request: "upload-b-begin",
    message: "too many batches",
  });
  assert.equal(events(b, "error").length, 1);
  assert.equal(events(a, "error").length, 0);
  assert.deepEqual(
    a.messages.filter((m) => m.type === "owned").at(-1).panes,
    [3],
  );

  // A window that leaves cancels its unfinished batches.
  await request(b, "upload_begin", {
    request: "upload-b-again",
    files: [{ name: "b", size: 1 }],
  });
  await host({
    type: "upload_started",
    request: "upload-b-again",
    batch: 7,
    dir: "/d",
  });
  await h.send(b, { type: "leave" });
  const cancel = sent("upload_cancel").at(-1);
  assert.equal(cancel.params.batch, 7);
  await host({ type: "done", request: cancel.params.request });
  assert.equal(events(a, "done").length, 0);
});

test("a window's binary-screens opt-in stays with the hosting connection", async () => {
  const h = harness({ extra: { capabilities: ["binary-screens"] } }),
    a = h.attach(),
    b = h.attach();
  await h.join(a);
  await h.join(b, { resumeOnly: true });
  const count = h.calls.length;
  // Each window's Session sees the replayed Welcome and asks to opt in; the
  // host page's own Session already did on the shared connection.
  await h.send(b, {
    type: "request",
    request: "binary_screens",
    params: { enabled: false },
  });
  assert.equal(h.calls.length, count);
  assert.equal(
    b.messages.filter((m) => m.event?.Stream?.type === "error").length,
    0,
  );
});

test("file requests are relayed and each reply reaches only the window that asked", async () => {
  const h = harness({ extra: { capabilities: ["files"] } });
  const a = h.attach(),
    b = h.attach();
  await h.join(a);
  await h.join(b);
  const events = (view, type) =>
    view.messages.filter((m) => m.event?.Stream?.type === type);
  const host = (event) =>
    h.send(a, { type: "host_event", event: { Stream: event } });
  const sent = (type) => h.calls.filter((call) => call.type === type);
  const request = (view, type, params) =>
    h.send(view, { type: "request", request: type, params });

  await request(b, "file_list", { request: "file-b-1", path: "~" });
  await request(a, "file_mkdir", { request: "file-a-1", path: "/home/me/x" });
  await request(a, "file_read", { request: "file-a-2", path: "/home/me/y" });
  assert.equal(sent("file_list").length, 1);
  assert.deepEqual(sent("file_list")[0].params, {
    request: "file-b-1",
    path: "~",
  });
  await host({
    type: "file_list",
    request: "file-b-1",
    path: "/home/me",
    entries: [],
    truncated: false,
    skipped: 0,
  });
  assert.equal(events(b, "file_list").length, 1);
  assert.equal(events(a, "file_list").length, 0, "another window's reply");
  await host({ type: "done", request: "file-a-1" });
  assert.equal(events(a, "done").length, 1);
  assert.equal(events(b, "done").length, 0);
  // A refusal answers only its request and leaves pane claims alone.
  await h.send(a, { type: "request", request: "claim", params: { pane: 1 } });
  await host({ type: "error", request: "file-a-2", message: "bad request" });
  assert.equal(events(a, "error").length, 1);
  assert.equal(events(b, "error").length, 0);
  await h.send(a, {
    type: "request",
    request: "input",
    params: { pane: 1, bytes: [65] },
  });
  assert.equal(sent("input").length, 1, "the claim survived the file error");

  // Identifiers are checked and cannot be reused while a reply is awaited.
  const count = h.calls.length;
  await request(a, "file_stat", { request: "bad id", path: "/" });
  await request(a, "file_stat", { request: "file-a-3", path: "/" });
  await request(b, "file_stat", { request: "file-a-3", path: "/" });
  assert.equal(h.calls.length, count + 1);
  assert.ok(
    b.messages.some(
      (m) => m.event?.Stream?.message === "Duplicate request identifier",
    ),
  );
  // A window that leaves never gets its late reply broadcast to others.
  const c = h.attach();
  await h.join(c);
  await request(c, "file_stat", { request: "file-c-1", path: "/" });
  await h.send(c, { type: "leave" });
  await host({
    type: "file_stat",
    request: "file-c-1",
    path: "/",
    entry: { name: "/", kind: "dir", size: 0, modified: 0, mode: 493 },
  });
  assert.equal(events(b, "file_stat").length, 0);
  assert.equal(events(a, "file_stat").length, 0);
});

test("a download's chunks reach only the window that began it", async () => {
  const h = harness({ extra: { capabilities: ["files", "file-downloads"] } });
  const a = h.attach(),
    b = h.attach();
  await h.join(a);
  await h.join(b);
  const events = (view, type) =>
    view.messages.filter((m) => m.event?.Stream?.type === type);
  const host = (event) =>
    h.send(a, { type: "host_event", event: { Stream: event } });
  const sent = (type) => h.calls.filter((call) => call.type === type);
  const request = (view, type, params) =>
    h.send(view, { type: "request", request: type, params });

  await request(b, "file_download_begin", {
    request: "file-b-1",
    paths: ["/home/me/a"],
  });
  assert.equal(sent("file_download_begin").length, 1);
  await host({
    type: "file_download_started",
    request: "file-b-1",
    download: 4,
    name: "a",
    size: 3,
  });
  assert.equal(events(b, "file_download_started").length, 1);
  await host({ type: "file_download_chunk", download: 4, offset: 0, data: "YWJj" });
  assert.equal(events(b, "file_download_chunk").length, 1);
  assert.equal(events(a, "file_download_chunk").length, 0, "another window's");
  // Only the window that began it acknowledges or cancels it.
  await request(a, "file_download_ack", { download: 4, offset: 3 });
  assert.equal(sent("file_download_ack").length, 0);
  await request(b, "file_download_ack", { download: 4, offset: 3 });
  assert.deepEqual({ ...sent("file_download_ack")[0].params }, { download: 4, offset: 3 });
  await request(a, "file_download_cancel", { request: "file-a-9", download: 4 });
  assert.equal(sent("file_download_cancel").length, 0);
  await host({
    type: "file_download_done",
    request: "file-b-1",
    download: 4,
    size: 3,
    hash: "a".repeat(64),
    skipped: 0,
    changed: 0,
  });
  assert.equal(events(b, "file_download_done").length, 1);
  assert.equal(events(a, "file_download_done").length, 0);
  // Late events of an ended download are dropped, never broadcast.
  await host({ type: "file_download_chunk", download: 4, offset: 3, data: "YQ==" });
  assert.equal(events(a, "file_download_chunk").length + events(b, "file_download_chunk").length, 1);

  // A window that leaves cancels its downloads.
  await request(b, "file_download_begin", {
    request: "file-b-2",
    paths: ["/home/me/big"],
  });
  await host({
    type: "file_download_started",
    request: "file-b-2",
    download: 5,
    name: "big",
  });
  await h.send(b, { type: "leave" });
  const cancel = sent("file_download_cancel").at(-1);
  assert.equal(cancel.params.download, 5);
  await host({ type: "done", request: cancel.params.request });
  await host({ type: "file_download_chunk", download: 5, offset: 0, data: "YQ==" });
  assert.equal(events(a, "done").length, 0);
  assert.equal(events(a, "file_download_chunk").length, 0);
});

test("a cancelled download is forgotten once the server answers the cancel", async () => {
  const h = harness({ extra: { capabilities: ["files", "file-downloads"] } });
  const a = h.attach(),
    b = h.attach();
  await h.join(a);
  await h.join(b);
  const host = (event) =>
    h.send(a, { type: "host_event", event: { Stream: event } });
  const sent = (type) => h.calls.filter((call) => call.type === type);
  const request = (view, type, params) =>
    h.send(view, { type: "request", request: type, params });
  const known = () => h.inspect("downloads.size");
  const start = async (view, id, download) => {
    await request(view, "file_download_begin", { request: id, paths: ["/home/me/big"] });
    await host({ type: "file_download_started", request: id, download, name: "big" });
  };

  // Cancelled by the window that began it.
  await start(b, "file-b-1", 4);
  assert.equal(known(), 1);
  await request(b, "file_download_cancel", { request: "file-b-2", download: 4 });
  assert.equal(sent("file_download_cancel").length, 1);
  assert.equal(known(), 1, "chunks sent before the cancel still reach it");
  await host({ type: "done", request: "file-b-2" });
  assert.equal(known(), 0);
  assert.equal(h.inspect("cancelling.size"), 0);
  await host({ type: "file_download_chunk", download: 4, offset: 0, data: "YQ==" });
  assert.equal(b.messages.filter((m) => m.event?.Stream?.type === "file_download_chunk").length, 0);

  // Cancelled by the worker for a window that left.
  await start(b, "file-b-3", 5);
  await h.send(b, { type: "leave" });
  const cancel = sent("file_download_cancel").at(-1);
  assert.equal(cancel.params.download, 5);
  assert.equal(known(), 1);
  await host({ type: "done", request: cancel.params.request });
  assert.equal(known(), 0);
  assert.equal(h.inspect("cancelling.size"), 0);
});
