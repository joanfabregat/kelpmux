import { WebAuthnAgent } from "./ssh/agent.mjs";
import { MemoryFileSystem } from "./ssh/memory-fs.mjs";
import { base64, unbase64, keyBlob } from "./ssh/wire.mjs";
import type { Connection, Identity, SignInKey } from "./types";
import { payloadLimit, queueLimit } from "./request-limits";
export {
  registeredKeys,
  selectExistingKey,
  discoverKey,
  keyFingerprint,
  mergeProfiles,
  profileRequest,
  unsupportedOperation,
} from "./keys";
/** SSH-agent progress: OpenSSH listed the identities, or asked one to sign. */
export type AuthEvent = { type: "list" } | { type: "sign"; identity: Identity };
const encoder = new TextEncoder();
const runtimeBase = "/vendor/libapps";
export function sshKey(identity: Identity): string {
  return (
    "sk-ecdsa-sha2-nistp256@openssh.com " +
    base64(keyBlob(unbase64(identity.point), identity.rpId))
  );
}
export function authorization(identity: Identity, config: Connection): string {
  if (/["\\\r\n]/.test(config.command))
    throw Error("Bridge command cannot be encoded safely");
  return `restrict,verify-required,command="${config.command}" ${sshKey(identity)} kelpmux-browser`;
}
export async function register(config: Connection): Promise<Identity> {
  const credential = (await navigator.credentials.create({
    publicKey: {
      challenge: crypto.getRandomValues(new Uint8Array(32)),
      rp: { id: config.rpId, name: "Kelpmux" },
      user: {
        id: crypto.getRandomValues(new Uint8Array(32)),
        name: config.username,
        displayName: `Kelpmux · ${config.username}`,
      },
      pubKeyCredParams: [{ type: "public-key", alg: -7 }],
      authenticatorSelection: {
        authenticatorAttachment: "cross-platform",
        residentKey: "required",
        userVerification: "required",
      },
      attestation: "none",
      timeout: 90000,
    },
  })) as PublicKeyCredential;
  if (!credential) throw Error("Registration canceled");
  const response = credential.response as AuthenticatorAttestationResponse;
  const spki = response.getPublicKey();
  if (!spki)
    throw Error("Your browser cannot export the credential public key");
  const key = await crypto.subtle.importKey(
    "spki",
    spki,
    { name: "ECDSA", namedCurve: "P-256" },
    true,
    ["verify"],
  );
  const point = await crypto.subtle.exportKey("raw", key);
  return {
    version: 1,
    rpId: config.rpId,
    origin: config.origin,
    credentialId: base64(credential.rawId),
    point: base64(point),
  };
}
export class Session {
  capabilities = new Set<string>();
  wasm: any;
  process: any;
  stream: any;
  agent: any;
  id = 0;
  client = 0;
  signedInKey?: { credentialId: string; label?: string };
  stopped = false;
  controlled = new Set<number>();
  desiredControl = new Set<number>();
  cancelSign?: () => void;
  control(pane: number, size?: { cols: number; rows: number }) {
    if (this.controlled.has(pane)) return;
    // SSH preserves ordering: claim precedes resize and the first input event.
    this.request("claim", { pane, takeover: true });
    this.desiredControl.add(pane);
    this.controlled.add(pane);
    if (size) this.request("resize", { pane, ...size });
  }
  releaseControl() {
    for (const pane of [...this.desiredControl])
      this.request("release", { pane });
  }
  pending = new Map<
    number,
    {
      resolve: (value: any) => void;
      reject: (error: Error) => void;
      timer: any;
    }
  >();
  /** The identity OpenSSH asked to sign with, once it has done so. */
  authenticated?: Identity;
  constructor(
    public config: Connection,
    public identity: SignInKey,
    public onEvent: (event: any) => void,
    public onSign: (sign: () => Promise<void>, cancel: () => void) => void,
    public onStatus: (message: string) => void,
    public onAuth: (event: AuthEvent) => void = () => {},
  ) {}
  /** The key that signed this connection's SSH login, when this page holds it. */
  signedIdentity(): Identity | undefined {
    return this.authenticated;
  }
  async connect() {
    if (!crossOriginIsolated || !globalThis.SharedArrayBuffer)
      throw Error("The SSH client requires browser cross-origin isolation");
    const [{ Background }, { RemoteReceiverWasiPreview1 }, wasm] =
      await Promise.all([
        import(/* @vite-ignore */ runtimeBase + "/wassh/js/process.js"),
        import(/* @vite-ignore */ runtimeBase + "/wassh/js/syscall_handler.js"),
        WebAssembly.instantiateStreaming(fetch("/client-core.wasm")),
      ]);
    if (this.stopped) return;
    this.wasm = wasm.instance.exports;
    this.wasm.reset();
    const { candidates, ...base } = this.identity;
    const identities = (candidates?.length ? candidates : [base.point])
      .slice(0, 4)
      .map((point) => ({
        ...base,
        point,
        blob: keyBlob(unbase64(point), base.rpId),
      }));
    this.agent = new WebAuthnAgent(
      identities,
      (challenge: Uint8Array) =>
        new Promise((resolve, reject) => {
          const abort = new AbortController();
          this.cancelSign = () => {
            abort.abort();
            reject(Error("SSH login canceled"));
          };
          this.onSign(
            async () => {
              try {
                const credential = await this.credential(
                  challenge,
                  abort.signal,
                );
                if (
                  !credential ||
                  base64(credential.rawId) !== this.identity.credentialId
                )
                  throw Error("Unexpected credential");
                resolve(credential.response);
                this.cancelSign = undefined;
              } catch (error) {
                throw error;
              }
            },
            () => {
              this.cancelSign?.();
            },
          );
        }),
      this.onStatus,
      {
        onList: () => this.onAuth({ type: "list" }),
        onSign: ({ blob, ...identity }: Identity & { blob: Uint8Array }) => {
          void blob;
          this.authenticated = identity;
          this.onAuth({ type: "sign", identity });
        },
      },
    );
    const fileSystem = new MemoryFileSystem({
      "/.ssh/known_hosts": `127.0.0.1 ${this.config.hostPublic}\n`,
    });
    const term = {
      screenSize: { width: 100, height: 30 },
      io: {
        writeUTF8: (bytes: Uint8Array) =>
          this.onStatus(new TextDecoder().decode(bytes)),
      },
    };
    const handler = new RemoteReceiverWasiPreview1({
      term,
      fileSystem,
      tcpSocketsOpen: () => this.openRelay(),
      unixSocketsOpen: (address: string) =>
        address === "/agent" ? this.agent : null,
      secureInput: () => {
        throw Error("Only hardware-key authentication is enabled");
      },
    });
    await handler.init();
    // Keep stderr in the status UI; stdout is the binary Kelpmux protocol.
    const output = handler.vfs.fds_.get(1);
    const stdout = Object.create(output);
    stdout.write = (data: Uint8Array) => {
      this.receive(data);
      return { nwritten: data.length };
    };
    handler.vfs.fds_.set(1, stdout);
    const executable = new URL("/vendor/ssh.wasm", location.origin).href;
    this.process = new Background(
      new URL("/vendor/libapps/wassh/js/worker.js", location.origin).href,
      {
        executable,
        handler,
        sabSize: 257 * 1024,
        argv: [
          "ssh",
          "-F",
          "/dev/null",
          "-T",
          "-o",
          "BatchMode=yes",
          "-o",
          "StrictHostKeyChecking=yes",
          "-o",
          "HostKeyAlgorithms=ssh-ed25519",
          "-o",
          "KexAlgorithms=curve25519-sha256",
          "-o",
          "UserKnownHostsFile=/.ssh/known_hosts",
          "-o",
          "GlobalKnownHostsFile=/dev/null",
          "-o",
          "PreferredAuthentications=publickey",
          "-o",
          "IdentityAgent=/agent",
          "-o",
          "ForwardAgent=no",
          "-o",
          "ClearAllForwardings=yes",
          "-o",
          "ServerAliveInterval=30",
          "-o",
          "ServerAliveCountMax=3",
          "-o",
          "IPQoS=none",
          "-o",
          "ConnectionAttempts=1",
          "-o",
          "ConnectTimeout=15",
          "-o",
          "LogLevel=ERROR",
          "-l",
          this.config.username,
          "127.0.0.1",
          this.config.command,
        ],
        environ: {
          HOME: "/",
          USER: this.config.username,
          TERM: "xterm-256color",
          SSH_AUTH_SOCK: "/agent",
        },
      },
    );
    const running = this.process.run();
    // stdin is the SSH channel input; messages may be queued before login finishes.
    this.send({ Stream: { type: "hello", version: 1 } });
    const exit = await running;
    if (!this.stopped) {
      this.close();
      throw Error(
        `SSH connection closed (${exit?.status ?? exit?.message ?? "disconnected"}). Reconnect to continue.`,
      );
    }
  }
  async credential(
    challenge: Uint8Array,
    signal: AbortSignal,
  ): Promise<PublicKeyCredential> {
    return (await navigator.credentials.get({
      signal,
      publicKey: {
        challenge: challenge as BufferSource,
        rpId: this.identity.rpId,
        allowCredentials: [
          { type: "public-key", id: unbase64(this.identity.credentialId) },
        ],
        userVerification: "required",
        timeout: 60000,
      },
    })) as PublicKeyCredential;
  }
  async openRelay() {
    const ws = new WebSocket(location.origin.replace(/^http/, "ws") + "/ssh");
    ws.binaryType = "arraybuffer";
    let callback: any;
    const queue: ArrayBuffer[] = [];
    const stream = {
      write: async (data: Uint8Array) => {
        const deadline = Date.now() + 10000;
        for (let offset = 0; offset < data.length; offset += 32768) {
          while (ws.bufferedAmount > 262144) {
            if (Date.now() > deadline || ws.readyState !== 1)
              throw Error("SSH relay stalled");
            await new Promise((r) => setTimeout(r, 20));
          }
          ws.send(data.slice(offset, offset + 32768).buffer as ArrayBuffer);
        }
      },
      close: () => ws.close(),
      onClose: () => {},
      get onDataAvailable() {
        return callback;
      },
      set onDataAvailable(fn: any) {
        callback = fn;
        while (queue.length) fn(queue.shift());
      },
    };
    ws.onmessage = (e) => {
      if (callback) callback(e.data);
      else if (queue.length < 16) queue.push(e.data);
      else ws.close();
    };
    ws.onclose = () => stream.onClose?.();
    await new Promise<void>((resolve, reject) => {
      ws.onopen = () => resolve();
      ws.onerror = () => reject(Error("Cannot reach SSH relay"));
    });
    this.stream = stream;
    return stream;
  }
  receive(data: Uint8Array) {
    const p = this.wasm.alloc(data.length);
    new Uint8Array(this.wasm.memory.buffer, p, data.length).set(data);
    this.wasm.receive(p, data.length);
    this.wasm.free(p, data.length);
    const text = new TextDecoder().decode(
      new Uint8Array(
        this.wasm.memory.buffer,
        this.wasm.output_ptr(),
        this.wasm.output_len(),
      ),
    );
    const result = JSON.parse(text);
    if (result.error) {
      this.onStatus(result.error);
      this.close();
      return;
    }
    for (const event of result.events) {
      if (event.Web) {
        const request = this.pending.get(event.Web.id);
        if (request) {
          clearTimeout(request.timer);
          this.pending.delete(event.Web.id);
          if (event.Web.error) request.reject(Error(event.Web.error));
          else request.resolve(event.Web.result);
        }
      } else {
        if (event.Stream?.type === "welcome") {
          this.signedInKey ??= {
            credentialId: this.identity.credentialId,
            label: this.identity.label,
          };
          this.client = event.Stream.client;
          this.capabilities = new Set(event.Stream.capabilities);
          if (this.capabilities.has("clipboard-write"))
            this.request("clipboard", { enabled: true });
          // Screens as binary records (KelpServer `binary-screens`): the
          // WASM core decodes them and returns ordinary JSON updates. A
          // relay between this page and the server must forward binary
          // records unchanged (KelpMux's web bridge does since it adopted
          // this); never send the request to a server without it.
          if (this.capabilities.has("binary-screens"))
            this.request("binary_screens", { enabled: true });
        }
        if (event.Stream?.type === "lease") {
          if (
            event.Stream.owner === this.client &&
            this.desiredControl.has(event.Stream.pane)
          )
            this.controlled.add(event.Stream.pane);
          else this.controlled.delete(event.Stream.pane);
        }
        if (event.Stream?.type === "update")
          this.controlled = new Set(
            event.Stream.leases
              .filter(
                (lease: any) =>
                  lease.owner === this.client &&
                  this.desiredControl.has(lease.pane),
              )
              .map((lease: any) => lease.pane),
          );
        this.onEvent(event);
      }
    }
  }
  send(message: any) {
    if (!this.process || this.stopped) return;
    const payload = encoder.encode(JSON.stringify(message));
    if (payload.length > payloadLimit(message))
      throw Error("Request too large");
    const frame = new Uint8Array(payload.length + 4);
    new DataView(frame.buffer).setUint32(0, payload.length);
    frame.set(payload, 4);
    const tty = this.process.handler.vfs.fds_.get(0);
    if (tty.data.length + frame.length > queueLimit(message))
      throw Error("SSH input queue is full; wait before sending more input");
    const queued = new Uint8Array(tty.data.length + frame.length);
    queued.set(tty.data);
    queued.set(frame, tty.data.length);
    tty.data = queued;
    this.process.handler.notify_?.();
  }
  request(type: string, params: any = {}) {
    if (type === "release") {
      this.controlled.delete(params.pane);
      this.desiredControl.delete(params.pane);
    }
    this.send({ Stream: { type, ...params } });
  }
  rpc(method: string, params: any = {}): Promise<any> {
    if (this.stopped) return Promise.reject(Error("SSH connection is closed"));
    return new Promise((resolve, reject) => {
      const id = ++this.id;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(
          Error(
            `${method} timed out; outcome may be unknown. Refresh before retrying.`,
          ),
        );
      }, 15000);
      this.pending.set(id, { resolve, reject, timer });
      this.send({ Web: { id, method, params } });
    });
  }
  select(
    pane: number,
    start: { x: number; y: number },
    end: { x: number; y: number },
  ): string {
    this.wasm.select(BigInt(pane), start.x, start.y, end.x, end.y);
    const result = this.wasmOutput();
    if (result.error) throw Error(result.error);
    return result.text || "";
  }
  historyScreen(lines: string[], cols: number, rows: number) {
    const bytes = encoder.encode(JSON.stringify(lines));
    const pointer = this.wasm.alloc(bytes.length);
    try {
      new Uint8Array(this.wasm.memory.buffer, pointer, bytes.length).set(bytes);
      this.wasm.layout_history(pointer, bytes.length, cols, rows);
      const result = this.wasmOutput();
      if (result.error) throw Error(result.error);
      return result.screen;
    } finally {
      this.wasm.free(pointer, bytes.length);
    }
  }
  private wasmOutput() {
    return JSON.parse(
      new TextDecoder().decode(
        new Uint8Array(
          this.wasm.memory.buffer,
          this.wasm.output_ptr(),
          this.wasm.output_len(),
        ),
      ),
    );
  }

  close() {
    this.stopped = true;
    this.desiredControl.clear();
    this.controlled.clear();
    this.cancelSign?.();
    this.cancelSign = undefined;
    this.agent?.close();
    this.stream?.close();
    this.process?.terminate();
    for (const request of this.pending.values()) {
      clearTimeout(request.timer);
      request.reject(Error("Connection closed; commands are not replayed"));
    }
    this.pending.clear();
  }
}
