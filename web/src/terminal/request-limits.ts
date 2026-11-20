// Size limits for bridge requests sent over the SSH channel. They mirror the
// `kelpmux web-bridge` limit: every request is capped at 640 KiB of JSON, and
// the SSH input queue holds at most 2 MiB.

export const requestLimit = 655360;
export const inputQueueLimit = 2 * 1024 * 1024;

/** Largest JSON payload allowed for this message. */
export function payloadLimit(_message: unknown): number {
  return requestLimit;
}

/** Largest SSH input queue (in bytes, including this frame) allowed when
 * queuing this message. */
export function queueLimit(_message: unknown): number {
  return inputQueueLimit;
}
