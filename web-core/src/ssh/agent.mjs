import {
  base64,
  bytes,
  concat,
  keyBlob,
  packet,
  reader,
  string,
  u32,
  webauthnSignature,
} from "./wire.mjs";

// An SSH-agent protocol adapter. Only identities and signing are supported;
// there is no agent forwarding, private-key import or arbitrary credential access.
// Several identities can share one credential: after discovery the browser
// knows only the candidate public keys recovered from a signature. OpenSSH
// queries the server with each public key before asking for a signature, so
// only an authorized candidate is ever signed for.
export class WebAuthnAgent {
  constructor(
    identities,
    sign,
    report = (message) => {
      void message;
    },
    events = {},
  ) {
    this.identities = Array.isArray(identities) ? identities : [identities];
    if (!this.identities.length || this.identities.length > 4)
      throw Error("Agent supports one to four identities");
    this.sign = sign;
    this.report = report;
    this.events = events;
    this.pending = new Uint8Array();
    this.chain = Promise.resolve();
    this.closed = false;
  }
  write(chunk) {
    if (this.closed) throw Error("Agent closed");
    this.pending = concat(this.pending, chunk);
    if (this.pending.length > 131072) throw Error("Agent queue limit");
    while (this.pending.length >= 4) {
      const length = new DataView(
        this.pending.buffer,
        this.pending.byteOffset,
        4,
      ).getUint32(0);
      if (length < 1 || length > 65536) throw Error("Agent packet limit");
      if (this.pending.length < length + 4) break;
      const message = this.pending.slice(4, length + 4);
      this.pending = this.pending.slice(length + 4);
      this.chain = this.chain.then(async () => {
        let reply;
        try {
          reply = await this.request(message);
        } catch (error) {
          this.report(`Signing denied: ${error.message}`);
          reply = packet(5);
        }
        if (!this.closed) this.onDataAvailable?.(reply);
      });
    }
  }
  async request(message) {
    const r = reader(message);
    const type = r.u8();
    if (type === 11) {
      r.end();
      this.events.onList?.();
      return packet(
        12,
        u32(this.identities.length),
        ...this.identities.flatMap((identity) => [
          string(identity.blob),
          string("Kelpmux browser security key"),
        ]),
      );
    }
    if (type !== 13) return packet(5);
    const requestedKey = r.string();
    const challenge = r.string();
    const flags = r.u32();
    r.end();
    const identity = this.identities.find(
      (item) => base64(item.blob) === base64(requestedKey),
    );
    if (flags !== 0 || !identity) throw Error("Unsupported signing request");
    this.events.onSign?.(identity);
    this.report("Approve the security-key request in your browser.");
    const assertion = await this.sign(challenge, identity);
    const auth = bytes(assertion.authenticatorData);
    const expectedRp = new Uint8Array(
      await crypto.subtle.digest(
        "SHA-256",
        new TextEncoder().encode(identity.rpId),
      ),
    );
    if (base64(auth.slice(0, 32)) !== base64(expectedRp))
      throw Error("Unexpected relying party");
    const signature = webauthnSignature(assertion, challenge, identity.origin);
    this.report("WebAuthn signature sent to OpenSSH for verification.");
    return packet(14, string(signature));
  }
  close() {
    this.closed = true;
    this.pending = new Uint8Array();
  }
}
export { keyBlob };
