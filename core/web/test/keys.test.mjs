import { test } from "node:test";
import assert from "node:assert/strict";
import { registeredKeys, selectExistingKey } from "../src/keys.ts";
import { base64, unbase64 } from "../src/ssh/wire.mjs";
const config = { origin: "https://example.test", rpId: "example.test" };
const point = base64(
  Uint8Array.from({ length: 65 }, (_, index) => (index === 0 ? 4 : index)),
);
const key = (id, label) => ({
  version: 1,
  ...config,
  point,
  credentialId: base64(new Uint8Array([id])),
  label,
});
test("saved keys remain scoped to the RP and origin and deduplicate the primary", () => {
  const desktop = key(1, "Desktop"),
    laptop = key(2, "Laptop");
  assert.equal(
    registeredKeys(config, [{ ...desktop, privateKey: "MUST_NOT_PERSIST" }])[0]
      .privateKey,
    undefined,
  );
  assert.deepEqual(
    registeredKeys(config, [desktop, laptop], desktop).map((key) => key.label),
    ["Desktop", "Laptop"],
  );
  assert.equal(
    registeredKeys(config, [
      { ...desktop, origin: "https://other.test" },
      { ...laptop, point: "bad" },
    ]).length,
    0,
  );
});
test("key display choices survive normalization without accepting arbitrary styling or secret fields", () => {
  const saved = {
    ...key(1, "Laptop"),
    emoji: "💻",
    color: "sage",
    privateKey: "NEVER_KEEP",
  };
  const restored = registeredKeys(config, [saved])[0];
  assert.equal(restored.emoji, "💻");
  assert.equal(restored.color, "sage");
  assert.equal(restored.privateKey, undefined);
  const bad = registeredKeys(config, [
    { ...saved, emoji: "<img src=x>", color: "red;background:url(x)" },
  ])[0];
  assert.equal(bad.emoji, undefined);
  assert.equal(bad.color, undefined);
});
test("selection offers every known credential and resolves the physical key before SSH", async () => {
  const desktop = key(1, "Desktop"),
    laptop = key(2, "Laptop");
  const requests = [];
  Object.defineProperty(navigator, "credentials", {
    configurable: true,
    value: {
      get: async (options) => {
        requests.push(options);
        return { rawId: unbase64(desktop.credentialId).buffer };
      },
    },
  });
  assert.equal(
    (await selectExistingKey(config, [laptop, desktop], { knownOnly: true }))
      .label,
    "Desktop",
  );
  assert.deepEqual(
    requests[0].publicKey.allowCredentials.map((value) => base64(value.id)),
    [laptop.credentialId, desktop.credentialId],
  );
  const abort = new AbortController();
  await selectExistingKey(config, [desktop], { signal: abort.signal });
  assert.equal(requests[1].signal, abort.signal);
  assert.equal(requests[1].publicKey.allowCredentials, undefined);
  await assert.rejects(
    selectExistingKey(config, [], { knownOnly: true }),
    /No registered keys/,
  );
});
test("discovery uses a cached registration or recovers the candidate public keys", async () => {
  const { discoverKey } = await import("../src/keys.ts");
  const pair = await crypto.subtle.generateKey(
    { name: "ECDSA", namedCurve: "P-256" },
    true,
    ["sign", "verify"],
  );
  const exported = base64(
    new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey)),
  );
  const authenticatorData = crypto.getRandomValues(new Uint8Array(37));
  const clientDataJSON = new TextEncoder().encode(
    JSON.stringify({ type: "webauthn.get", challenge: "x" }),
  );
  const message = new Uint8Array([
    ...authenticatorData,
    ...new Uint8Array(await crypto.subtle.digest("SHA-256", clientDataJSON)),
  ]);
  const raw = new Uint8Array(
    await crypto.subtle.sign(
      { name: "ECDSA", hash: "SHA-256" },
      pair.privateKey,
      message,
    ),
  );
  const integer = (bytes) => {
    let value = [...bytes];
    while (value.length > 1 && value[0] === 0 && !(value[1] & 0x80))
      value = value.slice(1);
    if (value[0] & 0x80) value = [0, ...value];
    return [2, value.length, ...value];
  };
  const body = [...integer(raw.slice(0, 32)), ...integer(raw.slice(32))];
  const signature = Uint8Array.from([0x30, body.length, ...body]);
  const requests = [];
  const rawId = new Uint8Array([7, 7, 7]);
  Object.defineProperty(navigator, "credentials", {
    configurable: true,
    value: {
      get: async (options) => {
        requests.push(options);
        return {
          rawId: rawId.buffer,
          response: {
            authenticatorData: authenticatorData.buffer,
            clientDataJSON: clientDataJSON.buffer,
            signature: signature.buffer,
          },
        };
      },
    },
  });
  const found = await discoverKey(config, [key(1, "Desktop")]);
  assert.equal(requests[0].publicKey.allowCredentials, undefined);
  assert.equal(requests[0].publicKey.userVerification, "required");
  assert.equal(requests[0].publicKey.rpId, config.rpId);
  assert.equal(found.credentialId, base64(rawId));
  assert.ok(found.candidates.includes(exported));
  assert.ok(found.candidates.length >= 1 && found.candidates.length <= 4);
  assert.equal(found.point, found.candidates[0]);
  const cached = { ...key(7, "Laptop"), credentialId: base64(rawId) };
  const known = await discoverKey(config, [cached]);
  assert.equal(known.label, "Laptop");
  assert.equal(known.candidates, undefined);
});
test("fingerprints match OpenSSH and server profiles replace local display fields", async () => {
  const {
    keyFingerprint,
    mergeProfiles,
    profileRequest,
    unsupportedOperation,
  } = await import("../src/keys.ts");
  const { keyBlob } = await import("../src/ssh/wire.mjs");
  const { createHash } = await import("node:crypto");
  const desktop = { ...key(1, "Desktop"), emoji: "💻" };
  const expected =
    "SHA256:" +
    createHash("sha256")
      .update(keyBlob(unbase64(desktop.point), desktop.rpId))
      .digest("base64")
      .replace(/=+$/, "");
  assert.equal(await keyFingerprint(desktop), expected);
  const laptop = { ...key(2, "Laptop"), fingerprint: "SHA256:laptop" };
  const plain = { ...key(3), fingerprint: "SHA256:plain" };
  const { keys, missing } = mergeProfiles(
    [{ ...desktop, fingerprint: expected }, laptop, plain],
    [
      { fingerprint: expected, label: "Office", color: "sage", updated: 1 },
      { fingerprint: "SHA256:other", label: "Elsewhere" },
      { fingerprint: "SHA256:bad", emoji: "<img>" },
      null,
    ],
  );
  assert.equal(keys[0].label, "Office");
  assert.equal(keys[0].color, "sage");
  assert.equal(keys[0].emoji, undefined);
  assert.equal(keys[1].label, "Laptop");
  assert.deepEqual(
    missing.map((item) => item.label),
    ["Laptop"],
  );
  assert.deepEqual(profileRequest(laptop), {
    fingerprint: "SHA256:laptop",
    label: "Laptop",
  });
  assert.equal(
    mergeProfiles([laptop], { not: "a list" }).keys[0].label,
    "Laptop",
  );
  assert.ok(unsupportedOperation(Error("unsupported web operation")));
  assert.ok(!unsupportedOperation(Error("timed out")));
});
