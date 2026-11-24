// Navigation history (src/navigation.ts) as main.tsx wires it, without a
// server (test/history.mjs): the state main.tsx keeps for the place shown
// (single terminal, grid, file browser, open file, side panel, phone list,
// panels), useHistorySync mirroring it into the URL, and an apply function
// with main.tsx's fallbacks. Terminals have pane ids 11, 12 and 13 and show
// as 1, 2 and 3. The state becomes ready after a delay, like a sign-in; the
// `phone` query parameter shows the phone's list instead of the grid.
import React, { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  dialogFromParam,
  locationOf,
  useHistorySync,
  type AppLocation,
} from "../src/navigation";

const panes = [11, 12, 13];
const files = new Set(["/home/me/notes.md", "/home/me/src/main.rs"]);
const phone = new URLSearchParams(location.search).has("phone");
const parentPath = (path: string) =>
  path.slice(0, path.lastIndexOf("/")) || "/";

function App() {
  const [ready, setReady] = useState(false);
  const [view, setView] = useState<"single" | "grid" | "files">("grid");
  const [active, setActive] = useState(11);
  const [phoneList, setPhoneList] = useState(true);
  const [filesDir, setFilesDir] = useState("~");
  const [shownDoc, setShownDoc] = useState("");
  const [line, setLine] = useState(0);
  const [side, setSide] = useState("");
  const [dialog, setDialog] = useState("");
  const [toast, setToast] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => setReady(true), 150);
    return () => clearTimeout(timer);
  }, []);
  const shown = phone ? (view === "grid" ? "single" : view) : view;
  const here = locationOf({
    phoneHome: phone && phoneList && shown !== "files",
    view: shown,
    terminal: active,
    filesDir,
    file: shownDoc,
    fileLine: line,
    side: !phone && shown === "single" ? side : "",
    dialog,
    notifyPane: 0,
  });
  const navigation = useHistorySync({
    here: ready ? here : null,
    ready,
    context: phone ? "phone" : "wide",
    apply(target: AppLocation) {
      setDialog(dialogFromParam(target.dialog));
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
      if (!target.file) {
        setShownDoc("");
        if (target.dir) setFilesDir(target.dir);
        return;
      }
      setShownDoc(target.file);
      setLine(target.line);
      // The server's answer arrives later.
      setTimeout(() => {
        if (files.has(target.file)) return;
        setToast(`“${target.file}” is not available`);
        navigation.automatic();
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
  const openFiles = (dir?: string) => {
    if (dir) setFilesDir(dir);
    setShownDoc("");
    setPhoneList(false);
    setView("files");
  };
  const openDoc = (path: string, at = 0) => {
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
    here.view === "terminal"
      ? `terminal ${here.terminal}${here.side ? ` + ${here.side}` : ""}`
      : here.view === "files"
        ? here.file
          ? `file ${here.file}${here.line ? `:${here.line}` : ""}`
          : `folder ${here.dir}`
        : here.view;
  if (!ready) return <p id="signing-in">Signing in…</p>;
  return (
    <div>
      <nav>
        {panes.map((id, index) => (
          <button key={id} data-pane={id} onClick={() => openPane(id)}>
            Terminal {index + 1}
          </button>
        ))}
        <button id="grid" onClick={() => setView("grid")}>
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
            setShownDoc("");
          }}
        >
          Close file
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
