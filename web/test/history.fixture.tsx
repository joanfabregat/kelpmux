// Navigation history (src/navigation.ts) as main.tsx wires it, without a
// server (test/history.mjs): the state main.tsx keeps for the place shown
// (single terminal, grid, file browser, open file, side panel, phone list,
// panels), useHistorySync mirroring it into the URL, and an apply function
// with main.tsx's fallbacks. Terminals have pane ids 11, 12 and 13 and show
// as 1, 2 and 3. The state becomes ready after a delay, like a sign-in; the
// `phone` query parameter shows the phone's list instead of the grid. The
// file root is /home/me: a folder or file outside it shows the root. The
// project "alpha" (folder /home/me/alpha) scopes the grid, the file
// browser and Recently closed (`?project=`); another id falls back to
// every project.
import React, { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  dialogFromParam,
  locationOf,
  useHistorySync,
  type AppLocation,
} from "../src/navigation";
import { outsideRootMessage, withinRoot } from "../src/files/root";
import { neighbourTab } from "../src/files/tabs";

const panes = [11, 12, 13];
/** The server's file root (Welcome `file_limits.root`). */
const root = "/home/me";
const files = new Set(["/home/me/notes.md", "/home/me/src/main.rs"]);
const phone = new URLSearchParams(location.search).has("phone");
const parentPath = (path: string) =>
  path.slice(0, path.lastIndexOf("/")) || "/";

function App() {
  const [ready, setReady] = useState(false);
  const [view, setView] = useState<"single" | "grid" | "files" | "repos">(
    "grid",
  );
  const [active, setActive] = useState(11);
  const [phoneList, setPhoneList] = useState(true);
  const [filesDir, setFilesDir] = useState("~");
  const [shownDoc, setShownDoc] = useState("");
  /** The Files view's tabs: the files open in the main area. */
  const [tabs, setTabs] = useState<string[]>([]);
  const [line, setLine] = useState(0);
  const [side, setSide] = useState("");
  const [dialog, setDialog] = useState("");
  const [toast, setToast] = useState("");
  const [project, setProject] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => setReady(true), 150);
    return () => clearTimeout(timer);
  }, []);
  const shown =
    phone && view === "grid"
      ? "single"
      : view === "repos" && !project
        ? "grid"
        : view;
  const here = locationOf({
    phoneHome: phone && phoneList && shown !== "files" && shown !== "repos",
    view: shown,
    terminal: active,
    filesDir,
    file: shownDoc,
    fileLine: line,
    side: !phone && shown === "single" ? side : "",
    dialog,
    notifyPane: 0,
    project,
  });
  const navigation = useHistorySync({
    here: ready ? here : null,
    ready,
    context: phone ? "phone" : "wide",
    apply(target: AppLocation) {
      setDialog(dialogFromParam(target.dialog));
      // As main.tsx: an unknown project shows every project.
      if (target.project && target.project !== "alpha") {
        setToast(`Project ${target.project} does not exist`);
        navigation.automatic();
      }
      setProject(target.project === "alpha" ? "alpha" : "");
      if (target.view === "repos") {
        // As main.tsx: a Repos view needs a project (an unknown one has
        // had its toast above).
        if (target.project !== "alpha") {
          if (!target.project) setToast("Choose a project");
          navigation.automatic();
          if (phone) setPhoneList(true);
          else setView("grid");
          return;
        }
        setPhoneList(false);
        setView("repos");
        return;
      }
      if (target.view === "home") return void (phone && setPhoneList(true));
      if (target.view === "grid") {
        if (phone) setPhoneList(true);
        else setView("grid");
        return;
      }
      if (target.view === "terminal") {
        if (target.terminal && !panes.includes(target.terminal)) {
          setToast(`Terminal ${target.terminal} is closed`);
          navigation.automatic();
          if (phone) setPhoneList(true);
          else setView("grid");
          return;
        }
        setPhoneList(false);
        setView("single");
        if (target.terminal) setActive(target.terminal);
        setSide(target.side);
        return;
      }
      setPhoneList(false);
      setView("files");
      // Outside the file root (/home/me): the root instead.
      if (!withinRoot(target.file || target.dir || "~", root, "/home/me")) {
        setToast(outsideRootMessage);
        navigation.automatic();
        setShownDoc("");
        setFilesDir(root);
        return;
      }
      if (!target.file) {
        setShownDoc("");
        if (target.dir) setFilesDir(target.dir);
        return;
      }
      setShownDoc(target.file);
      addTab(target.file);
      setLine(target.line);
      // The server's answer arrives later.
      setTimeout(() => {
        if (files.has(target.file)) return;
        setToast(`“${target.file}” is not available`);
        navigation.automatic();
        setTabs((all) => all.filter((path) => path !== target.file));
        setShownDoc("");
        setFilesDir(parentPath(target.file));
      }, 50);
    },
  });
  // main.tsx's actions.
  const openPane = (id: number) => {
    setView("single");
    setActive(id);
    setPhoneList(false);
  };
  const openFiles = (dir?: string, scope = "") => {
    setProject(scope);
    if (dir) setFilesDir(dir);
    setShownDoc("");
    setPhoneList(false);
    setView("files");
  };
  function addTab(path: string) {
    setTabs((all) => (all.includes(path) ? all : [...all, path]));
  }
  const openDoc = (path: string, at = 0) => {
    addTab(path);
    setShownDoc(path);
    setLine(at);
    setPhoneList(false);
    setView("files");
  };
  useEffect(() => {
    const keys = (event: KeyboardEvent) => {
      if (event.key === "Escape") setDialog("");
      if (event.altKey && event.code === "Tab") {
        event.preventDefault();
        setView((v) => (v === "grid" ? "single" : "grid"));
      }
    };
    window.addEventListener("keydown", keys);
    return () => window.removeEventListener("keydown", keys);
  }, []);
  const latest = useRef(navigation);
  latest.current = navigation;
  // The browser normalises `~` to the home folder after listing it.
  useEffect(() => {
    if (shown !== "files" || shownDoc || filesDir !== "~") return;
    const timer = setTimeout(() => {
      latest.current.automatic();
      setFilesDir("/home/me");
    }, 20);
    return () => clearTimeout(timer);
  }, [shown, shownDoc, filesDir]);
  (window as any).fixtureReady = ready;
  const place =
    (
    here.view === "terminal"
      ? `terminal ${here.terminal}${here.side ? ` + ${here.side}` : ""}`
      : here.view === "files"
        ? here.file
          ? `file ${here.file}${here.line ? `:${here.line}` : ""}`
          : `folder ${here.dir}`
        : here.view) + (here.project ? ` @${here.project}` : "");
  if (!ready) return <p id="signing-in">Signing in…</p>;
  return (
    <div>
      <nav>
        {panes.map((id, index) => (
          <button key={id} data-pane={id} onClick={() => openPane(id)}>
            Terminal {index + 1}
          </button>
        ))}
        <button
          id="alpha-grid"
          onClick={() => {
            setProject("alpha");
            setView("grid");
          }}
        >
          Alpha's terminals
        </button>
        <button
          id="alpha-files"
          onClick={() => {
            openFiles("/home/me/alpha", "alpha");
          }}
        >
          Alpha's files
        </button>
        <button
          id="alpha-closed"
          onClick={() => {
            setProject("alpha");
            setDialog("recovery");
          }}
        >
          Alpha's closed terminals
        </button>
        <button
          id="alpha-repos"
          onClick={() => {
            setProject("alpha");
            setPhoneList(false);
            setView("repos");
          }}
        >
          Alpha's repos
        </button>
        <button
          id="grid"
          onClick={() => {
            setProject("");
            setView("grid");
          }}
        >
          Grid
        </button>
        <button id="home" onClick={() => setPhoneList(true)}>
          List
        </button>
        <button id="files" onClick={() => openFiles("~")}>
          Files
        </button>
        <button id="src" onClick={() => openFiles("/home/me/src")}>
          src
        </button>
        {/* The column view (FilesBrowser's onDir, as main.tsx wires it):
            a folder clicked in a column pushes; the keyboard, or a file
            selected in an earlier column, replaces. */}
        <button
          id="column-click"
          onClick={() => setFilesDir("/home/me/src/lib")}
        >
          lib clicked
        </button>
        <button
          id="column-key"
          onClick={() => {
            navigation.automatic();
            setFilesDir("/home/me/src/bin");
          }}
        >
          bin by keyboard
        </button>
        <button
          id="column-file"
          onClick={() => {
            navigation.automatic();
            setFilesDir("/home/me");
          }}
        >
          a file selected in the home column
        </button>
        <button id="open-file" onClick={() => openDoc("/home/me/src/main.rs")}>
          main.rs
        </button>
        <button
          id="open-line"
          onClick={() => openDoc("/home/me/src/main.rs", 42)}
        >
          main.rs:42
        </button>
        <button
          id="beside"
          onClick={() => {
            setSide("/home/me/notes.md");
            openPane(active);
          }}
        >
          notes.md beside
        </button>
        <button
          id="close-file"
          onClick={() => {
            navigation.automatic();
            setTabs((all) => all.filter((path) => path !== shownDoc));
            setShownDoc("");
          }}
        >
          Close file
        </button>
        <button id="open-notes" onClick={() => openDoc("/home/me/notes.md")}>
          notes.md
        </button>
        {/* The Files view's tabs (main.tsx's FileTabs): the browser's,
            then the open files'; closing the shown one shows its
            neighbour (or the browser) in place of its entry. */}
        <button id="tab-browser" onClick={() => openFiles()}>
          Files tab
        </button>
        {tabs.map((path) => (
          <button key={path} data-tab={path} onClick={() => openDoc(path)}>
            {path}
          </button>
        ))}
        <button
          id="close-tab"
          onClick={() => {
            navigation.automatic();
            setTabs((all) => all.filter((path) => path !== shownDoc));
            setShownDoc(neighbourTab(tabs, shownDoc));
            setLine(0);
          }}
        >
          Close the shown tab
        </button>
        <button id="settings" onClick={() => setDialog("config")}>
          Settings
        </button>
        <button id="terminals" onClick={() => setDialog("terminals")}>
          Terminals
        </button>
      </nav>
      <output id="place">{place}</output>
      <output id="toast">{toast}</output>
      {dialog && (
        <section role="dialog" data-dialog={dialog}>
          <button id="close-dialog" onClick={() => setDialog("")}>
            Close
          </button>
          <button id="help" onClick={() => setDialog("help")}>
            Help
          </button>
          {dialog === "terminals" && (
            <button
              id="table-open"
              onClick={() => {
                setDialog("");
                openPane(13);
              }}
            >
              Open terminal 3
            </button>
          )}
        </section>
      )}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
