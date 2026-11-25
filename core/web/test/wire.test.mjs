import test from "node:test";
import assert from "node:assert/strict";
import { createECDH, generateKeyPairSync, sign, createHash } from "node:crypto";
import {
  base64,
  concat,
  decode,
  ecdsaSignature,
  keyBlob,
  packet,
  reader,
  string,
  u32,
  webauthnSignature,
  SIGNATURE_TYPE,
} from "../src/ssh/wire.mjs";
import { validatePublicKey } from "./fixtures/key-validation.mjs";
import { WebAuthnAgent } from "../src/ssh/agent.mjs";
const rp = "terminal.example.com";
function point() {
  const ecdh = createECDH("prime256v1");
  ecdh.generateKeys();
  return ecdh.getPublicKey();
}

test("public-key enrollment rejects other key types, RP IDs, malformed points and trailing bytes", () => {
  const key = keyBlob(point(), rp);
  assert.equal(validatePublicKey(base64(key), rp), base64(key));
  assert.throws(() => validatePublicKey(base64(key), "other.example"));
  assert.throws(() =>
    validatePublicKey(base64(concat(key, Uint8Array.of(0))), rp),
  );
  assert.throws(() =>
    validatePublicKey(
      base64(concat(string("ssh-ed25519"), string(new Uint8Array(32)))),
      rp,
    ),
  );
  assert.throws(() =>
    validatePublicKey(
      base64(keyBlob(concat(Uint8Array.of(4), new Uint8Array(64)), rp)),
      rp,
    ),
  );
  assert.throws(() => validatePublicKey("A".repeat(2048), rp));
});

test("SSH reader rejects truncation and trailing data", () => {
  assert.throws(() => reader(Uint8Array.of(0, 0, 0, 8, 1)).string());
  assert.throws(() => reader(Uint8Array.of(1)).end());
});

test("WebAuthn assertion produces the exact OpenSSH signature envelope and validates context", () => {
  const { privateKey } = generateKeyPairSync("ec", {
    namedCurve: "prime256v1",
  });
  const challenge = Uint8Array.of(1, 2, 3);
  const origin = `https://${rp}`;
  const clientDataJSON = new TextEncoder().encode(
    JSON.stringify({
      type: "webauthn.get",
      challenge: "AQID",
      origin,
      crossOrigin: false,
    }),
  );
  const auth = concat(
    createHash("sha256").update(rp).digest(),
    Uint8Array.of(5),
    u32(7),
  );
  const signature = sign(
    "sha256",
    concat(auth, createHash("sha256").update(clientDataJSON).digest()),
    privateKey,
  );
  const assertion = { authenticatorData: auth, clientDataJSON, signature };
  const r = reader(webauthnSignature(assertion, challenge, origin));
  assert.equal(decode(r.string()), SIGNATURE_TYPE);
  assert.deepEqual(r.string(), ecdsaSignature(signature));
  assert.equal(r.u8(), 5);
  assert.equal(r.u32(), 7);
  assert.equal(decode(r.string()), origin);
  assert.deepEqual(r.string(), clientDataJSON);
  assert.equal(r.string().length, 0);
  r.end();
  assert.throws(() => webauthnSignature(assertion, Uint8Array.of(8), origin));
  assert.throws(() =>
    webauthnSignature(assertion, challenge, "https://other.example"),
  );
  const unverified = auth.slice();
  unverified[32] = 1;
  assert.throws(() =>
    webauthnSignature(
      { ...assertion, authenticatorData: unverified },
      challenge,
      origin,
    ),
  );
});

test("agent handles fragmented identities request and refuses unadvertised signing keys", async () => {
  const identity = {
    blob: keyBlob(point(), rp),
    rpId: rp,
    origin: `https://${rp}`,
  };
  let called = false;
  const agent = new WebAuthnAgent(identity, () => {
    called = true;
  });
  const replies = [];
  agent.onDataAvailable = (data) => replies.push(data);
  const request = packet(11);
  agent.write(request.slice(0, 2));
  agent.write(request.slice(2));
  await agent.chain;
  const outer = reader(replies[0]);
  const inner = reader(outer.string());
  outer.end();
  assert.equal(inner.u8(), 12);
  assert.equal(inner.u32(), 1);
  assert.deepEqual(inner.string(), identity.blob);
  agent.write(
    packet(13, string(keyBlob(point(), rp)), string("challenge"), u32(0)),
  );
  await agent.chain;
  assert.equal(called, false);
  assert.deepEqual(replies[1], packet(5));
  assert.throws(() => agent.write(concat(u32(1000000), Uint8Array.of(11))));
});

test("agent lists every discovered candidate and signs only for the one OpenSSH requests", async () => {
  const candidates = [point(), point()].map((value) => ({
    blob: keyBlob(value, rp),
    rpId: rp,
    origin: `https://${rp}`,
  }));
  const events = [];
  const signed = [];
  const agent = new WebAuthnAgent(
    candidates,
    async (challenge, identity) => {
      signed.push(identity);
      throw Error("stop after selection");
    },
    () => {},
    {
      onList: () => events.push("list"),
      onSign: (identity) => events.push(identity),
    },
  );
  const replies = [];
  agent.onDataAvailable = (data) => replies.push(data);
  agent.write(packet(11));
  await agent.chain;
  const inner = reader(reader(replies[0]).string());
  assert.equal(inner.u8(), 12);
  assert.equal(inner.u32(), 2);
  assert.deepEqual(inner.string(), candidates[0].blob);
  assert.equal(decode(inner.string()), "Kelpmux browser security key");
  assert.deepEqual(inner.string(), candidates[1].blob);
  inner.string();
  inner.end();
  agent.write(packet(13, string(candidates[1].blob), string("c"), u32(0)));
  await agent.chain;
  assert.deepEqual(events, ["list", candidates[1]]);
  assert.deepEqual(signed, [candidates[1]]);
  assert.deepEqual(replies[1], packet(5));
  assert.throws(() => new WebAuthnAgent([], () => {}));
});
