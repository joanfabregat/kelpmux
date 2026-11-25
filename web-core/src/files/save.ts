// Saving a download (`FilesClient.download`, KelpServer `file-downloads`)
// on the user's computer. Where the File System Access API exists
// (Chromium's `showSaveFilePicker`), the user picks the file first and the
// bytes stream to it as they arrive: nothing is held in memory and the disk
// paces the server. Elsewhere the bytes are collected into a Blob, up to
// `BLOB_LIMIT`, and saved through an `<a download>` link once complete; a
// larger download is refused with `too_large` (a single file before its
// first byte, an archive when it grows past the limit).
//
// `saveDownload` must be called from the user's click (or key press): the
// picker needs that transient activation, so it is the first thing it does.
import {
  FileError,
  type DownloadArchive,
  type DownloadProgress,
  type DownloadResult,
  type FilesClient,
} from "./api.ts";
import { baseName, parentPath } from "./paths.ts";

/** Largest download collected in memory when the browser cannot stream to
 * a file (1 GiB). */
export const BLOB_LIMIT = 1024 * 1024 * 1024;

/** The file name a download is offered under: a file's own name, or
 * `<folder>.tar.gz`, or for several entries `<their folder>.tar.gz`. The
 * server suggests the same names. */
export function downloadName(
  paths: readonly string[],
  archive?: DownloadArchive,
): string {
  if (!archive) return baseName(paths[0] || "") || "download";
  if (paths.length === 1)
    return `${baseName(paths[0]) || "download"}.tar.gz`;
  const parent = parentPath(paths[0]);
  const shared = paths.every((path) => parentPath(path) === parent);
  return `${(shared && baseName(parent)) || "download"}.tar.gz`;
}

export type SaveOptions = {
  archive?: DownloadArchive;
  onProgress?: (progress: DownloadProgress) => void;
  signal?: AbortSignal;
  /** `false` never uses the save picker (always a Blob). */
  picker?: boolean;
};
export type SavedDownload = DownloadResult & {
  /** `file`: streamed to the file the user picked; `blob`: saved through
   * the browser's downloads. */
  saved: "file" | "blob";
};

function tooLarge(size?: number) {
  return new FileError({
    code: "too_large",
    message: `This browser can only save downloads up to ${BLOB_LIMIT / 1024 ** 3} GiB; use a Chromium-based browser to save larger ones directly to disk.`,
    size,
  });
}

/** Downloads `paths` and saves the result; rejects with code `cancelled`
 * when the user closes the save picker or aborts `signal`. */
export async function saveDownload(
  client: FilesClient,
  paths: string[],
  options: SaveOptions = {},
): Promise<SavedDownload> {
  const { archive, onProgress, signal } = options;
  const picker = (globalThis as any).showSaveFilePicker;
  if (typeof picker === "function" && options.picker !== false) {
    let handle: any;
    try {
      handle = await picker({ suggestedName: downloadName(paths, archive) });
    } catch (error: any) {
      if (error?.name === "AbortError")
        throw new FileError({
          code: "cancelled",
          message: "The download was cancelled",
        });
      // Not allowed here (no user activation, a sandboxed frame): fall
      // back to the browser's downloads.
      handle = undefined;
    }
    if (handle) {
      const writable = await handle.createWritable();
      try {
        const result = await client.download(paths, {
          archive,
          onProgress,
          signal,
          sink: { write: (bytes) => writable.write(bytes) },
        });
        await writable.close();
        return { ...result, saved: "file" };
      } catch (error) {
        // Discards what was written (Chromium writes to a temporary file
        // and replaces the target only on close).
        await writable.abort().catch(() => {});
        throw error;
      }
    }
  }
  const parts: Uint8Array[] = [];
  let total = 0;
  const result = await client.download(paths, {
    archive,
    onProgress,
    signal,
    sink: {
      start(start) {
        if (start.size !== undefined && start.size > BLOB_LIMIT)
          throw tooLarge(start.size);
      },
      write(bytes) {
        total += bytes.length;
        if (total > BLOB_LIMIT) throw tooLarge();
        parts.push(bytes);
      },
    },
  });
  saveBlob(
    new Blob(parts as BlobPart[], {
      type: result.archive ? "application/gzip" : "application/octet-stream",
    }),
    result.name,
  );
  return { ...result, saved: "blob" };
}

/** Hands a Blob to the browser's downloads under `name`. */
export function saveBlob(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.rel = "noopener";
  link.style.display = "none";
  document.body.append(link);
  link.click();
  link.remove();
  // The browser has read the URL once the download starts.
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}
