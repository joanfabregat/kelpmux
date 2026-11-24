// The Files view's tab strip, its topmost bar, above the browser's
// breadcrumbs or the shown file's header: the file browser's tab first
// (never closed), then a tab per file open in the main area, in opening
// order. A tab shows its file (the
// first one the browser, at its folder); its × or a middle-click closes
// it, asking first when it has unsaved changes. Same-name files carry the
// folders that tell them apart (tabs.ts). The strip scrolls sideways and
// keeps the shown tab in view.
import React, { useEffect, useRef } from "react";
import { Icon } from "../status";
import { tabLabels } from "./tabs";

export type FileTabsProps = {
  /** The files open in the main area, in tab order. */
  docs: { path: string; dirty: boolean }[];
  /** The file shown; "" when the browser is. */
  current: string;
  onBrowse: () => void;
  onShow: (path: string) => void;
  onClose: (path: string) => void;
};

export function FileTabs({
  docs,
  current,
  onBrowse,
  onShow,
  onClose,
}: FileTabsProps) {
  const strip = useRef<HTMLDivElement>(null);
  const labels = tabLabels(docs.map((doc) => doc.path));
  // Keeps the shown tab in view, scrolling the strip only (never the page).
  useEffect(() => {
    const element = strip.current;
    const tab = element?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (!element || !tab) return;
    const box = element.getBoundingClientRect();
    const at = tab.getBoundingClientRect();
    if (at.left < box.left) element.scrollLeft -= box.left - at.left;
    else if (at.right > box.right) element.scrollLeft += at.right - box.right;
  }, [current, docs.length]);
  return (
    <div
      ref={strip}
      className="file-tabs"
      role="tablist"
      aria-label="Open files"
      onWheel={(event) => {
        // A vertical wheel scrolls the strip sideways.
        if (!event.deltaY || event.deltaX) return;
        event.currentTarget.scrollLeft += event.deltaY;
      }}
    >
      <div
        className={`file-tab browser-tab ${current ? "" : "current"}`}
        data-tab=""
      >
        <button
          type="button"
          role="tab"
          className="file-tab-main"
          aria-selected={!current}
          title="File browser"
          onClick={onBrowse}
        >
          <Icon name="folder" size="sm" />
          <span className="file-tab-name">Files</span>
        </button>
      </div>
      {docs.map((doc) => {
        const label = labels.get(doc.path)!;
        const on = doc.path === current;
        return (
          <div
            key={doc.path}
            className={`file-tab ${on ? "current" : ""} ${doc.dirty ? "dirty" : ""}`}
            data-tab={doc.path}
            onMouseDown={(event) => {
              if (event.button === 1) event.preventDefault();
            }}
            onAuxClick={(event) => {
              if (event.button !== 1) return;
              event.preventDefault();
              onClose(doc.path);
            }}
          >
            <button
              type="button"
              role="tab"
              className="file-tab-main"
              aria-selected={on}
              aria-label={`${label.name}${label.hint ? ` (${label.hint})` : ""}${doc.dirty ? ", unsaved changes" : ""}`}
              title={doc.path}
              onClick={() => onShow(doc.path)}
            >
              <Icon name="file" size="sm" />
              <span className="file-tab-name">{label.name}</span>
              {label.hint && (
                <small className="file-tab-hint">{label.hint}</small>
              )}
            </button>
            {doc.dirty && (
              <span
                className="files-dirty"
                role="img"
                aria-label="Unsaved changes"
              />
            )}
            <button
              type="button"
              className="icon-btn file-tab-close"
              aria-label={`Close tab ${label.name}`}
              title="Close (middle-click)"
              onClick={() => onClose(doc.path)}
            >
              <Icon name="x" size="sm" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
