import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  G,
  N,
  P,
  add,
  encodePoint,
  multiply,
  onCurve,
  parseSignature,
  recoverCandidates,
  recoverPublicKeys,
  sqrt,
  toBytes,
} from "../src/p256.ts";

const hex = (value) => Buffer.from(value).toString("hex");
const big = (value) => BigInt("0x" + value);
// DER-encode an (r, s) pair the way authenticators return ES256 signatures.
function der(r, s) {
  const integer = (value) => {
    let bytes = [...toBytes(value)];
    while (bytes.length > 1 && bytes[0] === 0 && !(bytes[1] & 0x80))
      bytes = bytes.slice(1);
    if (bytes[0] & 0x80) bytes = [0, ...bytes];
    return [2, bytes.length, ...bytes];
  };
  const body = [...integer(r), ...integer(s)];
  return Uint8Array.from([0x30, body.length, ...body]);
}

// RFC 6979 A.2.5, P-256 with SHA-256.
const rfc = {
  x: big("C9AFA9D845BA75166B5C215767B1D6934E50C3DB36E89B127B8A622B120F6721"),
  ux: big("60FED4BA255A9D31C961EB74C6356D68C049B8923B61FA6CE669622E60F29FB6"),
  uy: big("7903FE1008B8BC99A41AE9E95628BC64F2F1B20C2D7E9F5177A3C294D4462299"),
  signatures: [
    {
      message: "sample",
      r: big(
        "EFD48B2AACB6A8FD1140DD9CD45E81D69D2C877B56AAF991C34D0EA84EAF3716",
      ),
      s: big(
        "F7CB1C942D657C41D436C7A1B6E29F65F3E900DBB9AFF4064DC4AB2F843ACDA8",
      ),
    },
    {
      message: "test",
      r: big(
        "F1ABB023518351CD71D881567B1EA663ED3EFCF6C5132B354F28D3B0B7D38367",
      ),
      s: big(
        "019F4113742A2B14BD25926B49C649155F267E60D3814B4C0CC84250E46F0083",
      ),
    },
  ],
};

test("curve arithmetic matches the P-256 generator, order and RFC 6979 key", () => {
  assert.ok(onCurve(G));
  assert.equal(multiply(N, G), null);
  assert.deepEqual(multiply(N - 1n, G), { x: G.x, y: P - G.y });
  assert.deepEqual(add(G, G), multiply(2n, G));
  assert.deepEqual(multiply(rfc.x, G), { x: rfc.ux, y: rfc.uy });
  const y = sqrt(rfc.uy * rfc.uy);
  assert.ok(y === rfc.uy || y === P - rfc.uy);
  // 3 is not a quadratic residue modulo P.
  assert.equal(sqrt(3n), null);
});

test("recovery returns the RFC 6979 public key among the candidates", async () => {
  const expected = hex(encodePoint({ x: rfc.ux, y: rfc.uy }));
  for (const { message, r, s } of rfc.signatures) {
    const digest = createHash("sha256").update(message).digest();
    const candidates = recoverCandidates(digest, r, s).map(hex);
    assert.ok(candidates.length >= 1 && candidates.length <= 4);
    assert.ok(candidates.includes(expected), message);
    const verified = (
      await recoverPublicKeys(new TextEncoder().encode(message), der(r, s))
    ).map(hex);
    assert.ok(verified.includes(expected), message);
    for (const point of verified) assert.equal(point.slice(0, 2), "04");
  }
});

test("recovery finds a WebCrypto key from a WebAuthn-shaped signature", async () => {
  for (let round = 0; round < 8; round++) {
    const pair = await crypto.subtle.generateKey(
      { name: "ECDSA", namedCurve: "P-256" },
      true,
      ["sign", "verify"],
    );
    const exported = hex(
      new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey)),
    );
    // authenticatorData ‖ SHA-256(clientDataJSON), as signed for WebAuthn.
    const clientData = createHash("sha256")
      .update(JSON.stringify({ type: "webauthn.get", round }))
      .digest();
    const message = Uint8Array.from([
      ...crypto.getRandomValues(new Uint8Array(37)),
      ...clientData,
    ]);
    const raw = new Uint8Array(
      await crypto.subtle.sign(
        { name: "ECDSA", hash: "SHA-256" },
        pair.privateKey,
        message,
      ),
    );
    const r = big(hex(raw.slice(0, 32))),
      s = big(hex(raw.slice(32)));
    const recovered = (await recoverPublicKeys(message, der(r, s))).map(hex);
    assert.ok(recovered.includes(exported));
    assert.ok(recovered.length >= 1 && recovered.length <= 4);
  }
});

test("signature parsing rejects malformed or out-of-range values", () => {
  assert.deepEqual(parseSignature(der(1n, 2n)), { r: 1n, s: 2n });
  assert.throws(() => parseSignature(Uint8Array.of(0x31, 0)));
  assert.throws(() => parseSignature(der(0n, 1n)), /out of range/);
  assert.throws(() => parseSignature(der(N, 1n)), /out of range/);
  assert.throws(() => parseSignature(Uint8Array.from([...der(1n, 2n), 0])));
  assert.deepEqual(recoverCandidates(new Uint8Array(32), 0n, 1n), []);
});
