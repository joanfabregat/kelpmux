/** Types shared by the reusable terminal component. Nothing here may import
 * application code; KelpHub can consume this directory at a pinned revision. */
/** A plain shell's latest foreground command (KelpServer facts). */
export type ShellCommand = {
  started_at: number;
  ended_at?: number | null;
  alternate_screen?: boolean;
};
export type Color =
  "Default" | { Indexed: number } | { Rgb: [number, number, number] };
export type Cell = {
  text: string;
  width: number;
  fg: Color;
  bg: Color;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  hyperlink?: string;
};
export type Screen = {
  cols: number;
  rows: number;
  cells: Cell[];
  cursor?: { visible: boolean; position: { x: number; y: number } };
  modes: Record<string, boolean | number>;
};
export type Pane = {
  id: number;
  pid: number;
  cwd: string;
  title: string;
  status: string;
  dead: boolean;
  kind?: string;
  session_id?: string;
  observation?: {
    source: string;
    checked_at?: number;
    event_at?: number | null;
  };
  /** When the current status began (seconds), for finished-turn identity. */
  status_since?: number;
  /** A plain shell's latest foreground command (KelpServer facts). */
  command?: ShellCommand | null;
  /** Last terminal output (seconds; 0 when unknown). */
  last_activity?: number;
};
/** A pane as a client shows it: KelpServer's facts plus the number the
 * client gives it, its position in the client's own order of terminals
 * (from 1). KelpServer 0.3.0-beta.2 sends neither a number nor parking:
 * both are client state. */
export type NumberedPane = Pane & { number: number };
export type Snapshot = { pane: number; revision: number; screen: Screen };
export type SearchMatch = {
  source: "primary" | "alternate";
  first_line: number;
  lines: string[];
  start: { x: number; y: number };
  end: { x: number; y: number };
};
export type Connection = {
  origin: string;
  rpId: string;
  hostPublic: string;
  username: string;
  command: string;
  workspace: string;
  dev: boolean;
  /** Present only when the relay configured error reporting (errors.ts). */
  errors?: { dsn: string; environment?: string };
  /** Present only when the relay sets them; see web/src/idle.ts. */
  idle_warning_minutes?: number;
  idle_lock_minutes?: number;
};
export type Identity = {
  version: number;
  rpId: string;
  origin: string;
  credentialId: string;
  point: string;
  fingerprint?: string;
  label?: string;
  emoji?: string;
  color?: string;
};
/** A key offered to SSH. After discovery, `candidates` holds every recovered
 * public point (base64) for the credential; `point` is the first of them. */
export type SignInKey = Identity & { candidates?: string[] };
/** Server-side display profile for a key, matched by SSH fingerprint. */
export type KeyProfile = {
  fingerprint: string;
  label?: string;
  emoji?: string;
  color?: string;
  updated?: number;
};
