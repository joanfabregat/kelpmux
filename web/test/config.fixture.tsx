// Settings → Files & folders as main.tsx wires it (the Settings dialog with
// the configuration problems banner), against a fake bridge
// (test/config.mjs). The fake `config_set` refuses a relative path the way
// the bridge's validator does, records each request in `window.saved`, and
// `window.push(config)` stands in for a `Config` notice. `?server_root=…`
// sets the file root the running server reports.
import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { Settings } from "../src/Settings";
import { ConfigProblems, FilesAndFolders } from "../src/UserConfig";
import {
  readUserConfig,
  type ConfigChanges,
  type UserConfigState,
} from "../src/user-config";
import { interfaceVariables, themeVariables } from "../src/appearance";
import "../src/styles/base.css";
import "../src/styles/shell.css";
import "../src/styles/dialogs.css";
import "../src/styles/kelpmux.css";

const query = new URLSearchParams(location.search);
const root = document.documentElement;
const mode = query.get("mode") === "light" ? "light" : "dark";
root.dataset.mode = mode;
for (const [key, value] of Object.entries({
  ...themeVariables("black", mode),
  ...interfaceVariables("black", mode),
}))
  root.style.setProperty(key, value);

const home = "/home/joan";
const resolve = (path: string) =>
  path === "~"
    ? home
    : path.startsWith("~/")
      ? `${home}/${path.slice(2)}`
      : path;
const w = window as any;
w.saved = [];
let config: UserConfigState = readUserConfig({
  path: `${home}/.config/kelpmux/config.toml`,
  exists: true,
  revision: "1",
  current: true,
  values: {
    files: { root: "~", extra_roots: ["/mnt/data"] },
    projects: { base: "~/projects" },
    scratch: { folder: "~/scratch" },
  },
  resolved: {
    files: { root: home, extra_roots: ["/mnt/data"] },
    projects: { base: `${home}/projects` },
    scratch: { folder: `${home}/scratch` },
  },
  errors: [],
})!;
let setter: (config: UserConfigState) => void = () => {};
w.push = (value: any) => setter(readUserConfig(value)!);
w.current = () => config;

async function save(changes: ConfigChanges) {
  w.saved.push(changes);
  const errors = Object.entries(changes).flatMap(([key, value]) =>
    (Array.isArray(value) ? value : [value])
      .filter((path) => !path.startsWith("/") && !path.startsWith("~"))
      .map((path) => ({
        key,
        line: 1,
        column: 1,
        message: `\`${key}\` must be an absolute path or start with \`~/\` (${path})`,
      })),
  );
  if (errors.length) return { saved: false, errors, config };
  const values = structuredClone(config.values);
  for (const [key, value] of Object.entries(changes)) {
    if (key === "files.root") values.files.root = value as string;
    if (key === "files.extra_roots")
      values.files.extra_roots = value as string[];
    if (key === "projects.base") values.projects.base = value as string;
    if (key === "scratch.folder") values.scratch.folder = value as string;
  }
  config = {
    ...config,
    revision: String(Number(config.revision) + 1),
    values,
    resolved: {
      files: {
        root: resolve(values.files.root),
        extra_roots: values.files.extra_roots.map(resolve),
      },
      projects: { base: resolve(values.projects.base) },
      scratch: { folder: resolve(values.scratch.folder) },
    },
  };
  setter(config);
  return { saved: true, errors: [], config };
}

function App() {
  const [state, setState] = useState(config);
  setter = (next) => {
    config = next;
    setState(next);
  };
  return (
    <div className="modal-shade settings-shade">
      <section className="modal config-modal" role="dialog" aria-modal="true">
        <header>
          <h2>Settings</h2>
        </header>
        <Settings
          active="files"
          onChange={() => {}}
          alerts={state.errors.length ? ["files"] : []}
        >
          <ConfigProblems config={state} />
          <section>
            <FilesAndFolders
              config={state}
              save={save}
              serverRoot={query.get("server_root") || home}
            />
          </section>
        </Settings>
      </section>
    </div>
  );
}
createRoot(document.getElementById("root")!).render(<App />);
w.fixtureReady = true;
