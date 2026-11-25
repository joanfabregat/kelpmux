import { ECDH } from "node:crypto";
import { decode, reader, KEY_TYPE } from "../../src/ssh/wire.mjs";
export function validatePublicKey(value, expectedRp) {
  if (
    typeof value !== "string" ||
    value.length > 1024 ||
    !/^[A-Za-z0-9+/]+={0,2}$/.test(value)
  )
    throw Error("Invalid public key");
  const data = Buffer.from(value, "base64");
  const r = reader(data);
  if (decode(r.string()) !== KEY_TYPE || decode(r.string()) !== "nistp256")
    throw Error("Unsupported public key");
  const point = r.string();
  if (
    point.length !== 65 ||
    point[0] !== 4 ||
    decode(r.string()) !== expectedRp
  )
    throw Error("Unexpected key or relying party");
  ECDH.convertKey(point, "prime256v1");
  r.end();
  return data.toString("base64");
}
