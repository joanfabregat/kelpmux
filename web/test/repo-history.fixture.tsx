// A checkout's history and diffs in the Repos view (test/repo-history.mjs),
// as main.tsx wires them, against a fake bridge and without any network or
// SSH endpoint: the real ReposView kept fresh by useProjectGit, its
// CheckoutPanel (RepoHistory.tsx) asking the bridge's history operations,
// and the URL kept by useHistorySync. The bridge is `window.bridge(method,
// params)`, which the test exposes. `?mode=light`, `?phone=1` and
// `?nohistory=1` (a bridge before API minor 10) change the theme, the
// layout and the operations offered.
import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { ReposView } from "../src/Repos";
import { useProjectGit } from "../src/git-status";
import { ToastProvider } from "../src/Toast";
import { readProjects, type Project } from "../src/projects";
import {
  emptyReposPlace,
  locationOf,
  useHistorySync,
  type AppLocation,
  type ReposPlace,
} from "../src/navigation";
import { interfaceVariables, themeVariables } from "../src/appearance";
import "../src/styles/base.css";
import "../src/styles/shell.css";
import "../src/styles/dialogs.css";
import "../src/styles/kelpmux.css";
import "../src/styles/state.css";
import "../src/styles/mobile.css";

const query = new URLSearchParams(location.search);
const phone = query.has("phone");
const history = !query.has("nohistory");
const mode = query.get("mode") === "light" ? "light" : "dark";
const root = document.documentElement;
root.dataset.mode = mode;
for (const [key, value] of Object.entries({
  ...themeVariables("black", mode),
  ...interfaceVariables("black", mode),
}))
  root.style.setProperty(key, value);

const w = window as any;
w.opened = [];
const bridge = (method: string, params: any = {}): Promise<any> =>
  w.bridge(method, params);

function App() {
  const [ready, setReady] = useState(false);
  const [projects, setProjects] = useState<Project[]>([]);
  const [view, setView] = useState<"repos" | "grid">("grid");
  const [scope, setScope] = useState("");
  const [place, setPlace] = useState<ReposPlace>(emptyReposPlace);
  useEffect(() => {
    void (async () => {
      setProjects(readProjects((await bridge("config"))?.projects));
      setReady(true);
    })();
  }, []);
  const project = projects.find((p) => p.id === scope);
  const shown = view === "repos" && project ? "repos" : "grid";
  const git = useProjectGit({
    enabled: ready,
    projects: projects.map((p) => p.id),
    shown: shown === "repos" ? scope : "",
    query: bridge,
  });
  const here = locationOf({
    phoneHome: false,
    view: shown,
    terminal: 0,
    filesDir: "",
    file: "",
    side: "",
    dialog: "",
    notifyPane: 0,
    project: scope,
    repos: place,
  });
  useHistorySync({
    here: ready ? here : null,
    ready,
    context: phone ? "phone" : "wide",
    apply(target: AppLocation) {
      setScope(target.project);
      if (target.view === "repos") {
        setPlace({
          repo: target.repo,
          tab: target.tab,
          rev: target.rev,
          commit: target.commit,
          change: target.change,
        });
        setView("repos");
      } else setView("grid");
    },
  });
  w.fixtureReady = ready;
  if (!ready) return <p>Signing in…</p>;
  return (
    <div className={`kelpmux-shell ${phone ? "phone touch" : ""}`}>
      <div className={`app ${shown}`}>
        <div className="workspace">
          <div className="workspace-main">
            {shown === "repos" && project ? (
              <ReposView
                key={project.id}
                project={project}
                place={place}
                onPlace={setPlace}
                history={history ? bridge : null}
                git={git.byProject[project.id]}
                loading={!!git.loading[project.id]}
                error={git.errors[project.id]}
                onRefresh={() => git.refresh(project.id)}
                onOpenFile={(path) => w.opened.push(path)}
                onBack={phone ? () => setView("grid") : undefined}
              />
            ) : (
              <div id="grid">grid</div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <ToastProvider>
    <App />
  </ToastProvider>,
);
