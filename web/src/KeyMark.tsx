import type { Identity } from "./terminal/types";
export function KeyMark({ identity }: { identity: Pick<Identity, "emoji"> }) {
  return (
    <span className="key-glyph" aria-hidden="true">
      {identity.emoji || (
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <circle cx="8" cy="8" r="4.5" />
          <path d="m11.3 11.3 8.2 8.2h2v-3h-3v-3h-3" />
        </svg>
      )}
    </span>
  );
}
