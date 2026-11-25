// Projects as main.tsx wires them (test/projects.mjs), against a fake bridge
// and a fake file server, without any network or SSH endpoint: the real
// Sidebar grouped by project with the row drag of main.tsx's dragProps (a
// row dropped on another project's group moves the terminal there), the
// project menu, the Move to project dialog, the project dialog, a project's
// grid (tiles are plain boxes), its files (the real FilesBrowser over a fake
// FilesClient transport) and its History (the real HistoryPanel),
// with the URL kept by useHistorySync. The bridge is `window.bridge(method,
// params)`, which the test exposes: its state outlives a reload. Terminals
// have pane ids 11 to 14; the file root is /home/me. `?mode=light` and
// `?phone=1` change the theme and the layout.
import React, { useEffect, useRef, useState } from "react";
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
import { HistoryPanel } from "../src/Panels";
import { MoveToProjectDialog, ProjectDialog } from "../src/ProjectDialog";
import { ToastProvider } from "../src/Toast";
import {
  groupOrder,
  projectColor,
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
const bridge = (method: string, params: any = {}): Promise<any> =>
  w.bridge(method, params);
const home = "/home/me";
const panes: NumberedPane[] = [
  [11, "claude", "/home/me/work/alpha"],
  [12, "vim", "/home/me/work/beta"],
  [13, "shell", "/home/me/scratch"],
  [14, "htop", "/home/me"],
].map(([id, title, cwd], index) => ({
  id,
  number: index + 1,
  title,
  cwd,
  status: "idle",
})) as any;

// ───── A fake file server (file_list and file_stat only) ─────
const tree: Record<string, string[]> = {
  "/home/me": ["notes/", "scratch/", "work/", "todo.txt"],
  "/home/me/notes": ["ideas.md"],
  "/home/me/scratch": [],
  "/home/me/work": ["alpha/", "beta/", "gamma/"],
  "/home/me/work/alpha": ["src/", "README.md"],
  "/home/me/work/alpha/src": ["main.rs"],
  "/home/me/work/beta": ["Cargo.toml"],
  "/home/me/work/gamma": ["lib/"],
  "/home/me/work/gamma/lib": [],
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
          new CustomEvent("file", { detail: { request: params.request, ...detail } }),
        ),
      );
    if (type === "file_list" && tree[path])
      return reply({ type: "file_list", path, entries: tree[path].map(entry) });
    if (type === "file_stat" && (tree[path] || path === home))
      return reply({ type: "file_stat", path, entry: entry(`${path.split("/").pop()}/`) });
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

type View = "single" | "grid" | "files";

function App() {
  const [ready, setReady] = useState(false);
  const [projects, setProjects] = useState<Project[]>([]);
  const [scratchFolder, setScratchFolder] = useState("/home/me/scratch");
  const [links, setLinks] = useState<Record<number, string>>({});
  const [view, setView] = useState<View>("grid");
  const [active, setActive] = useState(11);
  const [scope, setScope] = useState("");
  const [dir, setDir] = useState("~");
  const [dialog, setDialog] = useState("");
  const [menu, setMenu] = useState<{ project: Project; x: number; y: number } | null>(null);
  const [moving, setMoving] = useState<NumberedPane | null>(null);
  const [create, setCreate] = useState<{ name: string; folder: string } | null>(null);
  const [drag, setDrag] = useState<number | null>(null);
  const [dropProject, setDropProject] = useState("");
  const scratch = scratchProject("~/scratch", scratchFolder);
  const all = sidebarProjects(projects, scratch);
  const byId = (id: string) => all.find((p) => p.id === id);
  const scopeProject = scope ? byId(scope) : undefined;
  const ofPane = (pane: number) =>
    drag === pane && dropProject ? dropProject : projectOf(pane, links, projects);
  const ordered = groupOrder(
    panes,
    all.map((p) => p.id),
    ofPane,
  );
  function applyConfig(config: any) {
    setProjects(readProjects(config?.projects));
    if (config?.resolved?.scratch?.folder)
      setScratchFolder(config.resolved.scratch.folder);
  }
  async function load() {
    applyConfig(await bridge("config"));
    setLinks(readLinks(await bridge("projects")));
  }
  // As main.tsx: the configuration and the links are asked again every
  // few seconds (here 300 ms).
  useEffect(() => {
    void load().then(() => setReady(true));
    const timer = setInterval(load, 300);
    return () => clearInterval(timer);
  }, []);
  const here = locationOf({
    phoneHome: false,
    view,
    terminal: active,
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
      if (target.view === "grid" || target.view === "home") return setView("grid");
      if (target.view === "terminal") {
        setView("single");
        if (target.terminal) setActive(target.terminal);
        return;
      }
      setView("files");
      if (target.dir) setDir(target.dir);
    },
  });
  async function move(pane: number, project: Project) {
    const result = await bridge("project_move", {
      pane,
      project: project.scratch ? null : project.id,
    });
    setLinks(readLinks(result));
    w.toasts.push(`Moved to ${project.name}`);
  }
  // As main.tsx's dragProps, the project part: a row dragged over another
  // project's group moves there when dropped.
  const rowProps = (p: NumberedPane) => ({
    onPointerDown: (e: React.PointerEvent) => {
      if (e.button !== 0) return;
      const state = { startX: e.clientX, startY: e.clientY, moving: false, project: "" };
      const moveHandler = (event: PointerEvent) => {
        if (!state.moving) {
          if (Math.hypot(event.clientX - state.startX, event.clientY - state.startY) < 5)
            return;
          state.moving = true;
          setDrag(p.id);
        }
        const group = document
          .elementFromPoint(event.clientX, event.clientY)
          ?.closest<HTMLElement>("[data-project-group]")?.dataset.projectGroup;
        if (group && group !== state.project) {
          state.project = group;
          setDropProject(group);
        }
      };
      const up = () => {
        window.removeEventListener("pointermove", moveHandler);
        window.removeEventListener("pointerup", up);
        const target = state.project && byId(state.project);
        if (state.moving && target) {
          setLinks((old) => {
            const next = { ...old };
            if (target.scratch) delete next[p.id];
            else next[p.id] = target.id;
            return next;
          });
          void move(p.id, target);
        }
        setDrag(null);
        setDropProject("");
      };
      window.addEventListener("pointermove", moveHandler);
      window.addEventListener("pointerup", up);
    },
  });
  const projectMenuItems = (project: Project): [string, () => void, boolean?][] => [
    [
      "Files",
      () => {
        setScope(project.id);
        setDir(project.path);
        setView("files");
      },
    ],
    [
      "Terminals",
      () => {
        setScope(project.id);
        setView("grid");
      },
    ],
    [
      "History",
      () => {
        setScope(project.id);
        setDialog("recovery");
      },
    ],
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
    onCreateProject: (path) =>
      setCreate({ name: path.split("/").pop() || "", folder: path }),
    isProjectFolder: (path) => all.some((p) => p.path === path),
    upload: { unavailable: "no uploads here", limits: null, transport: null as any },
  };
  const resolve = (folder: string) =>
    folder.startsWith("/")
      ? folder
      : folder === "~"
        ? home
        : folder.startsWith("~/")
          ? `${home}/${folder.slice(2)}`
          : null;
  (window as any).fixtureReady = ready;
  if (!ready) return <p>Signing in…</p>;
  const inScope = (p: NumberedPane) => !scope || ofPane(p.id) === scope;
  const color = projectColor(scopeProject);
  const scopeBar = (what: string, back: string) =>
    scopeProject && (
      <div
        className="project-scope"
        data-scope={scopeProject.id}
        style={color ? ({ "--project": color } as React.CSSProperties) : undefined}
      >
        <span className={`project-dot ${color ? "" : "plain"}`} aria-hidden="true" />
        <span className="grow">
          <b>{scopeProject.name}</b> · {what}
        </span>
        <button type="button" className="ghost small" onClick={() => setScope("")}>
          {back}
        </button>
      </div>
    );
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
            mode={view}
            terminals={ordered}
            parked={[]}
            notifications={[]}
            current={view === "single" ? active : undefined}
            vitals={{}}
            diskMounts={[]}
            onDisksHover={() => {}}
            onMode={(next) => {
              setScope("");
              setView(next);
            }}
            rowProps={rowProps}
            rowClass={(p) => (drag === p.id ? "dragging" : "")}
            onOpen={(p) => {
              setActive(p.id);
              setView("single");
            }}
            onClose={() => {}}
            onPark={() => {}}
            onUnpark={() => {}}
            onMenu={(p) => setMoving(p)}
            onNew={() => {}}
            projects={all}
            projectOf={ofPane}
            dropProject={dropProject}
            onNewIn={() => {}}
            onProjectMenu={(project, x, y) => setMenu({ project, x, y })}
            onNewProject={() => setCreate({ name: "", folder: "" })}
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
      <div className="app single">
        <div className="workspace">
          <div className="workspace-main">
            {view === "files" ? (
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
              />
            ) : view === "grid" ? (
              <>
                {scopeBar("Terminals", "All terminals")}
                <div className="fixture-grid">
                  {ordered.filter(inScope).map((p) => (
                    <div key={p.id} className="fixture-tile" data-tile={p.id}>
                      {p.title}
                    </div>
                  ))}
                </div>
              </>
            ) : (
              <p id="single">Terminal {active}</p>
            )}
          </div>
        </div>
      </div>
      {dialog === "recovery" && (
        <div className="modal-shade" data-dialog="recovery">
          <section className="dialog panel">
            <HistoryPanel
              rpc={bridge}
              scope={scopeProject}
              projects={projects}
              onAllProjects={() => setScope("")}
              onRestored={() => {}}
              onCopy={() => {}}
            />
            <button id="close-dialog" onClick={() => setDialog("")}>
              Close
            </button>
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
      {moving && (
        <MoveToProjectDialog
          title={moving.title}
          projects={all}
          current={projectOf(moving.id, links, projects)}
          onMove={(project) => {
            setMoving(null);
            void move(moving.id, project);
          }}
          onCancel={() => setMoving(null)}
        />
      )}
      {create && (
        <ProjectDialog
          mode="create"
          initial={create}
          base="~/work"
          projects={all}
          resolve={resolve}
          onSave={async (draft) => {
            const result = await bridge("project_create", draft);
            applyConfig(result.config);
            if (result.saved) {
              setCreate(null);
              return { saved: true, problems: {} };
            }
            const problems: Record<string, string[]> = {};
            for (const error of result.errors) {
              const field = /\.(\w+)$/.exec(error.key)?.[1] || "";
              (problems[field] ||= []).push(error.message);
            }
            return { saved: false, problems };
          }}
          onCancel={() => setCreate(null)}
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
