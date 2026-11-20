import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import vm from "node:vm";

function harness() {
  let now = 1000,
    tick;
  const calls = [],
    sessions = [];
  const config = {
    origin: "https://example.test",
    rpId: "example.test",
    workspace: "test",
    username: "user",
  };
  const context = {
    Date: { now: () => now },
    location: { origin: config.origin },
    fetch: async () => ({ ok: true, json: async () => config }),
    setInterval: (fn) => {
      tick = fn;
    },
    setTimeout,
    clearTimeout,
  };
  const source = stripTypeScriptTypes(
    readFileSync(
      new URL("../src/terminal/session-worker.ts", import.meta.url),
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
          queueMicrotask(() =>
            port.onmessage({
              data: {
                type: "host_event",
                event: {
                  Stream: { type: "welcome", client: 7, capabilities: [] },
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
    advance: (ms) => {
      now += ms;
    },
    tick: () => tick(),
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

test("passive polls and heartbeats do not prevent the 15-minute idle lock", async () => {
  const h = harness(),
    a = h.attach();
  await h.join(a);
  h.advance(14 * 60000);
  await h.send(a, { type: "rpc", id: 1, method: "status" });
  await h.send(a, { type: "pulse" });
  h.tick();
  assert.notEqual(h.sessions[0].closed, true);
  h.advance(60001);
  await h.send(a, { type: "pulse" });
  h.tick();
  assert.equal(h.sessions[0].closed, true);
  assert.ok(
    a.messages.some(
      (message) =>
        message.type === "closed" && message.message.includes("15 minutes"),
    ),
  );
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
  h.advance(15 * 60000 + 1);
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
