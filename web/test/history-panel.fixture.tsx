// History (test/history-panel.mjs) as main.tsx shows it, against a fake
// bridge and without any network or SSH endpoint: the real HistoryPanel in
// the application's dialog, scoped to a project by the URL (`dialog=history`
// or the former `dialog=closed`, with `project=`), which the fixture writes
// back in its current form. The bridge is `window.bridge(method, params)`,
// which the test exposes; restored terminals are recorded in
// `window.restored`. `?mode=light` changes the theme.
import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { HistoryPanel } from "../src/Panels";
import { Icon } from "../src/status";
import { scratchProject, type Project } from "../src/projects";
import {
  dialogFromParam,
  emptyLocation,
  locationSearch,
  parseLocation,
  resolveLocation,
} from "../src/navigation";
import { interfaceVariables, themeVariables } from "../src/appearance";
import "../src/styles/base.css";
import "../src/styles/shell.css";
import "@kelpcore/web/terminal.css";
import "../src/styles/dialogs.css";
import "../src/styles/kelpmux.css";
import "../src/styles/state.css";
import "../src/styles/mobile.css";

const query = new URLSearchParams(location.search);
const mode = query.get("mode") === "light" ? "light" : "dark";
const root = document.documentElement;
root.dataset.mode = mode;
if (query.has("phone")) root.dataset.layout = "phone";
for (const [key, value] of Object.entries({
  ...themeVariables("black", mode),
  ...interfaceVariables("black", mode),
}))
  root.style.setProperty(key, value);

const w = window as any;
w.restored = [];
const bridge = (method: string, params: any = {}): Promise<any> =>
  w.bridge(method, params);
const projects: Project[] = [
  {
    id: "alpha",
    name: "Alpha",
    folder: "~/work/alpha",
    path: "/home/me/work/alpha",
    color: "blue",
    missing: false,
  },
  {
    id: "beta",
    name: "Beta",
    folder: "~/work/beta",
    path: "/home/me/work/beta",
    color: "green",
    missing: false,
  },
];
const scratch = scratchProject("~/scratch", "/home/me/scratch");

// The place the page was opened at.
const wanted = parseLocation(location.search);

function App() {
  const [dialog, setDialog] = useState(dialogFromParam(wanted.dialog || ""));
  const [scope, setScope] = useState(wanted.project || "");
  // The URL in its current form, as useHistorySync writes it.
  useEffect(() => {
    const place = resolveLocation(
      { ...wanted, dialog: dialog ? "history" : "", project: scope },
      emptyLocation,
    );
    history.replaceState(null, "", locationSearch(place, location.search));
  }, [dialog, scope]);
  const scopeProject =
    scope === scratch.id ? scratch : projects.find((p) => p.id === scope);
  return (
    <>
      <nav className="fixture-nav">
        <button id="open-history" onClick={() => setDialog("recovery")}>
          History
        </button>
        {[...projects, scratch].map((project) => (
          <button
            key={project.id}
            id={`scope-${project.id}`}
            onClick={() => {
              setScope(project.id);
              setDialog("recovery");
            }}
          >
            {project.name}
          </button>
        ))}
      </nav>
      {dialog === "recovery" && (
        <div className="modal-shade">
          <section
            className="modal panel-modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="modal-title"
            data-dialog="recovery"
          >
            <header>
              <h2 id="modal-title">
                {scopeProject ? `History · ${scopeProject.name}` : "History"}
              </h2>
              <button
                className="icon-btn"
                aria-label="Close dialog"
                onClick={() => setDialog("")}
              >
                <Icon name="x" />
              </button>
            </header>
            <HistoryPanel
              key={scope}
              rpc={bridge}
              conversations={query.get("old") === null}
              scope={scopeProject}
              projects={projects}
              onAllProjects={() => setScope("")}
              onRestored={(pane, existing) => w.restored.push([pane, existing])}
              onCopy={(text) => (w.copied = text)}
            />
          </section>
        </div>
      )}
    </>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
w.fixtureReady = true;
