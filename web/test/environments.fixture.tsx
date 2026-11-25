// A project's Environments view (test/environments.mjs), as main.tsx wires
// it, against a fake bridge, without any network or SSH endpoint: the real
// Sidebar grouped by project, with the environment tag on a terminal that
// runs one, the project menu with Environments (not for Scratch), the real
// EnvironmentsView kept fresh by useProjectEnvironments, and the URL kept
// by useHistorySync. The bridge is `window.bridge(method, params)`, which
// the test exposes. A terminal opened from the view is recorded in
// `window.opened`. `?mode=light` and `?phone=1` change the theme and the
// layout.
import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import type { NumberedPane } from "@kelpcore/web/types.ts";
import { Sidebar } from "../src/Sidebar";
import { FileMenu } from "../src/files/actions";
import { EnvironmentsView, useProjectEnvironments } from "../src/Environments";
import { readEnvironmentPanes, type EnvironmentPane } from "../src/environments";
import { ToastProvider } from "../src/Toast";
import {
  SCRATCH,
  groupOrder,
  projectOf,
  readLinks,
  readProjects,
  scratchProject,
  sidebarProjects,
  type Project,
} from "../src/projects";
import {
  dialogFromParam,
  locationOf,
  useHistorySync,
  type AppLocation,
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
const phone = query.has("phone");
const mode = query.get("mode") === "light" ? "light" : "dark";
const root = document.documentElement;
root.dataset.mode = mode;
for (const [key, value] of Object.entries({
  ...themeVariables("black", mode),
  ...interfaceVariables("black", mode),
}))
  root.style.setProperty(key, value);

const w = window as any;
w.toasts = [];
w.opened = [];
const bridge = (method: string, params: any = {}): Promise<any> =>
  w.bridge(method, params);
const panes: NumberedPane[] = [
  [11, "claude", "/home/me/work/alpha"],
  [14, "env web (process)", "/home/me/work/alpha/app"],
  [13, "shell", "/home/me/scratch"],
].map(([id, title, cwd], index) => ({
  id,
  number: index + 1,
  title,
  cwd,
  status: "idle",
})) as any;

type View = "grid" | "envs";

function App() {
  const [ready, setReady] = useState(false);
  const [projects, setProjects] = useState<Project[]>([]);
  const [links, setLinks] = useState<Record<number, string>>({});
  const [envPanes, setEnvPanes] = useState<Record<number, EnvironmentPane>>({});
  const [view, setView] = useState<View>("grid");
  const [scope, setScope] = useState("");
  const [dialog, setDialog] = useState("");
  const [menu, setMenu] = useState<{ project: Project; x: number; y: number } | null>(null);
  const scratch = scratchProject("~/scratch", "/home/me/scratch");
  const all = sidebarProjects(projects, scratch);
  const byId = (id: string) => all.find((p) => p.id === id);
  const scopeProject = scope ? byId(scope) : undefined;
  const shown: View = view === "envs" && (!scopeProject || scope === SCRATCH) ? "grid" : view;
  const ordered = groupOrder(
    panes,
    all.map((p) => p.id),
    (pane) => projectOf(pane, links, projects),
  );
  const loadPanes = async () =>
    setEnvPanes(readEnvironmentPanes(await bridge("environment_panes")));
  useEffect(() => {
    void (async () => {
      setProjects(readProjects((await bridge("config"))?.projects));
      setLinks(readLinks(await bridge("projects")));
      await loadPanes();
      setReady(true);
    })();
  }, []);
  // As main.tsx.
  const envs = useProjectEnvironments({
    enabled: ready,
    project: shown === "envs" ? scope : "",
    query: bridge,
    onStarted: () => void loadPanes(),
  });
  const here = locationOf({
    phoneHome: false,
    view: shown,
    terminal: 11,
    filesDir: "~",
    file: "",
    side: "",
    dialog,
    notifyPane: 0,
    project: scope,
  });
  const navigation = useHistorySync({
    here: ready ? here : null,
    ready,
    context: phone ? "phone" : "wide",
    apply(target: AppLocation) {
      setMenu(null);
      setDialog(dialogFromParam(target.dialog));
      let project = target.project;
      if (project && !all.some((p) => p.id === project)) {
        w.toasts.push("This project no longer exists");
        navigation.automatic();
        project = "";
      }
      setScope(project);
      if (target.view === "envs") {
        if (!project || project === SCRATCH) {
          navigation.automatic();
          setScope("");
          return setView("grid");
        }
        return setView("envs");
      }
      setView("grid");
    },
  });
  const projectMenuItems = (project: Project): [string, () => void, boolean?][] =>
    project.scratch
      ? [["Terminals", () => setScope(project.id)]]
      : [
          [
            "Environments",
            () => {
              setScope(project.id);
              setView("envs");
            },
            project.missing,
          ],
        ];
  (window as any).fixtureReady = ready;
  if (!ready) return <p>Signing in…</p>;
  return (
    <div
      className={`kelpmux-shell ${phone ? "phone touch" : ""}`}
      style={{ "--rail": "264px" } as React.CSSProperties}
    >
      {!phone && (
        <div className="rail-wrap">
          <Sidebar
            username="alice"
            dev={false}
            folded={false}
            mode={shown}
            terminals={ordered}
            parked={[]}
            notifications={[]}
            vitals={{}}
            diskMounts={[]}
            onDisksHover={() => {}}
            onMode={() => {
              setScope("");
              setView("grid");
            }}
            rowProps={() => ({})}
            rowClass={() => ""}
            onOpen={() => {}}
            onClose={() => {}}
            onPark={() => {}}
            onUnpark={() => {}}
            onMenu={() => {}}
            onNew={() => {}}
            projects={all}
            projectOf={(pane) => projectOf(pane, links, projects)}
            onNewIn={() => {}}
            onProjectMenu={(project, x, y) => setMenu({ project, x, y })}
            environmentOf={(pane) => envPanes[pane]}
            onState={() => {}}
            onDialog={setDialog}
            onResizeStart={() => {}}
            onToggleFold={() => {}}
          />
        </div>
      )}
      <div className={`app ${shown}`}>
        <div className="workspace">
          <div className="workspace-main">
            {shown === "envs" && scopeProject ? (
              <EnvironmentsView
                key={scopeProject.id}
                project={scopeProject}
                environments={envs.environments}
                loading={envs.loading}
                error={envs.error}
                pending={envs.pending}
                onRefresh={envs.refresh}
                onStart={envs.start}
                onStop={envs.stop}
                onCheck={envs.check}
                onOpenPane={(pane) => w.opened.push(pane)}
                onBack={phone ? () => setView("grid") : undefined}
              />
            ) : (
              <div id="grid">
                {all.map((project) => (
                  <button
                    key={project.id}
                    data-open-envs={project.id}
                    onClick={() => {
                      setScope(project.id);
                      setView("envs");
                    }}
                  >
                    {project.name} environments
                  </button>
                ))}
              </div>
            )}
          </div>
        </div>
      </div>
      {menu && (
        <FileMenu
          label={`Project ${menu.project.name}`}
          x={menu.x}
          y={menu.y}
          items={projectMenuItems(menu.project)}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <ToastProvider>
    <App />
  </ToastProvider>,
);
