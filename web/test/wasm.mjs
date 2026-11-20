import { readFileSync } from "node:fs";
import assert from "node:assert/strict";
const { instance } = await WebAssembly.instantiate(
  readFileSync(new URL("../public/client-core.wasm", import.meta.url)),
);
const w = instance.exports,
  encoder = new TextEncoder(),
  decoder = new TextDecoder();
const lines = encoder.encode(JSON.stringify(["A界e\u0301👩‍💻Z"]));
const lp = w.alloc(lines.length);
new Uint8Array(w.memory.buffer, lp, lines.length).set(lines);
w.layout_history(lp, lines.length, 12, 2);
const laidOut = JSON.parse(
  decoder.decode(
    new Uint8Array(w.memory.buffer, w.output_ptr(), w.output_len()),
  ),
).screen;
w.free(lp, lines.length);
assert.equal(laidOut.cells[1].width, 2);
assert.equal(laidOut.cells[3].text, "e\u0301");
assert.equal(laidOut.cells[4].text, "👩‍💻");
assert.equal(laidOut.cells[6].text, "Z");
for (let n = 0; n < 1000; n++) {
  w.reset();
  const body = encoder.encode(
    JSON.stringify({
      Stream: {
        type: "welcome",
        version: 1,
        client: 1,
        epoch: "test",
        capabilities: [],
      },
    }),
  );
  const frame = new Uint8Array(body.length + 4);
  new DataView(frame.buffer).setUint32(0, body.length);
  frame.set(body, 4);
  for (const chunk of [frame.slice(0, 3), frame.slice(3)]) {
    const p = w.alloc(chunk.length);
    new Uint8Array(w.memory.buffer, p, chunk.length).set(chunk);
    w.receive(p, chunk.length);
    w.free(p, chunk.length);
    const value = JSON.parse(
      decoder.decode(
        new Uint8Array(w.memory.buffer, w.output_ptr(), w.output_len()),
      ),
    );
    assert.equal(value.error, undefined);
  }
}
console.log(
  "PASS: WASM framing and allocation ABI survive repeated fragmented reconnects",
);
