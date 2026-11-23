import type { Connection, Identity, Snapshot } from "./types";

// Same-origin broker. WASSH stays in its originating page because Chromium
// SharedWorkers cannot provide the cross-origin isolation WASSH requires.
type View = {
  port: MessagePort;
  subscribed: number[];
  pulse: number;
  ready: boolean;
  revisions: Map<number, number>;
};
const views = new Set<View>();
const owners = new Map<number, View>();
const claiming = new Map<number, number>();
const screens = new Map<number, Snapshot>();
let ssh: HostSession | undefined,
  configKey = "",
  welcome: any,
  /** The bridge's notice that the workspace server is not running. */
  stopped: any,
  latest: any;
let host: View | undefined;
let signingKey: { credentialId: string; label?: string } | undefined;
/** Idle lock (decision 163): after `idleWarningMs` without user activity in
 * any window, every window shows a warning; `idleLockMs` later the
 * connection locks. Activity is terminal input and the windows' own
 * `activity` reports (keys, clicks, taps, scrolling), never terminal output,
 * polls or heartbeats. Deadlines are wall-clock: a worker that wakes past
 * both (a sleeping laptop, a suspended tab) locks at once, unwarned. The
 * relay may set both durations in /connection.json (`idle_warning_minutes`,
 * `idle_lock_minutes`); the defaults match web/src/idle.ts. */
const minute = 60 * 1000;
let lastActivity = Date.now();
let idleWarningMs = 60 * minute,
  idleLockMs = 15 * minute,
  idleWarned = false,
  idleTimer: ReturnType<typeof setTimeout> | undefined;
/** Minutes from the relay as milliseconds; the relay validated the ranges. */
function minutes(value: unknown, fallback: number) {
  return typeof value === "number" && Number.isFinite(value) && value > 0
    ? value * minute
    : fallback * minute;
}
function duration(ms: number) {
  const m = ms / minute;
  if (Number.isInteger(m)) return `${m} minute${m === 1 ? "" : "s"}`;
  const s = Math.round(ms / 1000);
  return `${s} second${s === 1 ? "" : "s"}`;
}
function idleNotice() {
  return idleWarned
    ? {
        type: "idle",
        warning: true,
        idleFor: duration(idleWarningMs),
        lockAt: lastActivity + idleWarningMs + idleLockMs,
      }
    : { type: "idle", warning: false };
}
/** Locks a connection idle past both durations; true when it did. */
function lockIfExpired() {
  if (!ssh || Date.now() - lastActivity < idleWarningMs + idleLockMs)
    return false;
  stop(
    `Session locked after ${duration(idleWarningMs + idleLockMs)} without activity. Authenticate again to reconnect.`,
  );
  return true;
}
/** Locks or warns by wall-clock time, then waits for the next deadline. */
function checkIdle() {
  if (lockIfExpired() || !ssh) return;
  if (!idleWarned && Date.now() - lastActivity >= idleWarningMs) {
    idleWarned = true;
    for (const view of views) send(view, idleNotice());
  }
  scheduleIdle();
}
function scheduleIdle() {
  clearTimeout(idleTimer);
  idleTimer = undefined;
  if (!ssh) return;
  const due =
    lastActivity + idleWarningMs + (idleWarned ? idleLockMs : 0) - Date.now();
  idleTimer = setTimeout(checkIdle, Math.max(0, due));
}
/** The user did something: restart the idle clock and lift any warning. */
function active() {
  lastActivity = Date.now();
  if (idleWarned) {
    idleWarned = false;
    for (const view of views) send(view, idleNotice());
  }
  scheduleIdle();
}
/** Uploads (KelpServer `uploads`) belong to the one SSH connection all
 * windows share: correlated upload requests by identifier (with the batch a
 * finish or cancel names), and batches with the window that began them and
 * their unacknowledged chunks. A view of `undefined` is a batch whose window
 * left and is being cancelled. */
const uploadRequests = new Map<string, { view?: View; batch?: number }>();
const batches = new Map<number, { view?: View; inFlight: number }>();
/** Chunks waiting for the connection's flow-control window: the server
 * counts unacknowledged chunks per connection, whichever window sent them. */
const queuedChunks: { batch: number; params: any }[] = [];
let chunksInFlight = 0;
let uploadSerial = 0;
const uploadRequest = /^[A-Za-z0-9_-]{1,128}$/;
function uploadWindow() {
  const n = welcome?.upload_limits?.max_chunks_in_flight;
  return Number.isSafeInteger(n) && n > 0 ? n : 1;
}
function flushChunks() {
  while (ssh && queuedChunks.length && chunksInFlight < uploadWindow()) {
    const { batch, params } = queuedChunks.shift()!;
    const entry = batches.get(batch);
    if (!entry?.view) continue;
    entry.inFlight += 1;
    chunksInFlight += 1;
    ssh.request("upload_chunk", params);
  }
}
/** The batch ended (done, failed or cancelled): the server drops its
 * chunks still in flight without acknowledging them. */
function settleBatch(batch: unknown) {
  const entry = batches.get(batch as number);
  if (!entry) return;
  batches.delete(batch as number);
  chunksInFlight = Math.max(0, chunksInFlight - entry.inFlight);
  for (let i = queuedChunks.length - 1; i >= 0; i--)
    if (queuedChunks[i].batch === batch) queuedChunks.splice(i, 1);
  flushChunks();
}
function cancelBatch(batch: number) {
  const entry = batches.get(batch);
  if (!entry || !ssh) return;
  entry.view = undefined;
  const request = `upload-left-${++uploadSerial}`;
  uploadRequests.set(request, { batch });
  ssh.request("upload_cancel", { request, batch });
}
/** Routes an upload event to the window it belongs to. */
function uploadEvent(value: any) {
  const e = value.Stream;
  let view: View | undefined;
  if (e.type === "upload_started") {
    view = uploadRequests.get(e.request)?.view;
    uploadRequests.delete(e.request);
    batches.set(e.batch, { view, inFlight: 0 });
    // The window that began it left before it started.
    if (!view || !views.has(view)) cancelBatch(e.batch);
  } else if (e.type === "upload_progress") {
    const entry = batches.get(e.batch);
    if (entry) {
      entry.inFlight = Math.max(0, entry.inFlight - 1);
      chunksInFlight = Math.max(0, chunksInFlight - 1);
      view = entry.view;
      flushChunks();
    }
  } else if (e.type === "upload_failed") {
    view = batches.get(e.batch)?.view;
    settleBatch(e.batch);
  } else {
    // upload_done, done or error of a correlated upload request.
    const entry = uploadRequests.get(e.request);
    uploadRequests.delete(e.request);
    view = entry?.view;
    if (e.type === "upload_done") settleBatch(e.batch);
    else if (entry?.batch !== undefined) settleBatch(entry.batch);
  }
  if (view) send(view, { type: "event", event: value });
}
function isUploadEvent(e: any) {
  return (
    [
      "upload_started",
      "upload_progress",
      "upload_failed",
      "upload_done",
    ].includes(e.type) ||
    (["done", "error"].includes(e.type) &&
      typeof e.request === "string" &&
      uploadRequests.has(e.request))
  );
}
/** An upload request from a window; chunks wait for the window's turn. */
function uploadFrom(view: View, type: string, params: any) {
  if (!ssh) return;
  if (type === "upload_chunk") {
    const entry = batches.get(params.batch);
    // A chunk of a batch that ended (or another window's) is dropped.
    if (!entry || entry.view !== view) return;
    if (queuedChunks.length >= 256)
      throw Error("Too many upload chunks queued");
    queuedChunks.push({ batch: params.batch, params });
    flushChunks();
    return;
  }
  if (typeof params.request !== "string" || !uploadRequest.test(params.request))
    throw Error("Invalid upload request");
  if (uploadRequests.has(params.request))
    throw Error("Duplicate upload request");
  if (type === "upload_begin") {
    if (!Array.isArray(params.files) || params.files.length > 4096)
      throw Error("Invalid upload files");
    uploadRequests.set(params.request, { view });
  } else {
    if (batches.get(params.batch)?.view !== view)
      throw Error("Unknown upload batch");
    uploadRequests.set(params.request, { view, batch: params.batch });
    if (type === "upload_cancel")
      for (let i = queuedChunks.length - 1; i >= 0; i--)
        if (queuedChunks[i].batch === params.batch) queuedChunks.splice(i, 1);
  }
  ssh.request(type, params);
}
function send(view: View, value: any) {
  if (views.has(view) && (view.ready || value.type === "closed"))
    view.port.postMessage(value);
}
function ownership() {
  for (const view of views)
    send(view, {
      type: "owned",
      panes: [...owners]
        .filter(([, owner]) => owner === view)
        .map(([pane]) => pane),
    });
}
function stop(message = "Signed out of all KelpMux windows.") {
  const old = ssh;
  ssh = undefined;
  old?.close();
  welcome = undefined;
  stopped = undefined;
  latest = undefined;
  configKey = "";
  owners.clear();
  claiming.clear();
  screens.clear();
  uploadRequests.clear();
  batches.clear();
  queuedChunks.length = 0;
  chunksInFlight = 0;
  for (const view of views) {
    view.ready = false;
    send(view, { type: "closed", message });
  }
  views.clear();
  host = undefined;
  signingKey = undefined;
  idleWarned = false;
  clearTimeout(idleTimer);
  idleTimer = undefined;
}
function subscribe() {
  if (!welcome || !ssh) return;
  const union = [...new Set([...views].flatMap((view) => view.subscribed))];
  ssh.request("subscribe", { panes: union });
}
function leave(view: View) {
  if (host === view) {
    stop(
      "The connection tab closed or reloaded. Authenticate again to reconnect.",
    );
    return;
  }
  views.delete(view);
  // Its unfinished uploads would otherwise hold the connection's batches.
  for (const [batch, entry] of batches)
    if (entry.view === view) cancelBatch(batch);
  for (const entry of uploadRequests.values())
    if (entry.view === view) entry.view = undefined;
  for (const [pane, owner] of owners)
    if (owner === view) {
      owners.delete(pane);
      ssh?.request("release", { pane });
    }
  ownership();
  subscribe();
  view.port.close();
}
function replay(view: View) {
  if (!welcome) {
    if (stopped) send(view, { type: "event", event: stopped });
    return;
  }
  if (signingKey) send(view, { type: "signed_in_key", key: signingKey });
  view.revisions.clear();
  send(view, { type: "event", event: { Stream: welcome } });
  if (latest)
    send(view, {
      type: "event",
      event: {
        Stream: { ...latest, subscribed: [], snapshots: [], patches: [] },
      },
    });
  ownership();
}
function event(value: any) {
  if (value.Workspace) {
    stopped = value.Workspace.running ? undefined : value;
    for (const view of views) send(view, { type: "event", event: value });
    return;
  }
  const e = value.Stream;
  if (!e) return;
  if (isUploadEvent(e)) {
    uploadEvent(value);
    return;
  }
  if (e.type === "welcome") {
    welcome = e;
    stopped = undefined;
    for (const view of views) replay(view);
    subscribe();
    return;
  }
  if (e.type === "update") {
    latest = e;
    for (const snapshot of e.snapshots) screens.set(snapshot.pane, snapshot);
    for (const patch of e.patches) {
      const snapshot = screens.get(patch.pane);
      if (!snapshot) continue;
      for (const [index, cell] of patch.changes)
        snapshot.screen.cells[index] = cell;
      snapshot.revision = patch.revision;
      snapshot.screen.cursor = patch.cursor;
      snapshot.screen.modes = patch.modes;
    }
    for (const id of screens.keys())
      if (!e.subscribed.includes(id)) screens.delete(id);
    const full = new Set<number>(
      e.snapshots.map((snapshot: any) => snapshot.pane),
    );
    const deltas = new Map<number, any>(
      e.patches.map((patch: any) => [patch.pane, patch]),
    );
    for (const [pane] of owners)
      if (
        !claiming.has(pane) &&
        !e.leases.some(
          (lease: any) => lease.pane === pane && lease.owner === ssh?.client,
        )
      )
        owners.delete(pane);
    for (const view of views) {
      const active = view.subscribed.filter(
        (id) => e.subscribed.includes(id) && screens.has(id),
      );
      const snapshots = [],
        patches = [];
      for (const id of active) {
        const snapshot = screens.get(id)!;
        const previous = view.revisions.get(id);
        const patch = deltas.get(id);
        if (previous === undefined || full.has(id)) snapshots.push(snapshot);
        else if (previous !== snapshot.revision) {
          if (patch && patch.base === previous) patches.push(patch);
          else snapshots.push(snapshot);
        }
        view.revisions.set(id, snapshot.revision);
      }
      for (const id of view.revisions.keys())
        if (!active.includes(id)) view.revisions.delete(id);
      send(view, {
        type: "event",
        event: {
          Stream: {
            ...e,
            subscribed: active,
            snapshots,
            patches,
          },
        },
      });
    }
    ownership();
    return;
  }
  if (e.type === "lease") {
    const pending = claiming.get(e.pane) || 0;
    if (e.owner === ssh?.client) {
      if (pending > 1) claiming.set(e.pane, pending - 1);
      else claiming.delete(e.pane);
    } else if (!pending) owners.delete(e.pane);
  }
  if (e.type === "clipboard") {
    const owner = owners.get(e.pane);
    if (owner) send(owner, { type: "event", event: value });
    return;
  }
  if (e.type === "error") {
    // A rejected claim has no lease acknowledgement; require a fresh intent.
    claiming.clear();
    owners.clear();
  }
  for (const view of views) send(view, { type: "event", event: value });
  ownership();
}
class HostSession {
  client = 0;
  next = 0;
  pending = new Map<
    number,
    {
      resolve: (value: any) => void;
      reject: (error: Error) => void;
      timer: ReturnType<typeof setTimeout>;
    }
  >();
  constructor(public owner: View) {}
  request(request: string, params: any) {
    send(this.owner, { type: "host_send", request, params });
  }
  rpc(method: string, params: any = {}): Promise<any> {
    return new Promise((resolve, reject) => {
      if (this.pending.size >= 64) {
        reject(Error("Too many pending workspace requests"));
        return;
      }
      const id = ++this.next;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(
          Error("Connection tab is unavailable; request was not replayed"),
        );
      }, 15000);
      this.pending.set(id, { resolve, reject, timer });
      send(this.owner, { type: "host_rpc", id, method, params });
    });
  }
  close() {
    send(this.owner, { type: "host_stop" });
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(Error("Connection closed"));
    }
    this.pending.clear();
  }
}
async function message(view: View, data: any) {
  // Mobile browsers may suspend interval callbacks; check before any action.
  if (lockIfExpired()) return;
  view.pulse = Date.now();
  if (data.type === "pulse") return;
  if (data.type === "activity") {
    if (view.ready && ssh) active();
    return;
  }
  if (data.type === "leave") {
    leave(view);
    return;
  }
  if (data.type === "join") {
    const expected: Connection = await fetch("/connection.json").then(
      (response) => {
        if (!response.ok) throw Error("Connection configuration unavailable");
        return response.json();
      },
    );
    if (!views.has(view)) return;
    if (
      JSON.stringify(data.config) !== JSON.stringify(expected) ||
      expected.origin !== location.origin
    )
      throw Error("Connection configuration mismatch");
    const key = JSON.stringify(expected);
    if (ssh && key !== configKey)
      throw Error(
        "Connection configuration changed; log out before reconnecting",
      );
    view.ready = true;
    if (ssh) {
      replay(view);
      if (idleWarned) send(view, idleNotice());
      return;
    }
    if (data.resumeOnly) {
      send(view, { type: "closed" });
      views.delete(view);
      return;
    }
    const identity = data.identity as Identity;
    if (identity.origin !== expected.origin || identity.rpId !== expected.rpId)
      throw Error("Security key origin mismatch");
    host = view;
    signingKey = { credentialId: identity.credentialId, label: identity.label };
    configKey = key;
    idleWarningMs = minutes(expected.idle_warning_minutes, 60);
    idleLockMs = minutes(expected.idle_lock_minutes, 15);
    ssh = new HostSession(view);
    active();
    send(view, { type: "host_start" });
    return;
  }
  if (!view.ready || !ssh) throw Error("SSH login is required");
  if (data.type.startsWith("host_")) {
    if (view !== host)
      throw Error("Only the connection tab can send transport responses");
    if (data.type === "host_event") {
      if (data.event?.Stream?.type === "welcome")
        ssh.client = data.event.Stream.client;
      event(data.event);
    } else if (data.type === "host_result") {
      const pending = ssh.pending.get(data.id);
      if (pending) {
        ssh.pending.delete(data.id);
        clearTimeout(pending.timer);
        if (data.error) pending.reject(Error(data.error));
        else pending.resolve(data.result);
      }
    } else if (data.type === "host_status") {
      for (const peer of views)
        send(peer, { type: "status", message: data.message });
    } else if (data.type === "host_closed")
      stop(
        data.message ||
          "SSH connection closed. Authenticate again to reconnect.",
      );
    else throw Error("Unsupported transport response");
    return;
  }
  if (data.type === "logout") {
    stop();
    return;
  }
  // A stopped workspace answers only its state and the start request.
  const startable =
    stopped && data.type === "rpc" && ["state", "start"].includes(data.method);
  if (!welcome && !startable) throw Error("SSH login is still in progress");
  if (data.type === "request") {
    const type = data.request,
      params = data.params || {};
    if (type === "hello") return;
    // The hosting Session opts in once; views cannot toggle the shared setting.
    if (type === "clipboard") return;
    if (type === "subscribe") {
      const panes = params.panes;
      if (
        !Array.isArray(panes) ||
        panes.length > 10 ||
        panes.some((id: any) => !Number.isSafeInteger(id) || id < 1)
      )
        throw Error("Invalid subscriptions");
      const union = new Set(
        [...views]
          .filter((peer) => peer !== view)
          .flatMap((peer) => peer.subscribed)
          .concat(panes),
      );
      if (union.size > 10)
        throw Error(
          "Too many panes across browser windows; reduce a grid view before opening more",
        );
      view.subscribed = [...new Set<number>(panes)];
      subscribe();
      return;
    }
    if (type.startsWith("upload_")) {
      if (
        ![
          "upload_begin",
          "upload_chunk",
          "upload_finish",
          "upload_cancel",
        ].includes(type)
      )
        throw Error("Unsupported upload operation");
      // An upload is deliberate, like typing: it keeps the session open.
      active();
      uploadFrom(view, type, params);
      return;
    }
    if (type === "acknowledge") {
      // Notification read marks are workspace state, not a pane lease.
      if (!Number.isSafeInteger(params.notification) || params.notification < 1)
        throw Error("Invalid notification");
      active();
      ssh.request(type, { notification: params.notification });
      return;
    }
    if (type === "claim") {
      owners.set(params.pane, view);
      claiming.set(params.pane, (claiming.get(params.pane) || 0) + 1);
      active();
      ownership();
    } else if (
      ["resize", "input", "paste", "mouse", "theme", "release"].includes(type)
    ) {
      if (owners.get(params.pane) !== view) return;
      if (type === "release") owners.delete(params.pane);
      if (["input", "paste", "mouse"].includes(type)) active();
    } else if (type !== "resync") throw Error("Unsupported pane operation");
    ssh.request(type, params);
    return;
  }
  if (data.type === "rpc") {
    // Polls and passive rendering must not keep a forgotten session unlocked.
    if (!["state", "status", "history"].includes(data.method)) active();
    const result = await ssh.rpc(data.method, data.params);
    send(view, { type: "result", id: data.id, result });
    return;
  }
  throw Error("Unsupported worker message");
}
setInterval(() => {
  // Timers may be throttled or suspended; this also catches a late wake-up.
  checkIdle();
  for (const view of views) if (Date.now() - view.pulse > 120000) leave(view);
}, 10000);
(globalThis as any).onconnect = (event: MessageEvent) => {
  const port = event.ports[0];
  if (views.size >= 32) {
    port.postMessage({
      type: "closed",
      message: "Too many KelpMux browser windows",
    });
    port.close();
    return;
  }
  const view: View = {
    port,
    subscribed: [],
    pulse: Date.now(),
    ready: false,
    revisions: new Map(),
  };
  views.add(view);
  port.onmessage = ({ data }) => {
    void message(view, data).catch((error) => {
      if (data?.type === "rpc")
        send(view, { type: "result", id: data.id, error: error.message });
      else if (data?.type === "join") {
        send(view, { type: "closed", message: error.message });
        views.delete(view);
      } else
        send(view, {
          type: "event",
          event: { Stream: { type: "error", message: error.message } },
        });
    });
  };
  port.start();
};
