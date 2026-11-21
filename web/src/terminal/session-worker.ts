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
let lastActivity = Date.now();
const idleMs = 15 * 60 * 1000;
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
  for (const view of views) {
    view.ready = false;
    send(view, { type: "closed", message });
  }
  views.clear();
  host = undefined;
  signingKey = undefined;
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
  if (ssh && Date.now() - lastActivity >= idleMs) {
    stop(
      "Session locked after 15 minutes without input. Authenticate again to reconnect.",
    );
    return;
  }
  view.pulse = Date.now();
  if (data.type === "pulse") return;
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
    lastActivity = Date.now();
    ssh = new HostSession(view);
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
    if (type === "acknowledge") {
      // Notification read marks are workspace state, not a pane lease.
      if (!Number.isSafeInteger(params.notification) || params.notification < 1)
        throw Error("Invalid notification");
      lastActivity = Date.now();
      ssh.request(type, { notification: params.notification });
      return;
    }
    if (type === "claim") {
      owners.set(params.pane, view);
      claiming.set(params.pane, (claiming.get(params.pane) || 0) + 1);
      lastActivity = Date.now();
      ownership();
    } else if (
      ["resize", "input", "paste", "mouse", "theme", "release"].includes(type)
    ) {
      if (owners.get(params.pane) !== view) return;
      if (type === "release") owners.delete(params.pane);
      if (["input", "paste", "mouse"].includes(type)) lastActivity = Date.now();
    } else if (type !== "resync") throw Error("Unsupported pane operation");
    ssh.request(type, params);
    return;
  }
  if (data.type === "rpc") {
    // Polls and passive rendering must not keep a forgotten session unlocked.
    if (!["state", "status", "history"].includes(data.method))
      lastActivity = Date.now();
    const result = await ssh.rpc(data.method, data.params);
    send(view, { type: "result", id: data.id, result });
    return;
  }
  throw Error("Unsupported worker message");
}
setInterval(() => {
  if (ssh && Date.now() - lastActivity >= idleMs) {
    stop(
      "Session locked after 15 minutes without input. Authenticate again to reconnect.",
    );
    return;
  }
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
