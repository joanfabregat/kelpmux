import { useState } from "react";
import { Bar } from "./Bar";
import { bytes } from "./format";
import { Icon } from "./status";

export function Stats({
  stats: s,
  vitals,
  query,
  setQuery,
}: {
  stats: any;
  vitals: any;
  query: string;
  setQuery: (v: string) => void;
}) {
  const [sort, setSort] = useState("cpu"),
    [desc, setDesc] = useState(true),
    [tab, setTab] = useState("main");
  const processes = [...(s.processes || [])]
    .filter((p: any) =>
      `${p.pid} ${p.user} ${p.command}`
        .toLowerCase()
        .includes(query.toLowerCase()),
    )
    .sort(
      (a: any, b: any) => (desc ? -1 : 1) * ((a[sort] || 0) - (b[sort] || 0)),
    );
  return (
    <div className="stats-body">
      <div className="stats-summary">
        <div>
          <h3>
            <Icon name="activity" size="sm" />
            CPU
          </h3>
          <div className="cpu-meters">
            {(s.cpus || []).map((n: number, i: number) => (
              <Bar key={i} label={`${i}`} value={n} />
            ))}
          </div>
          <h3>
            <Icon name="hard-drive" size="sm" />
            Disks
          </h3>
          {(s.disks || []).map((d: any) => (
            <div key={d.mount} className="stats-disk">
              <Bar label={d.mount} value={d.percent} />
              <small>
                {bytes(d.available)} available / {bytes(d.total)}
              </small>
            </div>
          ))}
        </div>
        <div>
          <h3>
            <Icon name="layers" size="sm" />
            Memory
          </h3>
          <Bar
            label="RAM"
            value={
              s.memory_total
                ? Math.round((s.memory_used / s.memory_total) * 100)
                : undefined
            }
          />
          <p>
            {bytes(s.memory_used)} / {bytes(s.memory_total)}
          </p>
          <Bar
            label="Swap"
            value={
              s.swap_total ? Math.round((s.swap_used / s.swap_total) * 100) : 0
            }
          />
          <p>
            {bytes(s.swap_used)} / {bytes(s.swap_total)}
          </p>
        </div>
        <div>
          <h3>
            <Icon name="monitor" size="sm" />
            Host
          </h3>
          <p>
            {vitals.date} {vitals.time} · Uptime{" "}
            {Math.floor((s.uptime || 0) / 86400)}d{" "}
            {Math.floor(((s.uptime || 0) % 86400) / 3600)}h
          </p>
          <p>Load average: {s.load || "—"}</p>
          <p>
            Network: ↓ {bytes(s.network?.[0])}/s · ↑ {bytes(s.network?.[1])}/s
          </p>
          <p>
            Disk I/O: ↓ {bytes(s.disk_io?.[0])}/s · ↑ {bytes(s.disk_io?.[1])}/s
          </p>
        </div>
      </div>
      <div className="stats-process-toolbar">
        <div
          className="seg stats-tabs"
          role="group"
          aria-label="Process columns"
        >
          <button aria-pressed={tab === "main"} onClick={() => setTab("main")}>
            Main
          </button>
          <button aria-pressed={tab === "io"} onClick={() => setTab("io")}>
            I/O
          </button>
        </div>
        <div className="process-search">
          <Icon name="search" size="sm" className="faint" />
          <input
            placeholder="/ search processes"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <span>
            {s.tasks || 0} tasks · {s.running || 0} running
          </span>
        </div>
      </div>
      <div className="process-table">
        <table>
          <thead>
            <tr>
              {[
                ["pid", "PID"],
                ["user", "USER"],
                ["cpu", "CPU%"],
                ["memory", "MEM%"],
                ["resident", "RES"],
                ...(tab === "io"
                  ? [
                      ["read", "READ/s"],
                      ["write", "WRITE/s"],
                    ]
                  : [
                      ["runtime", "TIME"],
                      ["state", "S"],
                    ]),
                ["command", "Command"],
              ].map(([key, label]) => (
                <th
                  key={key}
                  className={sort === key ? "sorted" : ""}
                  onClick={() => {
                    if (sort === key) setDesc(!desc);
                    else {
                      setSort(key);
                      setDesc(true);
                    }
                  }}
                >
                  {label}
                  {sort === key ? (desc ? "↓" : "↑") : ""}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {processes.map((p: any) => (
              <tr key={`${p.pid}:${p.birth}`}>
                <td>{p.pid}</td>
                <td>{p.user}</td>
                <td className="cpu">
                  {p.cpu == null ? "—" : (p.cpu / 10).toFixed(1)}
                </td>
                <td className="mem">{(p.memory / 10).toFixed(1)}</td>
                <td>{bytes(p.resident)}</td>
                {tab === "io" ? (
                  <>
                    <td>{bytes(p.read)}/s</td>
                    <td>{bytes(p.write)}/s</td>
                  </>
                ) : (
                  <>
                    <td>
                      {Math.floor(p.runtime / 60)}:
                      {String(p.runtime % 60).padStart(2, "0")}
                    </td>
                    <td>{p.state}</td>
                  </>
                )}
                <td>{p.command}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
