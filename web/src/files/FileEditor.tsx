// An open file in the main area, like a terminal view: its header (name,
// folder, dirty marker, Save, Reload, Download, Show in folder, Close) above
// KelpCore's CodeMirror editor, or an information view for binary, too
// large or unreadable files. A save that finds the file changed on disk
// offers Reload (discarding the edits), Overwrite or Cancel. CodeMirror is
// loaded with the first file opened. The same view is the side panel
// beside a terminal (`side`), and either place can move the file to the
// other (`onMove`).
import React, { Suspense, lazy, useEffect, useRef } from "react";
import { formatBytes } from "@kelpcore/web/uploads.ts";
import { canSave, languageFor } from "@kelpcore/web/files/model.ts";
import {
  baseName,
  displayPath,
  parentPath,
} from "@kelpcore/web/files/paths.ts";
import { Icon } from "../status";
import type { Documents, OpenDoc } from "./documents";
import { startDownload } from "./downloads";
import { useToast } from "../Toast";

const TextEditor = lazy(() =>
  import("@kelpcore/web/files/editor.tsx").then((m) => ({
    default: m.TextEditor,
  })),
);

export function FileEditor({
  doc,
  documents,
  home,
  maxRead,
  onClose,
  onReveal,
  onBack,
  side = false,
  hidden = false,
  onMove,
  jump,
  tabs,
}: {
  doc: OpenDoc;
  documents: Documents;
  home?: string;
  /** The server's largest read, for the too-large view. */
  maxRead: number;
  /** Closes the file (asking first when it has unsaved changes). */
  onClose: () => void;
  /** Shows the file's folder in the browser. */
  onReveal: () => void;
  /** The phone's back button to the terminal list. */
  onBack?: () => void;
  /** Shown in the side panel beside a terminal, not the main area. */
  side?: boolean;
  /** Mounted but not shown (another view or file is): the buffer and
   * its unsaved edits stay. */
  hidden?: boolean;
  /** Moves the file to the other place: beside the terminal from the main
   * area, to the main area from the side panel. Only a file without
   * unsaved changes moves (the buffer lives in the mounted editor). */
  onMove?: () => void;
  /** A position to show and put the cursor at (1-based); a new `seq`
   * jumps again. */
  jump?: { line: number; col?: number; seq: number };
  /** The Files view's tab strip, above the header (the main area only). */
  tabs?: React.ReactNode;
}) {
  // Editors call back with the path they were created with; the document's
  // current path (after a rename) is the one that counts.
  const path = useRef(doc.path);
  path.current = doc.path;
  const toast = useToast();
  const section = useRef<HTMLElement>(null);
  const jumped = useRef(0);
  const ready = !!doc.version && doc.status !== "binary" && doc.status !== "too_large";
  useEffect(() => {
    if (!jump || !ready || jumped.current === jump.seq) return;
    let alive = true,
      tries = 0;
    // The editor mounts once CodeMirror has loaded: wait for its view.
    void import("@codemirror/view").then(({ EditorView }) => {
      const go = () => {
        if (!alive) return;
        const dom = section.current?.querySelector<HTMLElement>(".cm-editor");
        const view = dom && EditorView.findFromDOM(dom);
        if (!view) {
          if (tries++ < 300) requestAnimationFrame(go);
          return;
        }
        jumped.current = jump.seq;
        const text = view.state.doc;
        const line = text.line(Math.max(1, Math.min(text.lines, jump.line)));
        const at =
          line.from + Math.max(0, Math.min(line.length, (jump.col || 1) - 1));
        view.dispatch({
          selection: { anchor: at },
          effects: EditorView.scrollIntoView(at, { y: "center" }),
        });
        view.focus();
      };
      go();
    });
    return () => {
      alive = false;
    };
  }, [jump, ready, doc.id]);
  const name = baseName(doc.path);
  const folder = displayPath(parentPath(doc.path), home);
  const saving = doc.status === "saving";
  let body: React.ReactNode;
  if (doc.status === "loading" && !doc.version)
    body = <p className="files-message">Opening {name}…</p>;
  else if (doc.status === "binary")
    body = (
      <div className="empty-state files-info">
        <Icon name="file" size="lg" />
        <h3>Binary file</h3>
        <p>
          {name} ({formatBytes(doc.size)}) is not text, so it cannot be edited
          here.
        </p>
      </div>
    );
  else if (doc.status === "too_large")
    body = (
      <div className="empty-state files-info">
        <Icon name="file" size="lg" />
        <h3>Too large to open</h3>
        <p>
          {name} is {formatBytes(doc.size)}; the editor opens files up to{" "}
          {formatBytes(maxRead)}.
        </p>
      </div>
    );
  else if (doc.status === "error" && !doc.version)
    body = (
      <div className="empty-state files-info">
        <Icon name="circle-alert" size="lg" />
        <h3>Cannot open {name}</h3>
        <p>{doc.error}</p>
        <button
          type="button"
          className="subtle"
          onClick={() => void documents.load(doc.path)}
        >
          Try again
        </button>
      </div>
    );
  else
    body = (
      <Suspense fallback={<p className="files-message">Loading the editor…</p>}>
        <TextEditor
          key={doc.id}
          path={doc.path}
          saved={doc.saved}
          version={doc.version}
          language={languageFor(doc.path)}
          active
          label={`Contents of ${name}`}
          readOnly={!!doc.readOnly}
          onDirty={(_, dirty) => documents.setDirty(path.current, dirty)}
          onSave={() => void documents.save(path.current)}
          register={(_, handle) => documents.register(path.current, handle)}
        />
      </Suspense>
    );
  return (
    <section
      ref={section}
      hidden={hidden}
      className={`files-view files-editor ${side ? "files-side" : ""}`}
      aria-label={`${side ? "Side panel" : "Editor"}: ${name}`}
      data-path={doc.path}
      onClick={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        // Ctrl/Cmd+S anywhere in the view (the editor handles its own).
        if (
          (event.ctrlKey || event.metaKey) &&
          !event.altKey &&
          event.key.toLowerCase() === "s" &&
          !event.defaultPrevented
        ) {
          event.preventDefault();
          if (doc.dirty && canSave(doc) && !saving)
            void documents.save(doc.path);
        }
      }}
    >
      {tabs}
      <header className="files-head">
        {onBack && (
          <button
            type="button"
            className="ghost phone-back"
            aria-label="Terminals"
            onClick={onBack}
          >
            <Icon name="chevron-left" />
          </button>
        )}
        <Icon name="file" size="sm" className="faint" />
        <span className="files-title" title={doc.path}>
          <strong>{name}</strong>
          {doc.dirty && (
            <span
              className="files-dirty"
              role="img"
              aria-label="Unsaved changes"
              title="Unsaved changes"
            />
          )}
          <small className="files-folder">{folder}</small>
        </span>
        {doc.readOnly && <span className="badge">read-only</span>}
        <div className="files-tools">
          <button
            type="button"
            className="small"
            title="Save (Ctrl/Cmd+S)"
            disabled={!doc.dirty || !canSave(doc) || saving}
            onClick={() => void documents.save(doc.path)}
          >
            <Icon name="save" size="sm" />
            {saving ? "Saving…" : "Save"}
          </button>
          <button
            type="button"
            className="icon-btn"
            title="Reload from disk"
            aria-label="Reload from disk"
            disabled={saving}
            onClick={() => void documents.load(doc.path)}
          >
            <Icon name="refresh-cw" size="sm" />
          </button>
          {onMove && (
            <button
              type="button"
              className="icon-btn"
              title={
                doc.dirty
                  ? "Save first to move the file"
                  : side
                    ? "Open in the main area"
                    : "Move beside terminal"
              }
              aria-label={side ? "Open in the main area" : "Move beside terminal"}
              disabled={doc.dirty}
              onClick={onMove}
            >
              <Icon name={side ? "maximize-2" : "panel-right"} size="sm" />
            </button>
          )}
          {documents.client?.canDownload && (
            <button
              type="button"
              className="icon-btn"
              title={
                doc.dirty
                  ? "Download the saved file (not your unsaved changes)"
                  : "Download"
              }
              aria-label={`Download ${name}`}
              onClick={() =>
                startDownload(documents.client!, doc.path, false, (message, options) =>
                  toast(message, { key: "files", ...options }),
                )
              }
            >
              <Icon name="download" size="sm" />
            </button>
          )}
          <button
            type="button"
            className="icon-btn"
            title="Show in folder"
            aria-label="Show in folder"
            onClick={onReveal}
          >
            <Icon name="folder-open" size="sm" />
          </button>
          <button
            type="button"
            className="icon-btn"
            title="Close"
            aria-label={`Close ${name}`}
            onClick={onClose}
          >
            <Icon name="x" size="sm" />
          </button>
        </div>
      </header>
      {(doc.error && doc.version > 0) || doc.readOnly ? (
        <div
          className={`notice compact ${doc.error ? "error" : ""}`}
          role={doc.error ? "alert" : "status"}
        >
          {doc.error || doc.readOnly}
        </div>
      ) : null}
      <div className="files-editor-body">{body}</div>
      {doc.status === "conflict" && (
        <div className="modal-shade confirmation">
          <section
            className="confirm-dialog files-conflict"
            role="alertdialog"
            aria-labelledby={`files-conflict-title-${doc.id}`}
          >
            <h2 id={`files-conflict-title-${doc.id}`}>{name} changed on disk</h2>
            <p>
              The file changed since it was opened here. Reload it (your
              unsaved changes are lost), overwrite it with your version, or
              cancel and keep editing.
            </p>
            <div className="confirm-buttons">
              <button
                className="subtle"
                onClick={() => documents.dismissConflict(doc.path)}
              >
                Cancel
              </button>
              <button
                className="subtle"
                onClick={() => void documents.load(doc.path)}
              >
                Reload
              </button>
              <button
                className="danger"
                autoFocus
                onClick={() => void documents.save(doc.path, true)}
              >
                Overwrite
              </button>
            </div>
          </section>
        </div>
      )}
    </section>
  );
}
