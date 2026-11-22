// The sidebar's system vitals, drawn like the TUI's sidebar status
// (kelpserver crates/server/src/native/render.rs, draw_sidebar_status and
// draw_sidebar_disks; colours and glyphs from native/vitals.rs): CPU and
// memory as coloured meters, then the first two disks as pies filled by
// usage, in the TUI's colours. A click opens the System status dialog.
import { Tooltip } from "./Tooltip";

/** Disk usage, in percent, from which a disk turns red (the TUI's 196). */
export const diskCriticalPercent = 90;

/** The TUI's disk pie glyph for a usage (vitals.rs pie_glyph). */
export function pieGlyph(percent: number) {
  return percent >= 88
    ? "●"
    : percent >= 63
      ? "◕"
      : percent >= 38
        ? "◑"
        : percent >= 13
          ? "◔"
          : "○";
}

/** The filled fraction the TUI's glyph shows: 0, ¼, ½, ¾ or all. */
export function pieFraction(percent: number) {
  return { "○": 0, "◔": 0.25, "◑": 0.5, "◕": 0.75, "●": 1 }[pieGlyph(percent)]!;
}

/** A disk pie: an outlined circle filled like the TUI's glyph, drawn as SVG
 * so every font shows the same shape. */
export function DiskPie({ percent }: { percent: number }) {
  const fraction = pieFraction(percent);
  const r = 6,
    c = 7;
  const angle = fraction * 2 * Math.PI;
  const x = c + r * Math.sin(angle),
    y = c - r * Math.cos(angle);
  return (
    <svg
      className="disk-pie"
      viewBox="0 0 14 14"
      aria-hidden="true"
      focusable="false"
    >
      <circle cx={c} cy={c} r={r} className="disk-pie-ring" />
      {fraction >= 1 ? (
        <circle cx={c} cy={c} r={r} className="disk-pie-fill" />
      ) : fraction > 0 ? (
        <path
          className="disk-pie-fill"
          d={`M${c} ${c}V${c - r}A${r} ${r} 0 ${fraction > 0.5 ? 1 : 0} 1 ${x.toFixed(3)} ${y.toFixed(3)}Z`}
        />
      ) : null}
    </svg>
  );
}

const clamp = (value: number) => Math.min(100, Math.max(0, Math.round(value)));

export type VitalsProps = {
  vitals: { cpu?: number; memory?: number; disks?: number[] };
  /** Mount points of the disks, in the same order, once known. */
  mounts: string[];
  folded: boolean;
  onOpen: () => void;
  /** The pointer reached the disks: a chance to load their mount points. */
  onDisksHover?: () => void;
};

export function Vitals({
  vitals,
  mounts,
  folded,
  onOpen,
  onDisksHover,
}: VitalsProps) {
  // Like the TUI, the sidebar shows the first two disks.
  const disks = (vitals.disks || []).slice(0, 2).map(clamp);
  const diskName = (index: number) => mounts[index] || `Disk ${index + 1}`;
  const meter = (
    kind: "cpu" | "memory",
    label: string,
    short: string,
    value?: number,
  ) => (
    <div className={`vital vital-${kind}`} data-vital={kind}>
      <span className="vital-label">{folded ? short : label}</span>
      {!folded && (
        <span className="meter" aria-hidden="true">
          <i style={{ width: `${value == null ? 0 : clamp(value)}%` }} />
        </span>
      )}
      <b className="vital-value">{value == null ? "—" : `${clamp(value)}%`}</b>
    </div>
  );
  const summary = [
    `CPU ${vitals.cpu == null ? "unknown" : `${clamp(vitals.cpu)}%`}`,
    `memory ${vitals.memory == null ? "unknown" : `${clamp(vitals.memory)}%`}`,
    ...disks.map((d, i) => `${diskName(i)} ${d}% used`),
  ].join(", ");
  return (
    <button
      type="button"
      className="vitals metrics"
      onClick={onOpen}
      aria-label={`System status: ${summary}`}
    >
      {meter("cpu", "CPU", "C", vitals.cpu)}
      {meter("memory", "RAM", "M", vitals.memory)}
      {disks.length > 0 && (
        <div
          className="vital vital-disks"
          data-vital="disks"
          onPointerEnter={onDisksHover}
        >
          {!folded && <span className="vital-label">Disks</span>}
          <span className="disk-pies">
            {disks.map((percent, index) => (
              <Tooltip
                key={index}
                content={`${diskName(index)} · ${percent}% used`}
              >
                <span
                  className={`disk ${percent >= diskCriticalPercent ? "critical" : ""}`}
                  data-disk={index}
                >
                  <DiskPie percent={percent} />
                  {!folded && <b className="vital-value">{percent}%</b>}
                </span>
              </Tooltip>
            ))}
          </span>
        </div>
      )}
    </button>
  );
}
