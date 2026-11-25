import { base64, concat, keyBlob, unbase64 } from "./ssh/wire.mjs";
import type { Connection, Identity, KeyProfile, SignInKey } from "./types";
import { keyColors, keyEmojis } from "./key-style.ts";
import { recoverPublicKeys } from "./p256.ts";
/** Display fields accepted from storage, imports or the server. */
export function profileFields(
  item: Partial<Pick<Identity, "label" | "emoji" | "color">>,
): Pick<Identity, "label" | "emoji" | "color"> {
  return {
    label:
      typeof item.label === "string"
        ? item.label.replace(/[\x00-\x1f\x7f]/g, "").slice(0, 40)
        : undefined,
    emoji:
      typeof item.emoji === "string" &&
      keyEmojis.some((e) => e.value === item.emoji)
        ? item.emoji
        : undefined,
    color:
      typeof item.color === "string" && Object.hasOwn(keyColors, item.color)
        ? item.color
        : undefined,
  };
}
export function registeredKeys(
  config: Connection,
  saved: Identity[],
  primary?: Identity | null,
): Identity[] {
  const result = new Map<string, Identity>();
  for (const item of [...saved, ...(primary ? [primary] : [])]) {
    try {
      if (
        !item ||
        item.version !== 1 ||
        item.rpId !== config.rpId ||
        item.origin !== config.origin ||
        typeof item.credentialId !== "string" ||
        typeof item.point !== "string"
      )
        continue;
      const id = unbase64(item.credentialId),
        point = unbase64(item.point);
      if (
        !id.length ||
        id.length > 1024 ||
        point.length !== 65 ||
        point[0] !== 4
      )
        continue;
      const key = {
        version: 1 as const,
        rpId: config.rpId,
        origin: config.origin,
        credentialId: base64(id),
        point: base64(point),
        fingerprint:
          typeof item.fingerprint === "string"
            ? item.fingerprint.slice(0, 128)
            : undefined,
        ...profileFields(item),
      };
      result.set(key.credentialId, key);
    } catch {}
  }
  return [...result.values()].slice(-32);
}
/** OpenSSH fingerprint: `SHA256:` + unpadded base64 of SHA-256(key blob). */
export async function keyFingerprint(
  identity: Pick<Identity, "point" | "rpId">,
): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    keyBlob(unbase64(identity.point), identity.rpId) as BufferSource,
  );
  return "SHA256:" + base64(new Uint8Array(digest)).replace(/=+$/, "");
}
/**
 * Apply server profiles to cached keys. A server profile replaces the
 * label, icon and colour of the key with the same fingerprint. `missing`
 * lists cached keys with local display choices but no server profile.
 */
export function mergeProfiles(
  keys: Identity[],
  profiles: unknown,
): { keys: Identity[]; missing: Identity[] } {
  const byFingerprint = new Map<string, KeyProfile>();
  if (Array.isArray(profiles))
    for (const profile of profiles.slice(0, 256))
      if (
        profile &&
        typeof profile.fingerprint === "string" &&
        profile.fingerprint.startsWith("SHA256:")
      )
        byFingerprint.set(profile.fingerprint, profile);
  const missing: Identity[] = [];
  const merged = keys.map((key) => {
    const profile = key.fingerprint && byFingerprint.get(key.fingerprint);
    if (profile) return { ...key, ...profileFields(profile) };
    if (key.fingerprint && (key.label || key.emoji || key.color))
      missing.push(key);
    return key;
  });
  return { keys: merged, missing };
}
/** Parameters for `key_profile_set`: the whole display profile. */
export function profileRequest(key: Identity): KeyProfile {
  return {
    fingerprint: key.fingerprint!,
    ...(key.label ? { label: key.label } : {}),
    ...(key.emoji ? { emoji: key.emoji } : {}),
    ...(key.color ? { color: key.color } : {}),
  };
}
/** Old bridges reject management methods they do not know. */
export const unsupportedOperation = (error: unknown) =>
  /unsupported web operation/i.test(String((error as Error)?.message ?? error));
/**
 * Ask the authenticator to choose one of its credentials for this website.
 * A cached registration is used directly. Otherwise the public key is
 * recovered from the assertion signature: the result carries every candidate
 * point, and SSH decides which one (if any) is authorized.
 */
export async function discoverKey(
  config: Connection,
  saved: Identity[],
  signal?: AbortSignal,
): Promise<SignInKey> {
  const credential = (await navigator.credentials.get({
    signal,
    publicKey: {
      challenge: crypto.getRandomValues(new Uint8Array(32)),
      rpId: config.rpId,
      userVerification: "required",
      timeout: 90000,
    },
  })) as PublicKeyCredential | null;
  if (!credential) throw Error("Key selection canceled");
  const credentialId = base64(credential.rawId);
  const known = registeredKeys(config, saved).find(
    (key) => key.credentialId === credentialId,
  );
  if (known) return known;
  const response = credential.response as AuthenticatorAssertionResponse;
  const clientData = new Uint8Array(
    await crypto.subtle.digest("SHA-256", response.clientDataJSON),
  );
  const points = await recoverPublicKeys(
    concat(new Uint8Array(response.authenticatorData), clientData),
    new Uint8Array(response.signature),
  );
  if (!points.length)
    throw Error("This security key returned a signature KelpMux cannot use.");
  const candidates = points.slice(0, 4).map((point) => base64(point));
  return {
    version: 1,
    rpId: config.rpId,
    origin: config.origin,
    credentialId,
    point: candidates[0],
    candidates,
  };
}
export async function selectExistingKey(
  config: Connection,
  saved: Identity[],
  options: { signal?: AbortSignal; knownOnly?: boolean } = {},
): Promise<Identity> {
  const known = registeredKeys(config, saved);
  if (options.knownOnly && !known.length)
    throw Error("No registered keys are saved for this website.");
  const credential = (await navigator.credentials.get({
    signal: options.signal,
    publicKey: {
      challenge: crypto.getRandomValues(new Uint8Array(32)),
      rpId: config.rpId,
      userVerification: "required",
      timeout: 90000,
      ...(options.knownOnly
        ? {
            allowCredentials: known.map((key) => ({
              type: "public-key" as const,
              id: unbase64(key.credentialId) as BufferSource,
            })),
          }
        : {}),
    },
  })) as PublicKeyCredential | null;
  if (!credential) throw Error("Key selection canceled");
  const id = base64(credential.rawId);
  const identity = known.find(
    (item) =>
      item.credentialId === id &&
      item.rpId === config.rpId &&
      item.origin === config.origin,
  );
  if (!identity)
    throw Error(
      "Key found. Import its KelpMux registration file below so this browser can display its public key.",
    );
  return identity;
}
