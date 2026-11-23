// File paths in terminal output, found where the user Cmd/Ctrl-clicks
// rather than by scanning the screen: the token under the pointer, its
// `:line:col` suffix, and its resolution against the terminal's folder.
// Pure functions, no DOM or React, so `node --test` loads them directly.
// Nothing here is KelpMux-specific; it could move to @kelpcore/web.

/** A span of a line, as string indices (`end` exclusive). */
export type Span = { start: number; end: number; text: string };
/** A path with an optional position in the file (1-based). */
export type Location = { path: string; line?: number; col?: number };
/** A link candidate: the span to underline and where it points. */
export type PathLink = Location & Span;

const space = /\s/;
const quotes = "'\"`";
/** Characters stripped from the start of a token. */
const leading = "'\"`([{<";
/** Characters stripped from the end of a token (sentence punctuation,
 * closing brackets and quotes, and the colon of `file.rs:12:3: error`). */
const trailing = ".,:;)]}>'\"`";

/**
 * The token at `index` of `line`: the text between quotes when `index` is
 * inside a quoted span (which may hold spaces), otherwise the run of
 * non-space characters around it, without leading brackets or quotes and
 * trailing punctuation. Null on a space or on stripped punctuation.
 */
export function tokenAt(line: string, index: number): Span | null {
  if (index < 0 || index >= line.length || space.test(line[index])) return null;
  const quoted = quotedSpan(line, index);
  if (quoted) return quoted;
  let start = index,
    end = index + 1;
  while (start > 0 && !space.test(line[start - 1])) start--;
  while (end < line.length && !space.test(line[end])) end++;
  while (start < end && leading.includes(line[start])) start++;
  while (end > start && trailing.includes(line[end - 1])) end--;
  if (index < start || index >= end) return null;
  return { start, end, text: line.slice(start, end) };
}

const word = /[\p{L}\p{N}_]/u;
/** The inside of the quoted span holding `index`, if any. Quotes pair from
 * the start of the line; a quote after a letter (`don't`) opens nothing,
 * and one before a letter closes nothing. */
function quotedSpan(line: string, index: number): Span | null {
  for (let i = 0; i < line.length; i++) {
    if (!quotes.includes(line[i]) || (i > 0 && word.test(line[i - 1])))
      continue;
    let close = line.indexOf(line[i], i + 1);
    while (close >= 0 && close + 1 < line.length && word.test(line[close + 1]))
      close = line.indexOf(line[i], close + 1);
    if (close < 0) continue;
    if (index > i && index < close) {
      let start = i + 1,
        end = close;
      // `'src/x.py:3:'` keeps its location without the colon.
      while (end > start && ".,:;".includes(line[end - 1])) end--;
      if (index >= end || start === end) return null;
      return { start, end, text: line.slice(start, end) };
    }
    if (index <= close && index >= i) return null;
    i = close;
  }
  return null;
}

/**
 * Splits `path:line` and `path:line:col` (positive numbers) from a token.
 * Null for URLs (`scheme://…`), text with control characters, and tokens
 * of only dots, slashes and tildes (`/`, `..`, `~`), which are rarely
 * meant as links.
 */
export function parseLocation(token: string): Location | null {
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(token)) return null;
  if (/[\u0000-\u001f\u007f-\u009f]/.test(token)) return null;
  let path = token,
    line: number | undefined,
    col: number | undefined;
  const match = /^(.+?):(\d+)(?::(\d+))?$/.exec(token);
  if (match) {
    path = match[1];
    line = Number(match[2]) || undefined;
    col = match[3] && line ? Number(match[3]) || undefined : undefined;
  }
  if (!path || /^[./~]+$/.test(path)) return null;
  return line ? (col ? { path, line, col } : { path, line }) : { path };
}

/** `path` without `.` and `..` components or repeated slashes; `..` never
 * climbs above the root. `path` must be absolute. */
export function normalizePath(path: string): string {
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return `/${parts.join("/")}`;
}

/**
 * The absolute path `path` names for a terminal in the folder `cwd`:
 * absolute paths as they are, `~` and `~/…` inside `home`, anything else
 * relative to `cwd`. Null when the folder needed is unknown (`cwd` or
 * `home` missing or not absolute) or for `~user` forms.
 */
export function resolvePath(
  path: string,
  cwd: string | undefined,
  home: string | undefined,
): string | null {
  if (path.startsWith("/")) return normalizePath(path);
  if (path === "~" || path.startsWith("~/")) {
    if (!home?.startsWith("/")) return null;
    return normalizePath(`${home}/${path.slice(1)}`);
  }
  if (path.startsWith("~")) return null;
  if (!cwd?.startsWith("/")) return null;
  return normalizePath(`${cwd}/${path}`);
}

/** The path of a `file://` URL (an OSC 8 hyperlink), or null. */
export function fileUrlPath(uri: string): string | null {
  if (!/^file:\/\//i.test(uri)) return null;
  try {
    const url = new URL(uri);
    return url.pathname ? decodeURIComponent(url.pathname) : null;
  } catch {
    return null;
  }
}

/** The path link at `index` of `line`, resolved for a terminal in `cwd`:
 * the token, its location and its absolute path, or null. */
export function pathLinkAt(
  line: string,
  index: number,
  cwd: string | undefined,
  home: string | undefined,
): PathLink | null {
  const token = tokenAt(line, index);
  if (!token) return null;
  const location = parseLocation(token.text);
  if (!location) return null;
  const path = resolvePath(location.path, cwd, home);
  return path ? { ...token, ...location, path } : null;
}
