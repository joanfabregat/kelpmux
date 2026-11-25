import { Session } from "./connection";

/** Window-side view of a memory-only SSH connection. No transport secrets cross the port. */
export class SharedSession extends Session {
  port?: MessagePort;
  host?: Session;
  private ended?: () => void;
  private heartbeat?: ReturnType<typeof setInterval>;
  private pageGone = () => this.detach();
  /** The shared connection's idle warning: shown with the time it locks,
   * or `null` once it is lifted or the view ends (session-worker.ts). */
  onIdle?: (notice: { idleFor: string; lockAt: number } | null) => void;
  /** Reports user activity in this window to the idle lock. */
  activity() {
    if (!this.stopped) this.port?.postMessage({ type: "activity" });
  }
  signedIdentity() {
    return this.host?.signedIdentity() ?? super.signedIdentity();
  }
  async connect(resumeOnly = false) {
    if (!globalThis.SharedWorker) {
      if (resumeOnly) return;
      return super.connect();
    }
    const module = await WebAssembly.instantiateStreaming(
      fetch("/client-core.wasm"),
    );
    this.wasm = module.instance.exports;
    this.wasm.reset();
    // The connection page must remain open. Other pages can reload or close
    // without moving SSH secrets out of the connection page.
    let worker: SharedWorker;
    try {
      worker = new SharedWorker(
        new URL("./session-worker.ts", import.meta.url),
        { name: "kelpmux-ssh-v1" },
      );
    } catch {
      if (resumeOnly) return;
      this.onStatus(
        "Connection sharing is unavailable in this browser; opening a separate SSH login.",
      );
      return super.connect();
    }
    this.port = worker.port;
    const ended = new Promise<void>((resolve) => {
      this.ended = resolve;
    });
    worker.onerror = (event) => {
      this.onStatus(
        `Shared SSH worker failed: ${event.message || "reconnect to continue"}`,
      );
      this.finish();
    };
    this.port.onmessage = async ({ data }) => {
      if (data.type === "signed_in_key") {
        this.signedInKey = data.key;
        this.onEvent({ SignedInKey: data.key });
      } else if (data.type === "event") {
        const bytes = new TextEncoder().encode(JSON.stringify(data.event));
        const frame = new Uint8Array(bytes.length + 4);
        new DataView(frame.buffer).setUint32(0, bytes.length);
        frame.set(bytes, 4);
        this.receive(frame);
      } else if (data.type === "owned") {
        this.controlled = new Set(data.panes);
        this.desiredControl = new Set(data.panes);
      } else if (data.type === "result") {
        const pending = this.pending.get(data.id);
        if (!pending) return;
        clearTimeout(pending.timer);
        this.pending.delete(data.id);
        if (data.error) pending.reject(Error(data.error));
        else pending.resolve(data.result);
      } else if (data.type === "host_start") {
        if (this.host) return;
        const native = (this.host = new Session(
          this.config,
          this.identity,
          (event) => this.port?.postMessage({ type: "host_event", event }),
          this.onSign,
          (message) => this.port?.postMessage({ type: "host_status", message }),
          this.onAuth,
        ));
        const closed = (message?: string) =>
          this.port?.postMessage({ type: "host_closed", message });
        void native
          .connect()
          .then(() => closed())
          .catch((error) => closed(error.message));
        this.onStatus(
          "This tab holds the SSH connection. Keep it open while using other windows.",
        );
      } else if (data.type === "host_send")
        this.host?.request(data.request, data.params);
      else if (data.type === "host_rpc") {
        try {
          const result = await this.host!.rpc(data.method, data.params);
          this.port?.postMessage({ type: "host_result", id: data.id, result });
        } catch (error: any) {
          this.port?.postMessage({
            type: "host_result",
            id: data.id,
            error: error.message,
          });
        }
      } else if (data.type === "host_stop") {
        this.host?.close();
        this.host = undefined;
      } else if (data.type === "idle")
        this.onIdle?.(
          data.warning ? { idleFor: data.idleFor, lockAt: data.lockAt } : null,
        );
      else if (data.type === "status") this.onStatus(data.message);
      else if (data.type === "closed") {
        if (data.message) this.onStatus(data.message);
        this.finish();
      }
    };
    this.port.start();
    this.port.postMessage({
      type: "join",
      config: this.config,
      identity: this.identity,
      resumeOnly,
    });
    const pulse = () => this.port?.postMessage({ type: "pulse" });
    this.heartbeat = setInterval(pulse, 10000);
    window.addEventListener("pagehide", this.pageGone);
    await ended;
  }
  request(type: string, params: any = {}) {
    if (!this.port) return super.request(type, params);
    if (type === "release") {
      this.controlled.delete(params.pane);
      this.desiredControl.delete(params.pane);
    }
    this.port.postMessage({ type: "request", request: type, params });
  }
  rpc(method: string, params: any = {}): Promise<any> {
    if (!this.port) return super.rpc(method, params);
    if (this.stopped) return Promise.reject(Error("SSH connection is closed"));
    return new Promise((resolve, reject) => {
      const id = ++this.id;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(Error(`${method} timed out; commands are not replayed`));
      }, 15000);
      this.pending.set(id, { resolve, reject, timer });
      this.port!.postMessage({ type: "rpc", id, method, params });
    });
  }
  private finish() {
    if (this.stopped) return;
    this.stopped = true;
    this.host?.close();
    this.host = undefined;
    clearInterval(this.heartbeat);
    window.removeEventListener("pagehide", this.pageGone);
    this.port?.close();
    this.port = undefined;
    this.controlled.clear();
    this.desiredControl.clear();
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(Error("SSH view closed; commands are not replayed"));
    }
    this.pending.clear();
    this.onIdle?.(null);
    this.ended?.();
  }
  detach() {
    if (!this.port) {
      super.close();
      return;
    }
    this.port.postMessage({ type: "leave" });
    this.finish();
  }
  close() {
    if (!this.port) {
      super.close();
      return;
    }
    this.port.postMessage({ type: "logout" });
    this.finish();
  }
}
