// Bounded SSH wire primitives, shared by the page, agent adapter and tests.
const encoder = new TextEncoder();
export const text = (value) => encoder.encode(value);
export const decode = (value) =>
  new TextDecoder("utf-8", { fatal: true }).decode(value);
export const bytes = (value) =>
  typeof value === "string" ? text(value) : new Uint8Array(value);
export function concat(...parts) {
  parts = parts.map(bytes);
  const result = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const part of parts) {
    result.set(part, offset);
    offset += part.length;
  }
  return result;
}
export function u32(n) {
  if (!Number.isInteger(n) || n < 0 || n > 0xffffffff)
    throw Error("Invalid uint32");
  const result = new Uint8Array(4);
  new DataView(result.buffer).setUint32(0, n);
  return result;
}
export const string = (value) => {
  const b = bytes(value);
  return concat(u32(b.length), b);
};
export const packet = (type, ...parts) =>
  string(concat(Uint8Array.of(type), ...parts));
export function reader(value) {
  const b = bytes(value);
  let offset = 0;
  const take = (length) => {
    if (!Number.isInteger(length) || length < 0 || length > b.length - offset)
      throw Error("Truncated SSH message");
    const part = b.slice(offset, offset + length);
    offset += length;
    return part;
  };
  const uint32 = () => new DataView(take(4).buffer).getUint32(0);
  return {
    take,
    u8: () => take(1)[0],
    u32: uint32,
    string: () => take(uint32()),
    end: () => {
      if (offset !== b.length) throw Error("Trailing SSH data");
    },
  };
}
export const base64 = (b) => btoa(String.fromCharCode(...bytes(b)));
export const unbase64 = (s) =>
  Uint8Array.from(atob(s.replaceAll("-", "+").replaceAll("_", "/")), (c) =>
    c.charCodeAt(0),
  );
export function mpint(value) {
  let b = bytes(value);
  while (b.length > 1 && b[0] === 0) b = b.slice(1);
  if (b[0] & 128) b = concat(Uint8Array.of(0), b);
  return string(b);
}
export function ecdsaSignature(der) {
  const r = reader(der);
  const length = () => {
    const n = r.u8();
    if (n < 128) return n;
    if (n === 129) return r.u8();
    throw Error("Invalid DER length");
  };
  if (r.u8() !== 0x30) throw Error("Invalid ECDSA sequence");
  const size = length();
  const body = reader(r.take(size));
  r.end();
  const integer = () => {
    if (body.u8() !== 2) throw Error("Invalid ECDSA integer");
    const n = body.u8();
    if (n > 33 || n < 1) throw Error("Invalid ECDSA integer size");
    return body.take(n);
  };
  const result = concat(mpint(integer()), mpint(integer()));
  body.end();
  return result;
}
export const KEY_TYPE = "sk-ecdsa-sha2-nistp256@openssh.com";
export const SIGNATURE_TYPE = "webauthn-sk-ecdsa-sha2-nistp256@openssh.com";
export function keyBlob(point, rpId) {
  point = bytes(point);
  if (point.length !== 65 || point[0] !== 4)
    throw Error("Expected uncompressed P-256 point");
  return concat(
    string(KEY_TYPE),
    string("nistp256"),
    string(point),
    string(rpId),
  );
}
export function webauthnSignature(
  assertion,
  expectedChallenge,
  expectedOrigin,
) {
  const clientData = bytes(assertion.clientDataJSON);
  const client = JSON.parse(decode(clientData));
  if (
    client.type !== "webauthn.get" ||
    client.origin !== expectedOrigin ||
    client.crossOrigin === true ||
    base64(unbase64(client.challenge)) !== base64(expectedChallenge)
  )
    throw Error("Unexpected WebAuthn client data");
  const auth = bytes(assertion.authenticatorData);
  if (auth.length < 37 || (auth[32] & 5) !== 5 || auth[32] & 0x40)
    throw Error("User presence and verification required");
  if (!(auth[32] & 0x80) && auth.length !== 37)
    throw Error("Unexpected authenticator extensions");
  return concat(
    string(SIGNATURE_TYPE),
    string(ecdsaSignature(assertion.signature)),
    auth.slice(32, 37),
    string(expectedOrigin),
    string(clientData),
    string(auth.slice(37)),
  );
}
