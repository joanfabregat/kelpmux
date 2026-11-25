// A project's Repos view and the Git marks (test/repos.mjs), as main.tsx
// wires them, against a fake bridge and a fake file server, without any
// network or SSH endpoint: the real Sidebar grouped by project with each
// header's changes mark (GitMark), the project menu with Repos, the real
// ReposView kept fresh by useProjectGit, and the real FilesBrowser with its
// checkout lines, with the URL kept by useHistorySync. The bridge is
// `window.bridge(method, params)`, which the test exposes. Opened files and
// new terminals are recorded in `#opened` and the bridge. `?mode=light` and
// `?phone=1` change the theme and the layout.
import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import type { NumberedPane } from "@kelpcore/web/types.ts";
import {
  FilesClient,
  isFileEvent,
  readFileLimits,
  type FilesTransport,
} from "@kelpcore/web/files/api.ts";
import { Sidebar } from "../src/Sidebar";
import { FilesBrowser } from "../src/files/FilesBrowser";
import { FileMenu, type FileActionsProps } from "../src/files/actions";
import { ReposView } from "../src/Repos";
import { checkoutsByPath } from "../src/git";
import { useProjectGit } from "../src/git-status";
import { ToastProvider } from "../src/Toast";
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
import "../src/styles/files.css";

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
const home = "/home/me";
const panes: NumberedPane[] = [
  [11, "claude", "/home/me/work/alpha/api"],
  [12, "vim", "/home/me/work/beta"],
  [13, "shell", "/home/me/scratch"],
].map(([id, title, cwd], index) => ({
  id,
  number: index + 1,
  title,
  cwd,
  status: "idle",
})) as any;

// ───── A fake file server (file_list and file_stat only) ─────
const tree: Record<string, string[]> = {
  "/home/me": ["scratch/", "work/"],
  "/home/me/scratch": [],
  "/home/me/work": ["alpha/", "beta/"],
  "/home/me/work/alpha": ["api/", "docs/", "web/", "notes.md"],
  "/home/me/work/alpha/api": ["src/", "Cargo.toml"],
  "/home/me/work/alpha/api/src": ["main.rs"],
  "/home/me/work/alpha/docs": ["index.md"],
  "/home/me/work/alpha/web": ["package.json"],
  "/home/me/work/beta": ["README.md"],
};
const entry = (name: string) => ({
  name: name.replace(/\/$/, ""),
  kind: name.endsWith("/") ? "dir" : "file",
  size: 10,
  modified: 1_700_000_000,
  mode: name.endsWith("/") ? 0o40755 : 0o100644,
});
const fileEvents = new EventTarget();
const filesTransport: FilesTransport = {
  send(type: string, params: any) {
    const path = params.path === "~" ? home : params.path;
    const reply = (detail: any) =>
      setTimeout(() =>
        fileEvents.dispatchEvent(
          new CustomEvent("file", {
            detail: { request: params.request, ...detail },
          }),
        ),
      );
    if (type === "file_list" && tree[path])
      return reply({ type: "file_list", path, entries: tree[path].map(entry) });
    if (type === "file_stat" && (tree[path] || path === home))
      return reply({
        type: "file_stat",
        path,
        entry: entry(`${path.split("/").pop()}/`),
      });
    reply({ type: "file_error", code: "not_found", message: "Not found" });
  },
  listen(handler) {
    const listener = (event: Event) => {
      const detail = (event as CustomEvent).detail;
      if (isFileEvent(detail)) handler(detail);
    };
    fileEvents.addEventListener("file", listener);
    return () => fileEvents.removeEventListener("file", listener);
  },
};
const client = new FilesClient(filesTransport, {
  limits: readFileLimits({
    max_read_bytes: 1 << 20,
    max_write_bytes: 1 << 20,
    max_list_entries: 1000,
    max_paths: 100,
    max_in_flight: 8,
    max_path_bytes: 4096,
    max_depth: 64,
    root: home,
  })!,
});

type View = "single" | "grid" | "files" | "repos";

function App() {
  const [ready, setReady] = useState(false);
  const [projects, setProjects] = useState<Project[]>([]);
  const [links, setLinks] = useState<Record<number, string>>({});
  const [view, setView] = useState<View>("grid");
  const [scope, setScope] = useState("");
  const [dir, setDir] = useState("~");
  const [dialog, setDialog] = useState("");
  const [opened, setOpened] = useState("");
  const [menu, setMenu] = useState<{ project: Project; x: number; y: number } | null>(null);
  const scratch = scratchProject("~/scratch", "/home/me/scratch");
  const all = sidebarProjects(projects, scratch);
  const byId = (id: string) => all.find((p) => p.id === id);
  const scopeProject = scope ? byId(scope) : undefined;
  const shown: View = view === "repos" && !scopeProject ? "grid" : view;
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
    shown: shown === "repos" ? scope : "",
    query: bridge,
  });
  const checkouts = checkoutsByPath(git.byProject);
  const here = locationOf({
    phoneHome: false,
    view: shown,
    terminal: 11,
    filesDir: dir,
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
      if (target.view === "repos") {
        if (!project) {
          navigation.automatic();
          return setView("grid");
        }
        return setView("repos");
      }
      if (target.view === "grid" || target.view === "home") return setView("grid");
      if (target.view === "terminal") return setView("single");
      setView("files");
      if (target.dir) setDir(target.dir);
    },
  });
  const openRepos = (project: string) => {
    setScope(project);
    setView("repos");
  };
  const projectMenuItems = (project: Project): [string, () => void, boolean?][] => [
    [
      "Files",
      () => {
        setScope(project.id);
        setDir(project.path);
        setView("files");
      },
    ],
    ["Repos", () => openRepos(project.id), project.missing],
  ];
  const actions: FileActionsProps = {
    client,
    home,
    onOpenFile: () => {},
    onInsertPath: () => {},
    onCopyPath: () => {},
    confirm: () => {},
    toast: (message) => w.toasts.push(message),
    onRenamed: () => {},
    onRemoved: () => {},
    upload: { unavailable: "no uploads here", limits: null, transport: null as any },
  };
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
            onMode={(next) => {
              setScope("");
              setView(next);
            }}
            rowProps={() => ({})}
            rowClass={() => ""}
            onOpen={() => setView("single")}
            onClose={() => {}}
            onPark={() => {}}
            onUnpark={() => {}}
            onMenu={() => {}}
            onNew={() => {}}
            projects={all}
            projectOf={(pane) => projectOf(pane, links, projects)}
            onNewIn={() => {}}
            onProjectMenu={(project, x, y) => setMenu({ project, x, y })}
            gitOf={(project) => git.byProject[project.id]}
            onState={() => {}}
            onDialog={setDialog}
            onResizeStart={() => {}}
            onToggleFold={() => {}}
            onFiles={() => {
              setScope("");
              setView("files");
            }}
          />
        </div>
      )}
      <div className={`app ${shown}`}>
        <div className="workspace">
          <div className="workspace-main">
            {shown === "repos" && scopeProject ? (
              <ReposView
                key={scopeProject.id}
                project={scopeProject}
                git={git.byProject[scopeProject.id]}
                loading={!!git.loading[scopeProject.id]}
                error={git.errors[scopeProject.id]}
                onRefresh={() => git.refresh(scopeProject.id)}
                onOpenFile={(path) => {
                  w.opened.push(path);
                  setOpened(path);
                }}
                onShowInFiles={(path) => {
                  setScope(scopeProject.id);
                  setDir(path);
                  setView("files");
                }}
                onNewTerminal={(path) =>
                  void bridge("new", {
                    ...(scopeProject.scratch ? {} : { project: scopeProject.id }),
                    cwd: path,
                  })
                }
                onBack={phone ? () => setView("grid") : undefined}
              />
            ) : shown === "files" ? (
              <FilesBrowser
                {...actions}
                dir={dir}
                onDir={(folder) => setDir(folder)}
                phone={phone}
                projects={all}
                scope={scopeProject}
                onOpenProject={(project) => {
                  setScope(project.id);
                  setDir(project.path);
                }}
                onAllFiles={() => setScope("")}
                checkoutAt={(path) => checkouts.get(path)}
              />
            ) : (
              <div id="grid">
                {all.map((project) => (
                  <button
                    key={project.id}
                    data-open-repos={project.id}
                    onClick={() => openRepos(project.id)}
                  >
                    {project.name} repos
                  </button>
                ))}
              </div>
            )}
            <p id="opened" hidden>
              {opened}
            </p>
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
