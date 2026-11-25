// A project's home page (test/project-home.mjs) as main.tsx wires it,
// against a fake bridge and without any network or SSH endpoint: the real
// Sidebar grouped by project (a group's name opens the page, its chevron
// folds it), the project menu with Overview, the real ProjectHome with its
// Git facts kept fresh by useProjectGit, the real ReposView (opened with a
// checkout expanded), the real HistoryPanel as History's dialog, and a
// stand-in for the grid and a terminal, with the URL kept by
// useHistorySync. The bridge is `window.bridge(method, params)`, which the
// test exposes; reopened terminals are recorded in `window.restored` and
// copied text in `window.copied`. `?mode=light` and `?phone=1` change the
// theme and the layout.
import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import type { NumberedPane } from "@kelpcore/web/types.ts";
import { Sidebar } from "../src/Sidebar";
import { ProjectHome } from "../src/ProjectHome";
import { ReposView } from "../src/Repos";
import { HistoryPanel } from "../src/Panels";
import { FileMenu } from "../src/files/actions";
import { useProjectGit } from "../src/git-status";
import { Icon } from "../src/status";
import { ToastProvider } from "../src/Toast";
import { titleLocation } from "../src/title-location";
import {
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
  emptyReposPlace,
  locationOf,
  useHistorySync,
  type AppLocation,
  type ReposPlace,
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
if (phone) root.dataset.layout = "phone";
for (const [key, value] of Object.entries({
  ...themeVariables("black", mode),
  ...interfaceVariables("black", mode),
}))
  root.style.setProperty(key, value);

const w = window as any;
w.toasts = [];
w.restored = [];
const bridge = (method: string, params: any = {}): Promise<any> =>
  w.bridge(method, params);
const now = Math.floor(Date.now() / 1000);
const panes: NumberedPane[] = (
  [
    [11, "fix the login flow", "/home/me/work/alpha/api", "claude", "input_needed", 120],
    [12, "shell", "/home/me/work/alpha", null, "idle", 3 * 3600],
    [13, "review the Repos view", "/home/me/work/alpha/web", "codex", "working", 5],
    [14, "vim", "/home/me/work/beta", null, "idle", 600],
    [15, "shell", "/home/me/scratch", null, "idle", 60],
    [16, "release notes", "/home/me/work/alpha/docs", "claude", "turn_finished", 900],
  ] as const
).map(([id, title, cwd, kind, status, ago], index) => ({
  id,
  number: index + 1,
  title,
  cwd,
  kind,
  status,
  pid: 100 + id,
  last_activity: now - ago,
})) as any;

type View = "single" | "grid" | "repos" | "project";

function App() {
  const [ready, setReady] = useState(false);
  const [projects, setProjects] = useState<Project[]>([]);
  const [links, setLinks] = useState<Record<number, string>>({});
  const [view, setView] = useState<View>("grid");
  const [scope, setScope] = useState("");
  const [active, setActive] = useState(11);
  const [dialog, setDialog] = useState("");
  const [place, setPlace] = useState<ReposPlace>(emptyReposPlace);
  const [phoneList, setPhoneList] = useState(phone);
  const [parked] = useState(() => new Set([16]));
  const [menu, setMenu] = useState<{ project: Project; x: number; y: number } | null>(null);
  const scratch = scratchProject("~/scratch", "/home/me/scratch");
  const all = sidebarProjects(projects, scratch);
  const byId = (id: string) => all.find((p) => p.id === id);
  const scopeProject = scope ? byId(scope) : undefined;
  const shown: View =
    (view === "repos" || view === "project") && !scopeProject
      ? "grid"
      : phone && view === "grid"
        ? "single"
        : view;
  const ordered = groupOrder(
    panes,
    all.map((p) => p.id),
    (pane) => projectOf(pane, links, projects),
  );
  useEffect(() => {
    void (async () => {
      setProjects(readProjects((await bridge("config"))?.projects));
      setLinks(readLinks(await bridge("projects")));
      setReady(true);
    })();
  }, []);
  // As main.tsx.
  const git = useProjectGit({
    enabled: ready,
    projects: all.map((p) => p.id),
    shown: shown === "repos" || shown === "project" ? scope : "",
    query: bridge,
  });
  const listShown = phone && (phoneList || shown === "grid");
  const here = locationOf({
    phoneHome: listShown,
    view: shown,
    terminal: active,
    filesDir: "",
    file: "",
    side: "",
    dialog,
    notifyPane: 0,
    project: scope,
    repos: place,
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
      if (target.view === "repos" || target.view === "project") {
        if (!project) {
          if (!target.project) w.toasts.push("Choose a project");
          navigation.automatic();
          if (phone) setPhoneList(true);
          return setView("grid");
        }
        setPhoneList(false);
        if (target.view === "repos")
          setPlace({ repo: target.repo, tab: target.tab, rev: target.rev, commit: target.commit, change: target.change });
        return setView(target.view);
      }
      if (target.view === "home" || target.view === "grid") {
        if (phone) setPhoneList(true);
        return setView("grid");
      }
      setPhoneList(false);
      if (target.terminal) setActive(target.terminal);
      setView("single");
    },
  });
  const openHome = (id: string) => {
    setMenu(null);
    setScope(id);
    setPhoneList(false);
    setView("project");
  };
  const openRepos = (id: string, repo = "", tab = "") => {
    setScope(id);
    setPlace(repo ? { ...emptyReposPlace, repo, tab } : emptyReposPlace);
    setPhoneList(false);
    setView("repos");
  };
  const openPane = (pane: NumberedPane) => {
    setActive(pane.id);
    setPhoneList(false);
    setView("single");
  };
  const reopened = (pane: number, existing: boolean) => {
    w.restored.push([pane, existing]);
    setDialog("");
    setActive(pane);
    setPhoneList(false);
    setView("single");
  };
  const projectMenuItems = (project: Project): [string, () => void, boolean?][] => [
    ...(shown === "project" && scope === project.id
      ? []
      : ([["Overview", () => openHome(project.id)]] as [string, () => void][])),
    ["Repos", () => openRepos(project.id)],
    [
      "History",
      () => {
        setScope(project.id);
        setDialog("recovery");
      },
    ],
  ];
  (window as any).fixtureReady = ready;
  if (!ready) return <p>Signing in…</p>;
  const sidebar = (
    <Sidebar
      username="alice"
      dev={false}
      folded={false}
      phone={phone}
      touch={phone}
      mode={shown}
      terminals={ordered.filter((p) => !parked.has(p.id))}
      parked={ordered.filter((p) => parked.has(p.id))}
      notifications={[]}
      current={shown === "single" ? active : undefined}
      vitals={{}}
      diskMounts={[]}
      onDisksHover={() => {}}
      onMode={(next) => {
        setScope("");
        setView(next as View);
      }}
      rowProps={() => ({})}
      rowClass={() => ""}
      onOpen={openPane}
      onClose={() => {}}
      onPark={() => {}}
      onUnpark={openPane}
      onMenu={() => {}}
      onNew={() => {}}
      projects={all}
      projectOf={(pane) => projectOf(pane, links, projects)}
      onNewIn={() => {}}
      onProjectMenu={(project, x, y) => setMenu({ project, x, y })}
      gitOf={(project) => git.byProject[project.id]}
      onProjectHome={(project) => openHome(project.id)}
      homeProject={shown === "project" ? scope : undefined}
      onState={() => {}}
      onDialog={setDialog}
      onResizeStart={() => {}}
      onToggleFold={() => {}}
    />
  );
  return (
    <div
      className={`kelpmux-shell ${phone ? "phone touch" : ""} ${listShown ? "phone-home" : ""}`}
      style={{ "--rail": "264px" } as React.CSSProperties}
    >
      {!phone && <div className="rail-wrap">{sidebar}</div>}
      <div className={`app ${shown}`}>
        <p id="title" hidden>
          {shown === "project" && scopeProject
            ? titleLocation({ view: "project", project: scopeProject.name })
            : ""}
        </p>
        <div className="workspace">
          <div className="workspace-main">
            {listShown ? (
              <div className="phone-list">{sidebar}</div>
            ) : shown === "project" && scopeProject ? (
              <ProjectHome
                key={scopeProject.id}
                project={scopeProject}
                projects={projects}
                panes={ordered.filter(
                  (p) => projectOf(p.id, links, projects) === scopeProject.id,
                )}
                parked={parked}
                notifications={[]}
                home="/home/me"
                git={git.byProject[scopeProject.id]}
                gitOffered
                gitError={git.errors[scopeProject.id]}
                conversations
                rpc={bridge}
                historyKey=""
                onOpenPane={openPane}
                onNewTerminal={() =>
                  void bridge("new", scopeProject.scratch ? {} : { project: scopeProject.id })
                }
                onOpenFiles={() => w.toasts.push(`files ${scopeProject.path}`)}
                onOpenEnvironments={
                  scopeProject.scratch
                    ? undefined
                    : () => w.toasts.push(`envs ${scopeProject.id}`)
                }
                onMenu={(x, y) => setMenu({ project: scopeProject, x, y })}
                onCopy={(text) => (w.copied = text)}
                onSeeTerminals={() => {
                  if (phone) setPhoneList(true);
                  else setView("grid");
                }}
                onSeeRepos={(repo, tab) => openRepos(scopeProject.id, repo, tab)}
                onSeeHistory={() => setDialog("recovery")}
                onRestored={reopened}
                onBack={phone ? () => setPhoneList(true) : undefined}
              />
            ) : shown === "repos" && scopeProject ? (
              <ReposView
                key={scopeProject.id}
                place={place}
                onPlace={setPlace}
                project={scopeProject}
                git={git.byProject[scopeProject.id]}
                loading={!!git.loading[scopeProject.id]}
                error={git.errors[scopeProject.id]}
                onRefresh={() => git.refresh(scopeProject.id)}
              />
            ) : shown === "single" ? (
              <div id="terminal" data-pane={active}>
                Terminal {active}
              </div>
            ) : (
              <div id="grid" data-scope={scope}>
                Grid {scope}
              </div>
            )}
          </div>
        </div>
      </div>
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
              rpc={bridge}
              conversations
              scope={scopeProject}
              projects={projects}
              onRestored={reopened}
              onCopy={(text) => (w.copied = text)}
            />
          </section>
        </div>
      )}
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
