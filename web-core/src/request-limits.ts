// Size limits for bridge requests sent over the SSH channel. They mirror the
// `kelpmux web-bridge` limit: every request is capped at 640 KiB of JSON,
// and the SSH input queue holds at most 2 MiB, except `file_write`
// (KelpServer `files`), which carries up to 2 MiB of file content that JSON
// escaping can grow to six times its size (\u00XX), plus a small envelope.
// The bridge itself accepts records up to the stream's 16 MiB.

export const requestLimit = 655360;
export const inputQueueLimit = 2 * 1024 * 1024;
/** Largest file content, in bytes, a browser saves with one `file_write`. */
export const fileContentLimit = 2 * 1024 * 1024;
/** Largest `file_write` request, in bytes of JSON. */
export const fileWriteLimit = 6 * fileContentLimit + 64 * 1024;

const isFileWrite = (message: any) => message?.Stream?.type === "file_write";

/** Largest JSON payload allowed for this message. */
export function payloadLimit(message: unknown): number {
  return isFileWrite(message) ? fileWriteLimit : requestLimit;
}

/** Largest SSH input queue (in bytes, including this frame) allowed when
 * queuing this message. A file write may queue on top of a full ordinary
 * input queue; every other message keeps the 2 MiB cap. */
export function queueLimit(message: unknown): number {
  return isFileWrite(message)
    ? inputQueueLimit + fileWriteLimit + 4
    : inputQueueLimit;
}
