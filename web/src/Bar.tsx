import { useEffect, useRef, useState } from "react";

/** Usage level for a percentage: ≥ 90 % is critical, ≥ 75 % a warning. */
export const usageLevel = (value?: number) =>
  value == null ? "" : value >= 90 ? "critical" : value >= 75 ? "warning" : "";

export function Bar({
  label,
  value,
  segmented = false,
}: {
  label: string;
  value?: number;
  segmented?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [segments, setSegments] = useState(12);
  useEffect(() => {
    if (!segmented || !ref.current) return;
    const observer = new ResizeObserver(([entry]) =>
      setSegments(Math.max(1, Math.floor(entry.contentRect.width / 8))),
    );
    observer.observe(ref.current);
    return () => observer.disconnect();
  }, [segmented]);
  const filled = Math.round(
    (Math.min(100, Math.max(0, value || 0)) * segments) / 100,
  );
  return (
    <div
      className={`metric metric-${label.toLowerCase()} ${usageLevel(value)}`}
    >
      <span title={label}>{label}</span>
      <div ref={ref} className={`bar ${segmented ? "segmented" : ""}`}>
        {segmented ? (
          Array.from({ length: segments }, (_, i) => (
            <i key={i} className={i < filled ? "filled" : "empty"} />
          ))
        ) : (
          <i style={{ width: `${Math.min(100, value || 0)}%` }} />
        )}
      </div>
      <strong>{value == null ? "—" : `${value}%`}</strong>
    </div>
  );
}
