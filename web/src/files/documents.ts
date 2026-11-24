// The files open in editors: their state (KelpCore files/model.ts), loading,
// saving with the hash each was read at, conflicts, and following renames
// and removals made in the file browser. The editors themselves register a
// handle while mounted, so a document can be shown in any host view.
import { useCallback, useRef, useState } from "react";
import type { FilesClient } from "@kelpcore/web/files/api.ts";
import type { EditorHandle } from "@kelpcore/web/files/editor.tsx";
import {
  canSave,
  loadFailed,
  loaded,
  newDoc,
  saveFailed,
  saveProblem,
  saved,
  type Doc,
} from "@kelpcore/web/files/model.ts";
import { isInside } from "@kelpcore/web/files/paths.ts";
import { fileContentLimit } from "@kelpcore/web/request-limits.ts";
import { friendlyError } from "./root.ts";

/** An open file: its state and a stable identity across renames. */
export type OpenDoc = Doc & { id: number };
let serial = 0;

export function useDocuments(client: FilesClient | null) {
  const [docs, setDocs] = useState<OpenDoc[]>([]);
  const docsRef = useRef(docs);
  docsRef.current = docs;
  const editors = useRef(new Map<string, EditorHandle>());
  /** The largest content this browser saves: the bridge's request bound,
   * or the server's smaller write limit. */
  const saveLimit = Math.min(
    fileContentLimit,
    client?.limits.max_write_bytes ?? fileContentLimit,
  );
  const update = (path: string, change: (doc: Doc) => Doc) =>
    setDocs((all) =>
      all.map((doc) =>
        doc.path === path ? { ...doc, ...change(doc), id: doc.id } : doc,
      ),
    );
  const get = (path: string) => docsRef.current.find((d) => d.path === path);

  const load = useCallback(
    async (path: string) => {
      if (!client) return;
      update(path, (doc) => ({ ...doc, status: "loading", error: "" }));
      try {
        const content = await client.read(path);
        update(path, (doc) => loaded(doc, content, saveLimit));
      } catch (error) {
        update(path, (doc) => loadFailed(doc, friendlyError(error)));
      }
    },
    [client, saveLimit],
  );
  /** Opens a file (or shows it again when it is already open); returns
   * its document's identity. */
  const open = useCallback(
    (path: string) => {
      const existing = get(path);
      if (existing) return existing.id;
      const id = ++serial;
      setDocs((all) => [...all, { ...newDoc(path), id }]);
      void load(path);
      return id;
    },
    [load],
  );
  /** Saves the editor's buffer; `overwrite` replaces the file whatever it
   * holds now (the conflict's Overwrite). */
  const save = useCallback(
    async (path: string, overwrite = false) => {
      const doc = get(path);
      const editor = editors.current.get(path);
      if (!client || !doc || !editor || !canSave(doc)) return;
      if (doc.status === "conflict" && !overwrite) return;
      const content = editor.content();
      const problem = saveProblem(content, saveLimit);
      if (problem) return update(path, (d) => ({ ...d, error: problem }));
      update(path, (d) => ({ ...d, status: "saving", error: "" }));
      try {
        const result = await client.write(path, content, {
          baseHash: overwrite ? undefined : doc.hash || undefined,
        });
        update(path, (d) =>
          saved(d, content, result, editors.current.get(path)?.content() ?? content),
        );
      } catch (error) {
        update(path, (d) => saveFailed(d, friendlyError(error)));
      }
    },
    [client, saveLimit],
  );
  const close = (path: string) => {
    editors.current.delete(path);
    setDocs((all) => all.filter((doc) => doc.path !== path));
  };
  /** Keeps editing after a conflict without saving (the next save
   * conflicts again until the file is reloaded or overwritten). */
  const dismissConflict = (path: string) =>
    update(path, (doc) =>
      doc.status === "conflict" ? { ...doc, status: "ready" } : doc,
    );
  const setDirty = (path: string, dirty: boolean) =>
    update(path, (doc) => (doc.dirty === dirty ? doc : { ...doc, dirty }));
  const register = (path: string, handle: EditorHandle | null) => {
    if (handle) editors.current.set(path, handle);
    else editors.current.delete(path);
  };
  /** An entry moved or was renamed in the browser: open files inside it
   * follow it. */
  const renamed = (from: string, to: string) =>
    setDocs((all) =>
      all.map((doc) => {
        if (!isInside(doc.path, from)) return doc;
        const path = to + doc.path.slice(from.length);
        const editor = editors.current.get(doc.path);
        if (editor) {
          editors.current.delete(doc.path);
          editors.current.set(path, editor);
        }
        return { ...doc, path };
      }),
    );
  /** Entries were trashed or deleted: unchanged files inside them close;
   * changed ones stay open with the reason. */
  const removed = (paths: string[]) =>
    setDocs((all) =>
      all.flatMap((doc) => {
        if (!paths.some((path) => isInside(doc.path, path))) return [doc];
        if (!doc.dirty) {
          editors.current.delete(doc.path);
          return [];
        }
        return [
          {
            ...doc,
            error: "This file was removed; saving creates nothing.",
          },
        ];
      }),
    );
  return {
    docs,
    /** The files client (downloads start from the editor too). */
    client,
    saveLimit,
    open,
    load,
    save,
    close,
    dismissConflict,
    setDirty,
    register,
    renamed,
    removed,
  };
}
export type Documents = ReturnType<typeof useDocuments>;
