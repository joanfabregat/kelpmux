// A typed client for KelpServer's `files` stream requests (file browsers and
// editors acting on the host where the shells run; see KelpCore's
// native-client-protocol.md, "Files"). Each call sends one correlated
// `file_*` request and resolves with its reply, or rejects with a
// `FileError` carrying the server's error code. Replies may arrive in any
// order and between other events; they are matched by request identifier.
// At most the server's `max_in_flight` requests run at once: further calls
// wait in a queue, and a `busy` refusal (other windows sharing the
// connection, or the server's own bound) is retried a few times.
// Downloads (`file-downloads`) start with a correlated request, then stream
// chunks named by the server's download identifier, each acknowledged once
// the caller's sink has written it (see `download`; `save.ts` saves to the
// user's computer).
// The application supplies the transport, as for uploads; this module
// imports nothing from it, nor from React or the DOM.

/** Welcome `file_limits`, read from the server rather than hardcoded. */
export type FileLimits = {
  max_read_bytes: number;
  max_write_bytes: number;
  max_list_entries: number;
  max_paths: number;
  max_in_flight: number;
  max_path_bytes: number;
  max_depth: number;
  /** Downloads (`file-downloads`), absent from servers without them. */
  downloads?: DownloadLimits;
  /** The primary file root: every path must lie inside it or inside an
   * extra root (`roots`); the server user's home directory unless the
   * server names another folder. Start browsing there and offer nothing
   * outside the roots (`isInside` in `paths.ts`): the server refuses such
   * paths with `outside_root`. Absent from servers without the boundary. */
  root?: string;
  /** Every file root with a label, the primary root first, then the extra
   * roots (folders outside it, such as a data drive). A move between two
   * roots is refused (`cross_root`). Absent from servers without extra
   * roots; then `root` alone applies. */
  roots?: FileRoot[];
};
/** One of Welcome `file_limits.roots`. */
export type FileRoot = {
  /** Canonical absolute path. */
  path: string;
  /** A short name: `Home`, the folder's name, or its full path. */
  label: string;
};
/** Welcome `file_limits.downloads`. */
export type DownloadLimits = {
  /** Bytes per chunk. */
  chunk_bytes: number;
  /** Chunks the server sends before the oldest is acknowledged. */
  max_chunks_in_flight: number;
  /** Unfinished downloads per connection. */
  max_downloads: number;
  max_archive_entries: number;
  max_archive_bytes: number;
};
/** Archive formats a download can use. */
export type DownloadArchive = "tar.gz";
/** What `file_download_started` announced. */
export type DownloadStart = {
  /** The suggested file name: the file's, or `<folder>.tar.gz`. */
  name: string;
  /** A single file's size when the server opened it. */
  size?: number;
  archive?: DownloadArchive;
  /** Archives: entries and input bytes the server's planning walk found. */
  entries?: number;
  inputBytes?: number;
};
export type DownloadProgress = {
  /** Bytes received and written so far. */
  received: number;
  /** A single file's size, when known. */
  size?: number;
  /** Archives: input bytes the server has read, of `inputBytes`. */
  input?: number;
  inputBytes?: number;
};
export type DownloadResult = {
  name: string;
  archive?: DownloadArchive;
  /** Bytes received. */
  size: number;
  /** SHA-256 of the bytes, as the server computed it. */
  hash: string;
  /** Archive entries left out (special files, unreadable entries). */
  skipped: number;
  /** Files that changed while the server read them. */
  changed: number;
};
/** Where a download's bytes go. `write` may return a promise: the next
 * chunk is acknowledged only once it settles, so a slow disk slows the
 * server rather than filling memory. */
export type DownloadSink = {
  start?(start: DownloadStart): void | Promise<void>;
  write(bytes: Uint8Array): void | Promise<void>;
};
export type DownloadOptions = {
  archive?: DownloadArchive;
  sink: DownloadSink;
  onProgress?: (progress: DownloadProgress) => void;
  /** Aborting cancels the download on the server; the call rejects with
   * code `cancelled`. */
  signal?: AbortSignal;
};
export type FileKind = "file" | "dir" | "symlink" | "other";
/** One entry as `lstat` sees it: a symlink is described, not followed. */
export type FileEntry = {
  name: string;
  kind: FileKind;
  /** Bytes (a symlink's link length). */
  size: number;
  /** Unix seconds. */
  modified: number;
  /** Permission bits, set-id and sticky included. */
  mode: number;
  /** A symlink's target, as text. */
  target?: string;
};
export type FileListing = {
  /** The listed directory, normalised and absolute (`~` expanded). */
  path: string;
  /** Directories first, then the other entries, each in byte order. */
  entries: FileEntry[];
  /** Entries were left out by the server's bounds. */
  truncated: boolean;
  /** Names that are not UTF-8, which cannot be addressed. */
  skipped: number;
};
export type FileContent = {
  path: string;
  size: number;
  modified: number;
  mode: number;
  /** SHA-256 of the content: the `baseHash` of a later write. */
  hash: string;
  /** The bytes hold NUL or are not UTF-8. */
  binary: boolean;
  /** The text, for a file that is not binary. */
  text?: string;
  /** The bytes, for a binary file. */
  bytes?: Uint8Array;
};
export type FileWritten = {
  path: string;
  hash: string;
  size: number;
  modified: number;
  created: boolean;
};
export type FileErrorCode =
  | "invalid_path"
  | "invalid_request"
  | "invalid_content"
  | "not_found"
  | "permission_denied"
  | "not_a_directory"
  | "is_a_directory"
  | "not_a_file"
  | "symlink"
  | "exists"
  | "not_empty"
  | "conflict"
  | "too_large"
  | "cross_device"
  | "read_only"
  | "no_space"
  | "busy"
  | "io"
  /** The path, or a symlink on the way, leads outside the file root. */
  | "outside_root"
  /** A rename whose two ends lie in different file roots. */
  | "cross_root"
  /** Client side: the connection closed or the reply never came. */
  | "closed"
  | "timeout"
  /** Client side: the download was cancelled. */
  | "cancelled";
/** The server's error body (`file_error.error`). */
export type FileErrorBody = {
  code: string;
  message: string;
  path?: string;
  current_hash?: string;
  size?: number;
};
export type FileRemoval = {
  removed: { path: string; trashed_as?: string }[];
  failed: { path: string; error: FileError }[];
};
/** Sends stream requests and delivers this connection's file events. */
export type FilesTransport = {
  send(type: string, params: any): void;
  listen(handler: (event: any) => void): () => void;
};

/** A failed or refused file request. `code` is the server's code, or
 * `closed` / `timeout` when no reply could be had. */
export class FileError extends Error {
  code: FileErrorCode | string;
  path?: string;
  /** `conflict`: the hash of the content now on disk, when it was hashed. */
  currentHash?: string;
  /** `too_large`: the file's size. */
  size?: number;
  constructor(body: FileErrorBody) {
    super(body.message || describeCode(body.code));
    this.name = "FileError";
    this.code = body.code;
    if (body.path) this.path = body.path;
    if (body.current_hash) this.currentHash = body.current_hash;
    if (typeof body.size === "number") this.size = body.size;
  }
}
const descriptions: Record<string, string> = {
  invalid_path: "The path is not valid",
  invalid_request: "The request was refused",
  invalid_content: "The content could not be decoded",
  not_found: "No such file or folder",
  permission_denied: "Permission denied",
  not_a_directory: "Not a folder",
  is_a_directory: "This is a folder",
  not_a_file: "Not a regular file",
  symlink: "This is a symbolic link",
  exists: "An entry with this name already exists",
  not_empty: "The folder is not empty",
  conflict: "The file changed on disk",
  too_large: "Too large",
  cross_device: "On another file system",
  read_only: "Read-only file system",
  no_space: "No space left on the device",
  busy: "The server is busy with other file operations",
  io: "Input/output error",
  outside_root: "Outside the folders this server lets you browse",
  cross_root: "Entries cannot move between these folders",
  closed: "The connection is closed",
  timeout: "No reply from the server; the outcome is unknown",
  cancelled: "The download was cancelled",
};
/** A short description of an error code, for a message without one. */
export function describeCode(code: string): string {
  return descriptions[code] || code;
}

const limitKeys = [
  "max_read_bytes",
  "max_write_bytes",
  "max_list_entries",
  "max_paths",
  "max_in_flight",
  "max_path_bytes",
  "max_depth",
] as const;

/** The Welcome's `file_limits`, or null when absent or malformed. */
export function readFileLimits(value: any): FileLimits | null {
  if (!value || typeof value !== "object") return null;
  const limits: any = {};
  for (const key of limitKeys) {
    const n = value[key];
    if (!Number.isSafeInteger(n) || n < 1) return null;
    limits[key] = n;
  }
  const downloads = readDownloadLimits(value.downloads);
  if (downloads) limits.downloads = downloads;
  // An absolute path without NUL, at most `max_path_bytes`; a malformed one
  // is dropped (the server still enforces its roots).
  const root = readRootPath(value.root, limits.max_path_bytes);
  if (root) limits.root = root;
  // Only with `root`, which must come first; malformed entries are dropped.
  if (root && Array.isArray(value.roots)) {
    const roots: FileRoot[] = [];
    for (const item of value.roots.slice(0, MAX_ROOTS)) {
      const path = readRootPath(item?.path, limits.max_path_bytes);
      const label = item?.label;
      if (
        path &&
        typeof label === "string" &&
        label.length > 0 &&
        label.length <= limits.max_path_bytes &&
        !roots.some((other) => other.path === path)
      )
        roots.push({ path, label });
    }
    if (roots[0]?.path === root) limits.roots = roots;
  }
  return limits as FileLimits;
}
/** Most roots read from `file_limits.roots` (the server's bound of extra
 * roots, plus the primary one). */
const MAX_ROOTS = 65;
function readRootPath(path: any, maxBytes: number): string | undefined {
  if (
    typeof path !== "string" ||
    !path.startsWith("/") ||
    path.includes("\0") ||
    new TextEncoder().encode(path).length > maxBytes
  )
    return undefined;
  return path.length > 1 ? path.replace(/\/+$/, "") : path;
}
const downloadKeys = [
  "chunk_bytes",
  "max_chunks_in_flight",
  "max_downloads",
  "max_archive_entries",
  "max_archive_bytes",
] as const;
function readDownloadLimits(value: any): DownloadLimits | undefined {
  if (!value || typeof value !== "object") return undefined;
  const limits: any = {};
  for (const key of downloadKeys) {
    const n = value[key];
    if (!Number.isSafeInteger(n) || n < 1) return undefined;
    limits[key] = n;
  }
  return limits as DownloadLimits;
}

/** Request identifiers of this module start with this prefix. */
export const FILE_REQUEST_PREFIX = "file-";
/** Whether a stream event belongs to a file request: a `file_*` reply, or
 * the `done` / plain `error` answering one. */
export function isFileEvent(e: any): boolean {
  if (!e || typeof e.type !== "string") return false;
  if (e.type.startsWith("file_")) return true;
  return (
    (e.type === "done" || e.type === "error") &&
    typeof e.request === "string" &&
    e.request.startsWith(FILE_REQUEST_PREFIX)
  );
}

/** Standard padded base64 to bytes. */
export function base64ToBytes(text: string): Uint8Array {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}
function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000)
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

/** The reply event type that completes each request type. */
const replies: Record<string, string> = {
  file_list: "file_list",
  file_stat: "file_stat",
  file_read: "file_content",
  file_write: "file_written",
  file_mkdir: "done",
  file_rename: "done",
  file_trash: "file_removed",
  file_delete: "file_removed",
  file_download_begin: "file_download_started",
};

type Pending = {
  type: string;
  params: any;
  resolve: (event: any) => void;
  reject: (error: FileError) => void;
  attempts: number;
  timer?: ReturnType<typeof setTimeout>;
  /** Called with the reply before the promise settles, in the same event
   * (a download registers itself before its first chunk is handled). */
  onReply?: (event: any) => void;
};
/** A started download: its chunks are written in order, each acknowledged
 * once written. */
type Download = {
  /** Writes chain here, so chunks reach the sink in order. */
  writing: Promise<void>;
  /** A chunk, the end or an error arrived; restarts the stall timer. */
  event(e: any): void;
  fail(error: FileError): void;
};

export type FilesClientOptions = {
  limits: FileLimits;
  /** How long to wait for a reply before giving up (default 120 s). The
   * server cannot cancel a request, so its outcome is then unknown. A
   * download's start is bounded the same way, so a call always settles; a
   * download that starts after its call gave up is cancelled when its start
   * arrives. Once started, a download fails after this long without an
   * event while nothing is being written. */
  timeoutMs?: number;
  /** Retries of a request refused as `busy` (default 5). */
  busyRetries?: number;
  /** Delay before retrying a `busy` request (default 300 ms). */
  busyDelayMs?: number;
  /** False when something between the browser and the server (an older
   * relay) cannot carry downloads, although the server sends them. */
  downloads?: boolean;
};

/** Begin requests given up on that are remembered (`abandoned`). */
const MAX_ABANDONED = 64;

function cancelledError() {
  return new FileError({ code: "cancelled", message: describeCode("cancelled") });
}

let serial = 0;
function requestId(): string {
  const random = new Uint8Array(4);
  crypto.getRandomValues(random);
  const hex = Array.from(random, (b) => b.toString(16).padStart(2, "0")).join(
    "",
  );
  return `${FILE_REQUEST_PREFIX}${hex}-${++serial}`;
}

export class FilesClient {
  readonly limits: FileLimits;
  private transport: FilesTransport;
  private timeoutMs: number;
  private busyRetries: number;
  private busyDelayMs: number;
  private relaysDownloads: boolean;
  /** Requests sent and waiting for their reply, by identifier. */
  private running = new Map<string, Pending>();
  /** Requests waiting for a free slot, in order. */
  private queue: Pending[] = [];
  /** Requests refused as `busy`, waiting to be retried. */
  private retrying = new Set<Pending>();
  /** Started downloads, by the server's identifier. */
  private downloads = new Map<number, Download>();
  /** Identifiers of `file_download_begin` requests given up on (their
   * reply timed out), oldest first: a download that starts after all is
   * cancelled at once rather than left to hold one of the connection's
   * download slots until the server's stall timeout. */
  private abandoned = new Set<string>();
  private stop: () => void;
  private closed = false;

  constructor(transport: FilesTransport, options: FilesClientOptions) {
    this.transport = transport;
    this.limits = options.limits;
    this.timeoutMs = options.timeoutMs ?? 120_000;
    this.busyRetries = options.busyRetries ?? 5;
    this.busyDelayMs = options.busyDelayMs ?? 300;
    this.relaysDownloads = options.downloads ?? true;
    this.stop = transport.listen((event) => this.event(event));
  }

  /** A directory's entries. `path` is absolute, `~` or starts with `~/`. */
  async list(path: string): Promise<FileListing> {
    const e = await this.request("file_list", { path });
    return {
      path: e.path,
      entries: e.entries,
      truncated: !!e.truncated,
      skipped: e.skipped || 0,
    };
  }
  /** One entry, not following a final symlink. */
  async stat(path: string): Promise<{ path: string; entry: FileEntry }> {
    const e = await this.request("file_stat", { path });
    return { path: e.path, entry: e.entry };
  }
  /** A regular file's whole content, or `too_large` (with its `size`) when
   * it is larger than `maxBytes` (default: the server's `max_read_bytes`). */
  async read(path: string, options: { maxBytes?: number } = {}) {
    const params: any = { path };
    if (options.maxBytes !== undefined) params.max_bytes = options.maxBytes;
    const e = await this.request("file_read", params);
    const content: FileContent = {
      path: e.path,
      size: e.size,
      modified: e.modified,
      mode: e.mode,
      hash: e.hash,
      binary: !!e.binary,
    };
    if (e.binary)
      content.bytes =
        e.encoding === "base64"
          ? base64ToBytes(e.content)
          : new TextEncoder().encode(e.content);
    else
      content.text =
        e.encoding === "base64"
          ? new TextDecoder().decode(base64ToBytes(e.content))
          : e.content;
    return content;
  }
  /** Replaces a file atomically, or creates it with `create`. With
   * `baseHash`, a file changed since that hash is not written: the call
   * rejects with code `conflict`. Text is sent as UTF-8, bytes as base64. */
  async write(
    path: string,
    content: string | Uint8Array,
    options: { baseHash?: string; create?: boolean } = {},
  ): Promise<FileWritten> {
    const size =
      typeof content === "string"
        ? new TextEncoder().encode(content).length
        : content.length;
    if (size > this.limits.max_write_bytes)
      throw new FileError({
        code: "too_large",
        message: `The file is larger than the server's ${this.limits.max_write_bytes}-byte write limit.`,
        size,
      });
    const params: any =
      typeof content === "string"
        ? { path, content }
        : { path, content: bytesToBase64(content), encoding: "base64" };
    if (options.baseHash) params.base_hash = options.baseHash;
    if (options.create) params.create = true;
    const e = await this.request("file_write", params);
    return {
      path: e.path,
      hash: e.hash,
      size: e.size,
      modified: e.modified,
      created: !!e.created,
    };
  }
  /** Creates one folder (its parent must exist). */
  async mkdir(path: string): Promise<void> {
    await this.request("file_mkdir", { path });
  }
  /** Renames or moves an entry within one file system; an existing `to` is
   * replaced only with `overwrite`. */
  async rename(
    from: string,
    to: string,
    options: { overwrite?: boolean } = {},
  ): Promise<void> {
    const params: any = { from, to };
    if (options.overwrite) params.overwrite = true;
    await this.request("file_rename", params);
  }
  /** Moves entries to the freedesktop home trash. */
  trash(paths: string[]): Promise<FileRemoval> {
    return this.removal("file_trash", paths);
  }
  /** Deletes entries permanently, folders with their contents. */
  delete(paths: string[]): Promise<FileRemoval> {
    return this.removal("file_delete", paths);
  }
  /** Whether the server sends files for download (`file-downloads`). */
  get canDownload(): boolean {
    return this.relaysDownloads && !!this.limits.downloads;
  }
  /** Downloads a regular file as itself, or files and folders as one
   * archive (`archive: "tar.gz"`; required for a folder or several paths).
   * Bytes go to `sink.write` in order; each chunk is acknowledged once
   * written, which paces the server. Resolves when every byte is written. */
  download(paths: string[], options: DownloadOptions): Promise<DownloadResult> {
    const { sink, onProgress, signal } = options;
    if (!this.canDownload)
      return Promise.reject(
        new FileError({
          code: "invalid_request",
          message: "This server does not send files for download.",
        }),
      );
    if (!paths.length || paths.length > this.limits.max_paths)
      return Promise.reject(
        new FileError({
          code: "invalid_request",
          message: `Download 1 to ${this.limits.max_paths} entries at once.`,
        }),
      );
    if (signal?.aborted) return Promise.reject(cancelledError());
    return new Promise<DownloadResult>((resolve, reject) => {
      /** The server's identifier, once started. */
      let id: number | undefined;
      let start: DownloadStart | undefined;
      let ended = false;
      /** Bytes received (`expected`) and written and acknowledged. */
      let expected = 0;
      let received = 0;
      let writes = 0;
      let stall: ReturnType<typeof setTimeout> | undefined;
      const state: Download = {
        writing: Promise.resolve(),
        event: (e) => handle(e),
        fail: (error) => finish(error),
      };
      const cancelOnServer = () => {
        if (id === undefined || this.closed) return;
        try {
          this.transport.send("file_download_cancel", {
            request: requestId(),
            download: id,
          });
        } catch {
          // The connection is gone, and the download with it.
        }
      };
      const finish = (error?: FileError, result?: DownloadResult) => {
        if (ended) return;
        ended = true;
        clearTimeout(stall);
        signal?.removeEventListener("abort", abort);
        if (id !== undefined) this.downloads.delete(id);
        if (error) reject(error);
        else resolve(result!);
      };
      const failLocally = (error: FileError) => {
        if (ended) return;
        cancelOnServer();
        finish(error);
      };
      // No event for `timeoutMs` while nothing is being written: the
      // server or the connection stalled.
      const arm = () => {
        clearTimeout(stall);
        stall = setTimeout(() => {
          if (writes > 0) return arm();
          failLocally(
            new FileError({ code: "timeout", message: describeCode("timeout") }),
          );
        }, this.timeoutMs);
      };
      const abort = () => failLocally(cancelledError());
      signal?.addEventListener("abort", abort);
      const handle = (e: any) => {
        if (ended) return;
        arm();
        if (e.type === "file_download_chunk") {
          let bytes: Uint8Array;
          try {
            bytes = base64ToBytes(e.data);
          } catch {
            bytes = new Uint8Array();
          }
          if (e.offset !== expected || !bytes.length) {
            failLocally(
              new FileError({
                code: "io",
                message: "The download arrived out of order",
              }),
            );
            return;
          }
          expected += bytes.length;
          const end = expected;
          const input = typeof e.input === "number" ? e.input : undefined;
          writes += 1;
          state.writing = state.writing
            .then(async () => {
              if (ended) return;
              await sink.write(bytes);
              if (ended) return;
              received = end;
              this.transport.send("file_download_ack", {
                download: id,
                offset: end,
              });
              arm();
              onProgress?.({
                received,
                size: start?.size,
                input,
                inputBytes: start?.inputBytes,
              });
            })
            .catch((error) =>
              failLocally(
                error instanceof FileError
                  ? error
                  : new FileError({
                      code: "io",
                      message: `The download could not be saved: ${error instanceof Error ? error.message : String(error)}`,
                    }),
              ),
            )
            .finally(() => {
              writes -= 1;
            });
        } else if (e.type === "file_download_done") {
          state.writing = state.writing.then(() => {
            if (ended) return;
            if (e.size !== received)
              return failLocally(
                new FileError({
                  code: "io",
                  message: "The download ended before all of its bytes arrived",
                }),
              );
            finish(undefined, {
              name: start!.name,
              archive: start!.archive,
              size: received,
              hash: e.hash,
              skipped: e.skipped || 0,
              changed: e.changed || 0,
            });
          });
        } else if (e.type === "file_error")
          finish(new FileError(e.error || { code: "io", message: "" }));
      };
      const onReply = (e: any) => {
        if (e.type !== "file_download_started") return;
        id = e.download;
        // Aborted while starting: stop it at once.
        if (ended) return cancelOnServer();
        start = {
          name: e.name,
          size: typeof e.size === "number" ? e.size : undefined,
          archive: e.archive,
          entries: e.entries,
          inputBytes: e.input_bytes,
        };
        this.downloads.set(e.download, state);
        arm();
        const begun = start;
        writes += 1;
        state.writing = state.writing
          .then(() => sink.start?.(begun))
          .catch((error) =>
            failLocally(
              error instanceof FileError
                ? error
                : new FileError({
                    code: "io",
                    message: error instanceof Error ? error.message : String(error),
                  }),
            ),
          )
          .finally(() => {
            writes -= 1;
          });
      };
      const params: any = { paths };
      if (options.archive) params.archive = options.archive;
      this.request("file_download_begin", params, onReply).catch((error) =>
        finish(error),
      );
    });
  }
  /** Rejects every waiting call and stops listening. */
  close() {
    if (this.closed) return;
    this.closed = true;
    this.stop();
    const error = () =>
      new FileError({ code: "closed", message: describeCode("closed") });
    for (const download of [...this.downloads.values()])
      download.fail(error());
    this.downloads.clear();
    this.abandoned.clear();
    for (const pending of [
      ...this.running.values(),
      ...this.queue,
      ...this.retrying,
    ]) {
      clearTimeout(pending.timer);
      pending.reject(error());
    }
    this.running.clear();
    this.queue = [];
    this.retrying.clear();
  }

  private async removal(type: string, paths: string[]): Promise<FileRemoval> {
    if (!paths.length) return { removed: [], failed: [] };
    if (paths.length > this.limits.max_paths)
      throw new FileError({
        code: "invalid_request",
        message: `At most ${this.limits.max_paths} entries can be removed at once.`,
      });
    const e = await this.request(type, { paths });
    return {
      removed: e.removed || [],
      failed: (e.failed || []).map((f: any) => ({
        path: f.path,
        error: new FileError(f.error),
      })),
    };
  }
  private request(
    type: string,
    params: any,
    onReply?: (event: any) => void,
  ): Promise<any> {
    if (this.closed)
      return Promise.reject(
        new FileError({ code: "closed", message: describeCode("closed") }),
      );
    return new Promise((resolve, reject) => {
      this.queue.push({ type, params, resolve, reject, attempts: 0, onReply });
      this.pump();
    });
  }
  private pump() {
    while (
      !this.closed &&
      this.queue.length &&
      this.running.size < Math.max(1, this.limits.max_in_flight)
    )
      this.dispatch(this.queue.shift()!);
  }
  private dispatch(pending: Pending) {
    const id = requestId();
    this.running.set(id, pending);
    pending.timer = setTimeout(() => {
      if (this.running.get(id) !== pending) return;
      this.running.delete(id);
      if (pending.type === "file_download_begin") this.abandon(id);
      pending.reject(
        new FileError({ code: "timeout", message: describeCode("timeout") }),
      );
      this.pump();
    }, this.timeoutMs);
    try {
      this.transport.send(pending.type, { request: id, ...pending.params });
    } catch (error) {
      clearTimeout(pending.timer);
      this.running.delete(id);
      const message = error instanceof Error ? error.message : String(error);
      pending.reject(
        new FileError({
          code: /too large/i.test(message) ? "too_large" : "closed",
          message,
        }),
      );
      this.pump();
    }
  }
  /** Remember a begin request given up on, within a bound (the oldest is
   * forgotten; its download, if it ever starts, ends at the server's stall
   * timeout instead). */
  private abandon(id: string) {
    this.abandoned.add(id);
    if (this.abandoned.size > MAX_ABANDONED)
      this.abandoned.delete(this.abandoned.values().next().value!);
  }
  /** The reply to a begin request given up on: a download that started
   * after all is cancelled at once. */
  private orphan(e: any) {
    if (e.type !== "file_download_started" || typeof e.download !== "number")
      return;
    try {
      this.transport.send("file_download_cancel", {
        request: requestId(),
        download: e.download,
      });
    } catch {
      // The connection is gone, and the download with it.
    }
  }
  private event(e: any) {
    // A started download's chunks, end and failure name the download.
    if (e && typeof e.download === "number") {
      const download = this.downloads.get(e.download);
      if (
        download &&
        ["file_download_chunk", "file_download_done", "file_error"].includes(
          e.type,
        )
      ) {
        download.event(e);
        return;
      }
      if (e.type === "file_download_chunk") return;
    }
    if (!e || typeof e.request !== "string") return;
    const pending = this.running.get(e.request);
    if (!pending) {
      if (this.abandoned.delete(e.request)) this.orphan(e);
      return;
    }
    let outcome: "resolve" | FileError;
    if (e.type === replies[pending.type]) outcome = "resolve";
    else if (e.type === "file_error")
      outcome = new FileError(
        e.error || { code: "io", message: "The request failed" },
      );
    else if (e.type === "error")
      outcome = new FileError({
        code: "invalid_request",
        message: e.message || "The request was refused",
      });
    else return;
    clearTimeout(pending.timer);
    this.running.delete(e.request);
    if (
      outcome !== "resolve" &&
      outcome.code === "busy" &&
      pending.attempts < this.busyRetries
    ) {
      pending.attempts += 1;
      this.retrying.add(pending);
      setTimeout(() => {
        if (!this.retrying.delete(pending) || this.closed) return;
        this.queue.unshift(pending);
        this.pump();
      }, this.busyDelayMs * pending.attempts);
    } else if (outcome === "resolve") {
      pending.onReply?.(e);
      pending.resolve(e);
    }
    else pending.reject(outcome);
    this.pump();
  }
}
