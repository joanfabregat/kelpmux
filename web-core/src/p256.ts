// P-256 public-key recovery from an ECDSA signature (SEC 1 §4.1.6).
// Everything here handles public data, so constant time is not required.
// Affine BigInt arithmetic is slow but runs only a few times per sign-in.
export const P =
  0xffffffff00000001000000000000000000000000ffffffffffffffffffffffffn;
export const N =
  0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551n;
const B = 0x5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604bn;
export const G: Point = {
  x: 0x6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296n,
  y: 0x4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5n,
};
/** Affine point; `null` is the point at infinity. */
export type Point = { x: bigint; y: bigint } | null;

export const mod = (a: bigint, m: bigint) => ((a % m) + m) % m;
export function inverse(a: bigint, m: bigint): bigint {
  let [r0, r1] = [mod(a, m), m],
    [s0, s1] = [1n, 0n];
  if (r0 === 0n) throw Error("No inverse");
  while (r1 !== 0n) {
    const q = r0 / r1;
    [r0, r1] = [r1, r0 - q * r1];
    [s0, s1] = [s1, s0 - q * s1];
  }
  if (r0 !== 1n) throw Error("No inverse");
  return mod(s0, m);
}
export function power(base: bigint, exponent: bigint, m: bigint): bigint {
  let result = 1n;
  base = mod(base, m);
  while (exponent > 0n) {
    if (exponent & 1n) result = (result * base) % m;
    base = (base * base) % m;
    exponent >>= 1n;
  }
  return result;
}
/** Square root modulo P (P ≡ 3 mod 4), or null when `a` is not a square. */
export function sqrt(a: bigint): bigint | null {
  const root = power(a, (P + 1n) / 4n, P);
  return (root * root) % P === mod(a, P) ? root : null;
}
const curveY2 = (x: bigint) => mod(x * x * x - 3n * x + B, P);
export function onCurve(point: Point): boolean {
  if (!point) return false;
  const { x, y } = point;
  return x >= 0n && x < P && y >= 0n && y < P && mod(y * y, P) === curveY2(x);
}
export function add(a: Point, b: Point): Point {
  if (!a) return b;
  if (!b) return a;
  let slope: bigint;
  if (a.x === b.x) {
    if (mod(a.y + b.y, P) === 0n) return null;
    slope = mod((3n * a.x * a.x - 3n) * inverse(2n * a.y, P), P);
  } else slope = mod((b.y - a.y) * inverse(b.x - a.x, P), P);
  const x = mod(slope * slope - a.x - b.x, P);
  return { x, y: mod(slope * (a.x - x) - a.y, P) };
}
export function multiply(k: bigint, point: Point): Point {
  let result: Point = null,
    addend = point;
  k = mod(k, N);
  while (k > 0n) {
    if (k & 1n) result = add(result, addend);
    addend = add(addend, addend);
    k >>= 1n;
  }
  return result;
}

export const toBigInt = (bytes: Uint8Array) =>
  bytes.reduce((n, byte) => (n << 8n) | BigInt(byte), 0n);
export function toBytes(value: bigint, length = 32): Uint8Array {
  const result = new Uint8Array(length);
  for (let i = length - 1; i >= 0; i--, value >>= 8n)
    result[i] = Number(value & 0xffn);
  if (value) throw Error("Integer too large");
  return result;
}
/** Uncompressed SEC 1 encoding: 0x04 ‖ X ‖ Y. */
export const encodePoint = (point: { x: bigint; y: bigint }) => {
  const result = new Uint8Array(65);
  result[0] = 4;
  result.set(toBytes(point.x), 1);
  result.set(toBytes(point.y), 33);
  return result;
};

/** Parse a DER ECDSA-Sig-Value into (r, s). */
export function parseSignature(der: Uint8Array): { r: bigint; s: bigint } {
  let offset = 0;
  const byte = () => {
    if (offset >= der.length) throw Error("Truncated ECDSA signature");
    return der[offset++];
  };
  const length = () => {
    const n = byte();
    if (n < 128) return n;
    if (n === 129) return byte();
    throw Error("Invalid DER length");
  };
  const integer = () => {
    if (byte() !== 2) throw Error("Invalid ECDSA integer");
    const n = length();
    if (n < 1 || n > 33 || offset + n > der.length)
      throw Error("Invalid ECDSA integer size");
    const value = toBigInt(der.slice(offset, offset + n));
    offset += n;
    return value;
  };
  if (byte() !== 0x30) throw Error("Invalid ECDSA sequence");
  if (length() !== der.length - offset) throw Error("Invalid ECDSA length");
  const r = integer(),
    s = integer();
  if (offset !== der.length) throw Error("Trailing ECDSA data");
  if (r < 1n || r >= N || s < 1n || s >= N)
    throw Error("ECDSA value out of range");
  return { r, s };
}

/**
 * Candidate public keys for a signature over a message whose SHA-256 is
 * `digest`. Usually two candidates, at most four. Callers must still verify.
 */
export function recoverCandidates(
  digest: Uint8Array,
  r: bigint,
  s: bigint,
): Uint8Array[] {
  if (r < 1n || r >= N || s < 1n || s >= N) return [];
  const e = mod(toBigInt(digest.slice(0, 32)), N);
  const rInverse = inverse(r, N);
  const u1 = mod(-e * rInverse, N),
    u2 = mod(s * rInverse, N);
  const eG = multiply(u1, G);
  const found = new Map<string, Uint8Array>();
  for (const x of [r, r + N]) {
    if (x >= P) continue;
    const y = sqrt(curveY2(x));
    if (y === null) continue;
    for (const candidateY of [y, mod(P - y, P)]) {
      const R = { x, y: candidateY };
      if (!onCurve(R)) continue;
      const Q = add(eG, multiply(u2, R));
      if (!Q || !onCurve(Q)) continue;
      const encoded = encodePoint(Q);
      found.set(encoded.join(","), encoded);
    }
  }
  return [...found.values()];
}

/**
 * Recover the public keys that verify a DER ES256 signature over `message`
 * (for WebAuthn: authenticatorData ‖ SHA-256(clientDataJSON)). Every
 * returned point is checked by WebCrypto.
 */
export async function recoverPublicKeys(
  message: Uint8Array,
  der: Uint8Array,
): Promise<Uint8Array[]> {
  const { r, s } = parseSignature(der);
  const digest = new Uint8Array(
    await crypto.subtle.digest("SHA-256", message as BufferSource),
  );
  const raw = new Uint8Array(64);
  raw.set(toBytes(r), 0);
  raw.set(toBytes(s), 32);
  const verified: Uint8Array[] = [];
  for (const point of recoverCandidates(digest, r, s)) {
    try {
      const key = await crypto.subtle.importKey(
        "raw",
        point as BufferSource,
        { name: "ECDSA", namedCurve: "P-256" },
        false,
        ["verify"],
      );
      if (
        await crypto.subtle.verify(
          { name: "ECDSA", hash: "SHA-256" },
          key,
          raw as BufferSource,
          message as BufferSource,
        )
      )
        verified.push(point);
    } catch {}
  }
  return verified;
}
