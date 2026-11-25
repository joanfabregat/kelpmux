// Files dropped onto a terminal, sent to the host where its shells run with
// KelpServer's `uploads` stream requests (decision 151a): `upload_begin`,
// windowed `upload_chunk`s acknowledged one by one with `upload_progress`,
// then `upload_finish` (or `upload_cancel`). The application owns the drop
// target and dialog; this module owns the protocol, the client-side limit
// checks and the shell quoting of the resulting paths. It imports nothing
// from the application.

/** Welcome `upload_limits`, read from the server rather than hardcoded. */
export type UploadLimits = {
  max_files: number;
  max_file_bytes: number;
  max_batch_bytes: number;
  max_chunk_bytes: number;
  max_chunks_in_flight: number;
  max_batches: number;
};
/** What an upload needs of a dropped `File`. */
export type UploadFile = {
  name: string;
  size: number;
  slice(start: number, end: number): { arrayBuffer(): Promise<ArrayBuffer> };
};
/** Bytes received by the server so far: per file, and in total. */
export type UploadProgress = {
  received: number[];
  done: number;
  total: number;
};
/** Sends stream requests and delivers this connection's upload events. */
export type UploadTransport = {
  send(type: string, params: any): void;
  listen(handler: (event: any) => void): () => void;
};

/** Largest chunk this client sends, below the server's `max_chunk_bytes`
 * when that is larger. A full window of base64 chunks (about 1.4 MiB) stays
 * well inside the 2 MiB SSH input queue, so typing keeps working during an
 * upload. */
export const CHUNK_BYTES = 128 * 1024;

const limitKeys = [
  "max_files",
  "max_file_bytes",
  "max_batch_bytes",
  "max_chunk_bytes",
  "max_chunks_in_flight",
  "max_batches",
] as const;

/** The Welcome's `upload_limits`, or null when absent or malformed. */
export function readUploadLimits(value: any): UploadLimits | null {
  if (!value || typeof value !== "object") return null;
  const limits: any = {};
  for (const key of limitKeys) {
    const n = value[key];
    if (!Number.isSafeInteger(n) || n < 1) return null;
    limits[key] = n;
  }
  return limits as UploadLimits;
}

/** Why these files cannot be uploaded as one batch, or null. */
export function uploadProblem(
  files: readonly { name: string; size: number }[],
  limits: UploadLimits,
): string | null {
  if (!files.length) return "There are no files to upload.";
  if (files.length > limits.max_files)
    return `At most ${limits.max_files} files can be uploaded at once; ${files.length} were dropped.`;
  let total = 0;
  for (const file of files) {
    if (!Number.isSafeInteger(file.size) || file.size < 0)
      return `${file.name} has no readable size.`;
    if (file.size > limits.max_file_bytes)
      return `${file.name} is larger than the ${formatBytes(limits.max_file_bytes)} limit per file.`;
    total += file.size;
  }
  if (total > limits.max_batch_bytes)
    return `These files total ${formatBytes(total)}, more than the ${formatBytes(limits.max_batch_bytes)} limit per upload.`;
  return null;
}

/** Why `path` cannot be a destination folder, or null. An empty path means
 * the server's own upload folder. The server checks the rest (an existing
 * directory owned by its user, no symlink). */
export function destinationProblem(path: string): string | null {
  if (!path) return null;
  if (!path.startsWith("/"))
    return "The destination must be an absolute folder path on the server, starting with /.";
  if (path.length > 4096) return "The destination path is too long.";
  if (/[\u0000-\u001f\u007f-\u009f]/.test(path))
    return "The destination cannot contain control characters.";
  if (path.split("/").some((part) => part === "." || part === ".."))
    return "The destination cannot contain . or .. components.";
  return null;
}

/** A destination as sent: without trailing slashes (except the root). */
export function normalizeDestination(path: string): string {
  const trimmed = path.trim();
  return trimmed.length > 1 ? trimmed.replace(/\/+$/, "") || "/" : trimmed;
}

/** One path quoted for POSIX shells: left bare when it only holds safe
 * characters, otherwise in single quotes with `'` written as `'\''`. */
export function shellQuote(path: string): string {
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(path)) return path;
  return `'${path.replace(/'/g, `'\\''`)}'`;
}

/** The text pasted into the terminal: the quoted paths, space-separated,
 * with no newline (nothing runs until the user presses Enter). */
export function pastedPaths(paths: readonly string[]): string {
  return paths.map(shellQuote).join(" ");
}

export function formatBytes(n: number): string {
  for (const [unit, factor] of [
    ["GiB", 2 ** 30],
    ["MiB", 2 ** 20],
    ["KiB", 1024],
  ] as const)
    if (n >= factor) {
      const value = n / factor;
      return `${Number.isInteger(value) ? value : value.toFixed(1)} ${unit}`;
    }
  return `${n} B`;
}

/** Standard padded base64, as `upload_chunk` expects. */
export function base64Bytes(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000)
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

/** An upload the user cancelled. */
export function cancelled(error: unknown): boolean {
  return error instanceof Error && error.name === "AbortError";
}
function abortError() {
  const error = Error("Upload cancelled");
  error.name = "AbortError";
  return error;
}

function requestId(kind: string): string {
  const random = new Uint8Array(6);
  crypto.getRandomValues(random);
  const hex = Array.from(random, (b) => b.toString(16).padStart(2, "0")).join(
    "",
  );
  return `upload-${hex}-${kind}`;
}

/**
 * Uploads `files` as one batch and resolves with their absolute paths on the
 * server, in order. At most `max_chunks_in_flight` chunks are unacknowledged
 * at any time. Aborting `signal` sends `upload_cancel` and rejects with an
 * AbortError once the server confirms; a failure rejects with the server's
 * message (the server has then removed the batch).
 */
export function upload(
  transport: UploadTransport,
  files: readonly UploadFile[],
  options: {
    limits: UploadLimits;
    dest?: string;
    signal?: AbortSignal;
    onStarted?: (dir: string) => void;
    onProgress?: (progress: UploadProgress) => void;
  },
): Promise<string[]> {
  const { limits, signal } = options;
  const problem =
    uploadProblem(files, limits) || destinationProblem(options.dest || "");
  if (problem) return Promise.reject(Error(problem));
  if (signal?.aborted) return Promise.reject(abortError());
  const chunk = Math.max(1, Math.min(limits.max_chunk_bytes, CHUNK_BYTES));
  const window = Math.max(1, limits.max_chunks_in_flight);
  const total = files.reduce((sum, file) => sum + file.size, 0);
  const begin = requestId("begin"),
    finish = requestId("finish"),
    cancel = requestId("cancel");
  return new Promise<string[]>((resolve, reject) => {
    const received = files.map(() => 0);
    let batch: number | undefined,
      settled = false,
      cancelling = false,
      finishing = false,
      reading = false,
      inFlight = 0,
      file = 0,
      offset = 0;
    const stop = transport.listen(event);
    signal?.addEventListener("abort", abort);
    function settle(error: Error | null, paths?: string[]) {
      if (settled) return;
      settled = true;
      stop();
      signal?.removeEventListener("abort", abort);
      if (error) reject(error);
      else resolve(paths || []);
    }
    function sendCancel() {
      try {
        transport.send("upload_cancel", { request: cancel, batch });
      } catch {}
    }
    function abort() {
      if (settled || cancelling) return;
      cancelling = true;
      // Before `upload_started` there is no batch to name yet; it is
      // cancelled as soon as it arrives.
      if (batch !== undefined) sendCancel();
    }
    /** A local failure (reading a file, a full SSH queue): the batch is
     * cancelled on the server without waiting for its confirmation. */
    function fail(error: unknown) {
      if (batch !== undefined && !cancelling) {
        cancelling = true;
        sendCancel();
      }
      settle(error instanceof Error ? error : Error(String(error)));
    }
    function event(e: any) {
      if (settled || !e || typeof e !== "object") return;
      switch (e.type) {
        case "upload_started":
          if (e.request !== begin) return;
          batch = e.batch;
          options.onStarted?.(e.dir);
          if (cancelling) sendCancel();
          else void pump();
          return;
        case "upload_progress":
          if (batch === undefined || e.batch !== batch) return;
          inFlight = Math.max(0, inFlight - 1);
          if (e.file_index >= 0 && e.file_index < received.length)
            received[e.file_index] = e.received;
          options.onProgress?.({
            received: [...received],
            done: received.reduce((sum, n) => sum + n, 0),
            total,
          });
          void pump();
          return;
        case "upload_failed":
          if (batch !== undefined && e.batch === batch)
            settle(Error(e.message || "The upload failed"));
          return;
        case "upload_done":
          if (e.request === finish) settle(null, e.paths);
          return;
        case "done":
          if (e.request === cancel) settle(abortError());
          return;
        case "error":
          if (e.request === begin || e.request === finish)
            settle(Error(e.message || "The upload was refused"));
          else if (e.request === cancel)
            settle(cancelling ? abortError() : Error(e.message));
          return;
      }
    }
    async function pump() {
      if (reading || settled || cancelling || batch === undefined) return;
      reading = true;
      try {
        while (!settled && !cancelling && inFlight < window) {
          // Files in order; an empty file needs no chunk.
          while (file < files.length && offset >= files[file].size) {
            file += 1;
            offset = 0;
          }
          if (file >= files.length) break;
          const end = Math.min(offset + chunk, files[file].size);
          const bytes = new Uint8Array(
            await files[file].slice(offset, end).arrayBuffer(),
          );
          if (settled || cancelling) return;
          if (bytes.length !== end - offset)
            throw Error(`${files[file].name} changed while it was read`);
          transport.send("upload_chunk", {
            batch,
            file_index: file,
            offset,
            data: base64Bytes(bytes),
          });
          inFlight += 1;
          offset = end;
        }
        if (
          !settled &&
          !cancelling &&
          !finishing &&
          file >= files.length &&
          inFlight === 0
        ) {
          finishing = true;
          transport.send("upload_finish", { request: finish, batch });
        }
      } catch (error) {
        fail(error);
      } finally {
        reading = false;
      }
    }
    try {
      const params: any = {
        request: begin,
        files: files.map((f) => ({ name: f.name, size: f.size })),
      };
      if (options.dest) params.dest = options.dest;
      transport.send("upload_begin", params);
    } catch (error) {
      fail(error);
    }
  });
}
