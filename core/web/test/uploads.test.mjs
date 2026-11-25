import { test } from "node:test";
import assert from "node:assert/strict";
import {
  CHUNK_BYTES,
  base64Bytes,
  cancelled,
  destinationProblem,
  formatBytes,
  normalizeDestination,
  pastedPaths,
  readUploadLimits,
  shellQuote,
  upload,
  uploadProblem,
} from "../src/uploads.ts";

const limits = {
  max_files: 64,
  max_file_bytes: 100 * 2 ** 20,
  max_batch_bytes: 2 ** 30,
  max_chunk_bytes: 256 * 1024,
  max_chunks_in_flight: 8,
  max_batches: 4,
};
const file = (name, size) => {
  const bytes = Uint8Array.from({ length: size }, (_, i) => i % 251);
  return {
    name,
    size,
    bytes,
    slice: (start, end) => ({
      arrayBuffer: async () => bytes.slice(start, end).buffer,
    }),
  };
};

test("paths are quoted for the shell and joined with spaces, without Enter", () => {
  assert.equal(shellQuote("/home/me/up/a.png"), "/home/me/up/a.png");
  assert.equal(shellQuote("/up/my photo.png"), "'/up/my photo.png'");
  assert.equal(shellQuote("/up/it's.txt"), "'/up/it'\\''s.txt'");
  assert.equal(shellQuote("/up/$(rm -rf ~).txt"), "'/up/$(rm -rf ~).txt'");
  assert.equal(shellQuote("/up/a;b`c`"), "'/up/a;b`c`'");
  assert.equal(
    pastedPaths(["/up/a b", "/up/c"]),
    "'/up/a b' /up/c",
    "space-separated",
  );
  assert.doesNotMatch(pastedPaths(["/up/a", "/up/b"]), /[\r\n]/);
});

test("the advertised limits are enforced before an upload starts", () => {
  assert.equal(uploadProblem([{ name: "a", size: 1 }], limits), null);
  assert.match(uploadProblem([], limits), /no files/);
  const many = Array.from({ length: 65 }, (_, i) => ({
    name: `${i}`,
    size: 1,
  }));
  assert.match(uploadProblem(many, limits), /At most 64 files/);
  assert.match(
    uploadProblem(
      [{ name: "big.iso", size: limits.max_file_bytes + 1 }],
      limits,
    ),
    /big\.iso is larger than the 100 MiB limit per file/,
  );
  const eleven = Array.from({ length: 11 }, (_, i) => ({
    name: `${i}`,
    size: 100 * 2 ** 20,
  }));
  assert.match(uploadProblem(eleven, limits), /more than the 1 GiB limit/);
  assert.equal(formatBytes(1536), "1.5 KiB");
});

test("destinations are absolute folders without . or .. or control characters", () => {
  assert.equal(destinationProblem(""), null, "the server's upload folder");
  assert.equal(destinationProblem("/home/me/project"), null);
  assert.match(destinationProblem("project"), /absolute/);
  assert.match(destinationProblem("/home/me/../other"), /\.\./);
  assert.match(destinationProblem("/home/./me"), /\./);
  assert.match(destinationProblem("/home/me\nx"), /control/);
  assert.equal(normalizeDestination(" /home/me/ "), "/home/me");
  assert.equal(normalizeDestination("/"), "/");
});

test("Welcome limits are read only when every bound is a positive integer", () => {
  assert.deepEqual(readUploadLimits(limits), limits);
  assert.equal(readUploadLimits(undefined), null);
  assert.equal(readUploadLimits({ ...limits, max_chunks_in_flight: 0 }), null);
  assert.equal(readUploadLimits({ ...limits, max_files: "64" }), null);
});

test("base64 matches the standard padded alphabet", () => {
  const bytes = Uint8Array.from([0, 1, 2, 250, 251, 255]);
  assert.equal(base64Bytes(bytes), Buffer.from(bytes).toString("base64"));
  const big = Uint8Array.from({ length: 70000 }, (_, i) => i % 256);
  assert.equal(base64Bytes(big), Buffer.from(big).toString("base64"));
});

/** A fake server connection that records requests and lets the test answer. */
function transport() {
  const sent = [];
  let handler = () => {};
  return {
    sent,
    send(type, params) {
      sent.push({ type, ...structuredClone(params) });
    },
    listen(fn) {
      handler = fn;
      return () => {
        handler = () => {};
      };
    },
    emit(event) {
      handler(event);
    },
  };
}
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

test("an upload keeps at most the window of chunks unacknowledged, then finishes", async () => {
  const t = transport();
  const files = [
    file("a.bin", CHUNK_BYTES * 10 + 5),
    file("empty", 0),
    file("b", 3),
  ];
  const progress = [];
  const done = upload(t, files, {
    limits: { ...limits, max_chunks_in_flight: 4 },
    dest: "/home/me/drop",
    onProgress: (p) => progress.push(p),
  });
  const begin = t.sent[0];
  assert.equal(begin.type, "upload_begin");
  assert.deepEqual(begin.files, [
    { name: "a.bin", size: CHUNK_BYTES * 10 + 5 },
    { name: "empty", size: 0 },
    { name: "b", size: 3 },
  ]);
  assert.equal(begin.dest, "/home/me/drop");
  assert.match(begin.request, /^[A-Za-z0-9_-]{1,128}$/);
  t.emit({
    type: "upload_started",
    request: begin.request,
    batch: 9,
    dir: "/d",
  });
  await settle();
  let chunks = t.sent.filter((m) => m.type === "upload_chunk");
  assert.equal(chunks.length, 4, "the window");
  const received = new Map();
  let acknowledged = 0;
  while (acknowledged < chunks.length) {
    const chunk = chunks[acknowledged++];
    const size = Buffer.from(chunk.data, "base64").length;
    received.set(
      chunk.file_index,
      (received.get(chunk.file_index) || 0) + size,
    );
    t.emit({
      type: "upload_progress",
      batch: 9,
      file_index: chunk.file_index,
      received: received.get(chunk.file_index),
    });
    await settle();
    chunks = t.sent.filter((m) => m.type === "upload_chunk");
    assert.ok(chunks.length - acknowledged <= 4, "never more than the window");
  }
  // 11 chunks of a.bin, none for the empty file, one for b.
  assert.equal(chunks.length, 12);
  assert.ok(chunks.every((c) => c.batch === 9));
  const a = Buffer.concat(
    chunks
      .filter((c) => c.file_index === 0)
      .map((c) => Buffer.from(c.data, "base64")),
  );
  assert.deepEqual(new Uint8Array(a), files[0].bytes);
  assert.deepEqual(
    chunks
      .filter((c) => c.file_index === 0)
      .map((c) => c.offset)
      .slice(0, 3),
    [0, CHUNK_BYTES, 2 * CHUNK_BYTES],
  );
  assert.equal(progress.at(-1).done, progress.at(-1).total);
  const finish = t.sent.at(-1);
  assert.equal(finish.type, "upload_finish");
  assert.equal(finish.batch, 9);
  t.emit({
    type: "upload_done",
    request: finish.request,
    batch: 9,
    paths: ["/d/a.bin", "/d/empty", "/d/b"],
  });
  assert.deepEqual(await done, ["/d/a.bin", "/d/empty", "/d/b"]);
});

test("cancel sends upload_cancel, even before the batch started", async () => {
  const t = transport();
  const abort = new AbortController();
  const done = upload(t, [file("a", 10)], { limits, signal: abort.signal });
  abort.abort();
  const begin = t.sent[0];
  assert.equal(t.sent.length, 1, "nothing to name before upload_started");
  t.emit({
    type: "upload_started",
    request: begin.request,
    batch: 3,
    dir: "/d",
  });
  await settle();
  const cancel = t.sent.at(-1);
  assert.equal(cancel.type, "upload_cancel");
  assert.equal(cancel.batch, 3);
  assert.equal(t.sent.filter((m) => m.type === "upload_chunk").length, 0);
  t.emit({ type: "done", request: cancel.request });
  await assert.rejects(done, (error) => cancelled(error));
});

test("server failures and refusals reject with the server's message", async () => {
  const t = transport();
  const failed = upload(t, [file("a", 10)], { limits });
  t.emit({
    type: "upload_started",
    request: t.sent[0].request,
    batch: 4,
    dir: "/d",
  });
  await settle();
  t.emit({ type: "upload_failed", batch: 5, message: "another batch" });
  t.emit({ type: "upload_failed", batch: 4, message: "disk full" });
  await assert.rejects(failed, /disk full/);

  const r = transport();
  const refused = upload(r, [file("a", 1)], { limits, dest: "/nowhere" });
  r.emit({
    type: "error",
    request: r.sent[0].request,
    message: "destination does not exist",
  });
  await assert.rejects(refused, /destination does not exist/);

  const local = transport();
  await assert.rejects(
    upload(local, [file("a", 1)], { limits, dest: "relative" }),
    /absolute/,
  );
  assert.equal(local.sent.length, 0, "refused before anything is sent");
});
