// Project status, archive, restore and delete (test/archive.mjs) as
// main.tsx wires them, against a fake bridge and without any network or
// SSH endpoint: the real Sidebar (paused projects dimmed, archived ones in
// the Archived group), the project menu with Pause / Resume, Archive… and
// Delete… (as main.tsx builds it from statusActions), the real
// RemovalDialog, the real ProjectHome with the real ArchivedPanel for an
// archived project, and `?view=settings` for the real ArchiveSettings. The
// bridge is `window.bridge(method, params)`, which the test exposes; the
// configuration is re-read after each job and status change, as the
// bridge's `Config` notice would bring it. Toasts are recorded in
// `window.toasts`. `?mode=light` and `?phone=1` change the theme and the
// layout.
import React, { useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Sidebar } from "../src/Sidebar";
import { ProjectHome } from "../src/ProjectHome";
import { FileMenu } from "../src/files/actions";
import {
  ArchivedPanel,
  ArchiveSettings,
  RemovalDialog,
  useArchiveJobs,
} from "../src/Archive";
import { statusActions } from "../src/archive-state";
import { ToastProvider } from "../src/Toast";
import {
  readProjects,
  scratchProject,
  sidebarProjects,
  type Project,
} from "../src/projects";
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
const view = query.get("view") === "settings" ? "settings" : "app";
const mode = query.get("mode") === "light" ? "light" : "dark";
const root = document.documentElement;
root.dataset.mode = mode;
if (phone) root.dataset.layout = "phone";
for (const [key, value] of Object.entries({
  ...themeVariables("black", mode),
  ...interfaceVariables("black", mode),
}))
  root.style.setProperty(key, value);

const w = window as any;
w.toasts = [];
const bridge = (method: string, params: any = {}): Promise<any> =>
  w.bridge(method, params);

function App() {
  const [ready, setReady] = useState(false);
  const [projects, setProjects] = useState<Project[]>([]);
  const [scope, setScope] = useState(query.get("project") || "");
  const [menu, setMenu] = useState<{ project: Project; x: number; y: number } | null>(null);
  const [removal, setRemoval] = useState<{ mode: "archive" | "delete"; project: Project } | null>(null);
  const reload = useCallback(async () => {
    setProjects(readProjects((await bridge("config"))?.projects));
  }, []);
  const jobs = useArchiveJobs(bridge, (job) => {
    w.toasts.push(`${job.kind} ${job.state}`);
    void reload();
  });
  useEffect(() => {
    void reload().then(() => setReady(true));
  }, [reload]);
  w.fixtureReady = ready;
  const scratch = scratchProject("~/scratch", "/home/me/scratch");
  const all = sidebarProjects(projects, scratch);
  const shown = all.find((p) => p.id === scope);
  const menuItems = (project: Project): [string, () => void, boolean?][] => {
    const items: [string, () => void, boolean?][] = [];
    if (project.status === "archived") {
      items.push(["Restore…", () => setScope(project.id)]);
      return items;
    }
    for (const action of statusActions(project))
      if (action === "pause" || action === "resume")
        items.push([
          action === "pause" ? "Pause" : "Resume",
          () =>
            void bridge("project_status", {
              id: project.id,
              status: action === "pause" ? "paused" : "active",
            }).then(reload),
        ]);
      else if (action === "archive")
        items.push(["Archive…", () => setRemoval({ mode: "archive", project }), project.missing]);
      else if (action === "delete")
        items.push(["Delete…", () => setRemoval({ mode: "delete", project })]);
    return items;
  };
  if (!ready) return <p>Loading…</p>;
  if (view === "settings")
    return (
      <div className="modal-shade">
        <section className="modal panel-modal" role="dialog" aria-label="Settings">
          <header>
            <h2>Settings</h2>
          </header>
          <div style={{ padding: "0 20px 20px" }}>
            <ArchiveSettings rpc={bridge} />
          </div>
        </section>
      </div>
    );
  const sidebar = (
    <Sidebar
      username="alice"
      dev={false}
      folded={false}
      phone={phone}
      touch={phone}
      mode="grid"
      terminals={[]}
      parked={[]}
      notifications={[]}
      vitals={{}}
      diskMounts={[]}
      onDisksHover={() => {}}
      onMode={() => setScope("")}
      rowProps={() => ({})}
      rowClass={() => ""}
      onOpen={() => {}}
      onClose={() => {}}
      onPark={() => {}}
      onUnpark={() => {}}
      onMenu={() => {}}
      onNew={() => {}}
      projects={all}
      projectOf={() => "scratch"}
      onNewIn={() => {}}
      onProjectMenu={(project, x, y) => setMenu({ project, x, y })}
      onProjectHome={(project) => setScope(project.id)}
      homeProject={scope || undefined}
      onState={() => {}}
      onDialog={() => {}}
      onResizeStart={() => {}}
      onToggleFold={() => {}}
    />
  );
  return (
    <div
      className={`kelpmux-shell ${phone ? "phone touch" : ""}`}
      style={{ "--rail": "264px" } as React.CSSProperties}
    >
      {!phone && <div className="rail-wrap">{sidebar}</div>}
      <div className="app project">
        <div className="workspace">
          <div className="workspace-main">
            {shown ? (
              <ProjectHome
                key={shown.id}
                project={shown}
                projects={projects}
                panes={[]}
                parked={new Set()}
                notifications={[]}
                home="/home/me"
                gitOffered={false}
                conversations={false}
                rpc={bridge}
                historyKey=""
                onOpenPane={() => {}}
                onNewTerminal={() => {}}
                onMenu={(x, y) => setMenu({ project: shown, x, y })}
                onCopy={() => {}}
                onSeeTerminals={() => {}}
                onSeeRepos={() => {}}
                onSeeHistory={() => {}}
                onRestored={() => {}}
                onBack={phone ? () => setScope("") : undefined}
                archived={
                  shown.status === "archived" ? (
                    <ArchivedPanel
                      project={shown}
                      rpc={bridge}
                      jobs={jobs}
                      base="~/work"
                      taken={projects.map((p) => p.folder)}
                    />
                  ) : undefined
                }
              />
            ) : phone ? (
              <div className="phone-list">{sidebar}</div>
            ) : (
              <div id="grid">Grid</div>
            )}
          </div>
        </div>
      </div>
      {menu && (
        <FileMenu
          label={`Project ${menu.project.name}`}
          x={menu.x}
          y={menu.y}
          items={menuItems(menu.project)}
          onClose={() => setMenu(null)}
        />
      )}
      {removal && (
        <RemovalDialog
          mode={removal.mode}
          project={removal.project}
          rpc={bridge}
          jobs={jobs}
          onRemoveOnly={async () => {
            const result = await bridge("project_delete", {
              id: removal.project.id,
              confirmed: true,
            });
            await reload();
            return !!result?.saved;
          }}
          onClose={() => {
            setRemoval(null);
            void reload();
          }}
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
