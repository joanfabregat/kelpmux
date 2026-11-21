// Agent provider marks (from KelpHub). These are simple monochrome shapes drawn
// for KelpHub, not the providers' brand assets: an eight-ray spark for Claude
// Code and a hexagon around a prompt chevron for Codex. Plain terminals have
// no mark.
import { providerLabel, providerOf } from "./model";
import "../styles/state.css";

const marks = {
  claude: (
    <path
      d="M8 1.5v13M1.5 8h13M3.4 3.4l9.2 9.2M12.6 3.4l-9.2 9.2"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.9"
      strokeLinecap="round"
    />
  ),
  codex: (
    <>
      <path
        d="M8 1.3l5.8 3.35v6.7L8 14.7l-5.8-3.35v-6.7z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinejoin="round"
      />
      <path
        d="M5.6 6l2.2 2-2.2 2M8.9 10.4h2"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </>
  ),
};

export function ProviderLogo({
  kind,
  decorative = false,
  className = "",
}: {
  /** Pane kind or run agent: "claude", "codex", anything else for none. */
  kind?: string | null;
  decorative?: boolean;
  className?: string;
}) {
  const provider = providerOf(kind);
  if (!provider) return null;
  const name = providerLabel[provider];
  return (
    <svg
      className={`provider-logo provider-${provider} ${className}`.trim()}
      viewBox="0 0 16 16"
      role={decorative ? undefined : "img"}
      aria-label={decorative ? undefined : name}
      aria-hidden={decorative ? true : undefined}
      focusable="false"
    >
      {decorative ? null : <title>{name}</title>}
      {marks[provider]}
    </svg>
  );
}
