// The idle warning as main.tsx wires it, against the real shared worker
// (src/terminal/session-worker.ts, built to /worker.js) instead of SSH
// (test/idle.mjs). Each page joins the worker as SharedSession does; the
// first page plays the connection tab and answers `host_start` with a
// welcome. The page reports activity with the same hook as main.tsx and
// shows the same IdleWarning; a lock shows the worker's message.
import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  IdleWarning,
  useActivityReports,
  type IdleNotice,
} from "../src/IdleWarning";
import "../src/styles/base.css";
import "../src/styles/dialogs.css";

declare global {
  interface Window {
    idleMessages: any[];
  }
}
window.idleMessages = [];

function App() {
  const [port, setPort] = useState<MessagePort | null>(null);
  const [idle, setIdle] = useState<IdleNotice | null>(null);
  const [locked, setLocked] = useState("");
  const [online, setOnline] = useState(false);
  useEffect(() => {
    const worker = new SharedWorker("/worker.js", { name: "kelpmux-ssh-v1" });
    const port = worker.port;
    let pulse: ReturnType<typeof setInterval> | undefined;
    port.onmessage = ({ data }) => {
      window.idleMessages.push(data);
      if (data.type === "host_start")
        port.postMessage({
          type: "host_event",
          event: { Stream: { type: "welcome", client: 1, capabilities: [] } },
        });
      else if (data.type === "event" && data.event?.Stream?.type === "welcome")
        setOnline(true);
      else if (data.type === "idle")
        setIdle(
          data.warning ? { idleFor: data.idleFor, lockAt: data.lockAt } : null,
        );
      else if (data.type === "closed") {
        setOnline(false);
        setIdle(null);
        setLocked(data.message || "closed");
        clearInterval(pulse);
      }
    };
    port.start();
    void fetch("/connection.json")
      .then((response) => response.json())
      .then((config) => {
        port.postMessage({
          type: "join",
          config,
          identity: {
            origin: config.origin,
            rpId: config.rpId,
            credentialId: "fixture",
          },
          resumeOnly: new URLSearchParams(location.search).has("resume"),
        });
        pulse = setInterval(() => port.postMessage({ type: "pulse" }), 1000);
        setPort(port);
      });
  }, []);
  const report = () => port?.postMessage({ type: "activity" });
  useActivityReports(online ? report : null, !!idle);
  return (
    <main style={{ padding: 16 }}>
      <p data-state={online ? "online" : locked ? "locked" : "connecting"}>
        {online ? "Signed in" : locked || "Connecting"}
      </p>
      {idle && <IdleWarning notice={idle} onStay={report} />}
    </main>
  );
}
createRoot(document.getElementById("root")!).render(<App />);
