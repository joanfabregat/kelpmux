// An open file in the main area, like a terminal view: its header (name,
// folder, dirty marker, Save, Reload, Show in folder, Close) above
// KelpCore's CodeMirror editor, or an information view for binary, too
// large or unreadable files. A save that finds the file changed on disk
// offers Reload (discarding the edits), Overwrite or Cancel. CodeMirror is
// loaded with the first file opened.
import React, { Suspense, lazy, useRef } from "react";
import { formatBytes } from "@kelpcore/web/uploads.ts";
import { canSave, languageFor } from "@kelpcore/web/files/model.ts";
import {
  baseName,
  displayPath,
  parentPath,
} from "@kelpcore/web/files/paths.ts";
import { Icon } from "../status";
import type { Documents, OpenDoc } from "./documents";

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
}) {
  // Editors call back with the path they were created with; the document's
  // current path (after a rename) is the one that counts.
  const path = useRef(doc.path);
  path.current = doc.path;
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
      className="files-view files-editor"
      aria-label={`Editor: ${name}`}
      data-path={doc.path}
      onClick={(event) => event.stopPropagation()}
    >
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
            aria-labelledby="files-conflict-title"
          >
            <h2 id="files-conflict-title">{name} changed on disk</h2>
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
