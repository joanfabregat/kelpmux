// Pure state of an open file in an editor: loading, the saved baseline and
// its hash, dirty state, saving and conflicts. No DOM, React or CodeMirror
// imports, so `node --test` loads this module directly. Derived from
// KelpHub's workspace editor model (web/src/files/model.ts).
import { FileError, type FileContent, type FileWritten } from "./api.ts";

export type Language =
  | "javascript"
  | "jsx"
  | "typescript"
  | "tsx"
  | "json"
  | "markdown"
  | "css"
  | "html"
  | "python"
  | "rust"
  | "yaml"
  | "shell"
  | "toml"
  | "dockerfile"
  | "text";

const byExtension: Record<string, Language> = {
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "jsx",
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "tsx",
  json: "json",
  jsonc: "json",
  webmanifest: "json",
  md: "markdown",
  markdown: "markdown",
  css: "css",
  html: "html",
  htm: "html",
  svelte: "html",
  vue: "html",
  py: "python",
  pyi: "python",
  rs: "rust",
  yml: "yaml",
  yaml: "yaml",
  sh: "shell",
  bash: "shell",
  zsh: "shell",
  toml: "toml",
  dockerfile: "dockerfile",
  containerfile: "dockerfile",
};
const byName: Record<string, Language> = {
  dockerfile: "dockerfile",
  containerfile: "dockerfile",
  ".bashrc": "shell",
  ".bash_profile": "shell",
  ".zshrc": "shell",
  ".zshenv": "shell",
  ".profile": "shell",
  "cargo.lock": "toml",
  pipfile: "toml",
};

/** The syntax mode for a path, chosen by file name or extension. */
export function languageFor(path: string): Language {
  const name = (path.split("/").pop() || "").toLowerCase();
  if (Object.hasOwn(byName, name)) return byName[name];
  // Dockerfile.dev, Containerfile.prod
  const stem = name.split(".")[0];
  if (stem === "dockerfile" || stem === "containerfile") return "dockerfile";
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return "text";
  const extension = name.slice(dot + 1);
  return Object.hasOwn(byExtension, extension)
    ? byExtension[extension]
    : "text";
}

export type DocStatus =
  | "loading"
  | "ready"
  | "saving"
  | "conflict"
  | "binary"
  | "too_large"
  | "error";
export type Doc = {
  path: string;
  status: DocStatus;
  /** Hash of the disk version the buffer is based on; null before a read. */
  hash: string | null;
  /** Content at the last read or save (the clean baseline). */
  saved: string;
  /** Incremented whenever the editor must replace its buffer with `saved`. */
  version: number;
  dirty: boolean;
  /** The last failure to show, or "". */
  error: string;
  /** Size on disk at the last read or save, in bytes. */
  size: number;
  /** Modification time at the last read or save (Unix seconds). */
  modified: number;
  /** Why the file opens read-only (too large to save from here), or "". */
  readOnly: string;
};

export const newDoc = (path: string): Doc => ({
  path,
  status: "loading",
  hash: null,
  saved: "",
  version: 0,
  dirty: false,
  error: "",
  size: 0,
  modified: 0,
  readOnly: "",
});

/** UTF-8 length of `text`, without encoding when it is clearly small. */
function utf8Length(text: string, limit: number): number {
  return text.length * 3 <= limit
    ? text.length
    : new TextEncoder().encode(text).length;
}
/** Why `content` cannot be saved through a limit of `limit` bytes, or "". */
export function saveProblem(content: string, limit: number): string {
  return utf8Length(content, limit) > limit
    ? `This file is larger than ${formatLimit(limit)} and cannot be saved from the editor.`
    : "";
}
function formatLimit(bytes: number): string {
  return bytes % 2 ** 20 === 0
    ? `${bytes / 2 ** 20} MiB`
    : bytes % 1024 === 0
      ? `${bytes / 1024} KiB`
      : `${bytes} bytes`;
}

/** Apply a read: the buffer is replaced and becomes clean. A file larger
 * than `saveLimit` opens read-only. */
export function loaded(doc: Doc, content: FileContent, saveLimit: number): Doc {
  const base = {
    ...doc,
    hash: content.hash,
    dirty: false,
    error: "",
    size: content.size,
    modified: content.modified,
  };
  if (content.binary || content.text == null)
    return { ...base, status: "binary", saved: "", readOnly: "" };
  return {
    ...base,
    status: "ready",
    saved: content.text,
    version: doc.version + 1,
    readOnly: saveProblem(content.text, saveLimit),
  };
}
/** Apply a failed read: a file over the read limit gets its own view. */
export function loadFailed(doc: Doc, error: unknown): Doc {
  if (error instanceof FileError && error.code === "too_large")
    return {
      ...doc,
      status: "too_large",
      size: error.size ?? doc.size,
      error: "",
    };
  return { ...doc, status: "error", error: errorText(error) };
}

/** Whether a document can be saved now. */
export const canSave = (doc: Doc) =>
  !doc.readOnly && (doc.status === "ready" || doc.status === "conflict");

/** Apply a successful write of `content`. The document stays dirty when the
 * buffer changed again while the write was in flight. */
export function saved(
  doc: Doc,
  content: string,
  result: Pick<FileWritten, "hash" | "size" | "modified">,
  current: string,
): Doc {
  return {
    ...doc,
    status: "ready",
    hash: result.hash,
    saved: content,
    dirty: current !== content,
    error: "",
    size: result.size,
    modified: result.modified,
  };
}

/** Apply a failed write: a conflict opens the conflict choice (reload,
 * overwrite or cancel); anything else keeps the edits and reports it. */
export function saveFailed(doc: Doc, error: unknown): Doc {
  return error instanceof FileError && error.code === "conflict"
    ? { ...doc, status: "conflict", error: "" }
    : { ...doc, status: "ready", error: errorText(error) };
}

export const errorText = (error: unknown) =>
  error instanceof Error ? error.message : String(error);
