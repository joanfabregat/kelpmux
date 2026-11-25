// GitHub in the web client (test/github.mjs), as main.tsx wires it,
// against a fake bridge, without any network or SSH endpoint: the real
// GitHubSettings (Settings → GitHub) and the real ReposView of a project,
// kept fresh by useProjectGit, with its Add repository picker, clone rows
// and GitHub facts. The bridge is `window.bridge(method, params)`, which
// the test exposes. `?view=settings` shows Settings → GitHub, otherwise
// the Repos view; `?mode=light` and `?phone=1` change the theme and the
// layout. Settings opened from the picker is recorded in `window.settings`.
import React, { useEffect } from "react";
import { createRoot } from "react-dom/client";
import { ReposView } from "../src/Repos";
import { GitHubSettings } from "../src/GitHub";
import { OPEN_SETTINGS } from "../src/Settings";
import { useProjectGit } from "../src/git-status";
import { ToastProvider } from "../src/Toast";
import type { Project } from "../src/projects";
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
const view = query.get("view") === "settings" ? "settings" : "repos";
const mode = query.get("mode") === "light" ? "light" : "dark";
const root = document.documentElement;
root.dataset.mode = mode;
for (const [key, value] of Object.entries({
  ...themeVariables("black", mode),
  ...interfaceVariables("black", mode),
}))
  root.style.setProperty(key, value);

const w = window as any;
w.settings = [];
window.addEventListener(OPEN_SETTINGS, (event) =>
  w.settings.push((event as CustomEvent<string>).detail),
);
const bridge = (method: string, params: any = {}): Promise<any> =>
  w.bridge(method, params);
const alpha: Project = {
  id: "alpha",
  name: "Alpha",
  folder: "~/work/alpha",
  path: "/home/me/work/alpha",
  color: "blue",
  missing: false,
};

function App() {
  useEffect(() => {
    w.fixtureReady = true;
  }, []);
  const git = useProjectGit({
    enabled: view === "repos",
    projects: [alpha.id],
    shown: view === "repos" ? alpha.id : "",
    query: bridge,
  });
  if (view === "settings")
    return (
      <div className="modal-shade">
        <section className="modal panel-modal" role="dialog" aria-label="Settings">
          <header>
            <h2>Settings</h2>
          </header>
          <div style={{ padding: "0 20px 20px" }}>
            <GitHubSettings rpc={bridge} />
          </div>
        </section>
      </div>
    );
  return (
    <div className={`kelpmux-shell ${phone ? "phone touch" : ""}`}>
      <div className="app repos">
        <div className="workspace">
          <div className="workspace-main">
            <ReposView
              project={alpha}
              git={git.byProject[alpha.id]}
              loading={!!git.loading[alpha.id]}
              error={git.errors[alpha.id]}
              onRefresh={() => git.refresh(alpha.id)}
              onNewTerminal={() => {}}
              github={bridge}
              onBack={phone ? () => {} : undefined}
            />
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
