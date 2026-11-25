import { useEffect, useRef, useState } from "react";
import type { Store } from "./Terminal";
import type { SearchMatch } from "./types";
export function PaneFind({
  store,
  pane,
  onReveal,
  onClose,
  onError,
  initialQuery = "",
}: {
  store: Store;
  pane: number;
  onReveal: (match: SearchMatch | null) => void;
  onClose: () => void;
  onError: (error: unknown) => void;
  /** Query to start with, for example from a workspace search. */
  initialQuery?: string;
}) {
  const [query, setQuery] = useState(initialQuery);
  const [hits, setHits] = useState<SearchMatch[]>([]);
  const [truncated, setTruncated] = useState(false);
  const [index, setIndex] = useState(0);
  const [busy, setBusy] = useState(false);
  const reveal = useRef(onReveal);
  reveal.current = onReveal;
  const errors = useRef(onError);
  errors.current = onError;
  useEffect(() => {
    let alive = true;
    setHits([]);
    reveal.current(null);
    setIndex(0);
    if (!query) {
      setBusy(false);
      return;
    }
    setBusy(true);
    const timer = setTimeout(async () => {
      try {
        const response = await store.session?.rpc("search", {
          pane,
          query,
          workspace: store.workspace,
        });
        if (!alive) return;
        const found = response?.PaneSearch?.hits || [];
        setHits(found);
        setTruncated(!!response?.PaneSearch?.truncated);
        setBusy(false);
        if (found.length) reveal.current(found[0]);
      } catch (error) {
        if (alive) {
          setBusy(false);
          errors.current(error);
        }
      }
    }, 160);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [query, pane, store]);
  function move(delta: number) {
    if (!hits.length) return;
    const next = (index + delta + hits.length) % hits.length;
    setIndex(next);
    reveal.current(hits[next]);
  }
  return (
    <div
      className="pane-find"
      role="search"
      aria-label="Search selected pane"
      onPointerDown={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.key === "Escape") {
          event.preventDefault();
          onClose();
        }
        if (event.key === "Enter") {
          event.preventDefault();
          move(event.shiftKey ? -1 : 1);
        }
      }}
    >
      <input
        autoFocus
        aria-label="Search terminal content"
        maxLength={256}
        value={query}
        placeholder="Find in this pane…"
        onChange={(event) => setQuery(event.target.value)}
      />
      <span role="status">
        {busy
          ? "Searching…"
          : query
            ? hits.length
              ? `${index + 1}/${hits.length}${truncated ? "+" : ""}`
              : "No matches"
            : ""}
      </span>
      <button
        title="Previous match"
        disabled={!hits.length}
        onClick={() => move(-1)}
      >
        ↑
      </button>
      <button
        title="Next match"
        disabled={!hits.length}
        onClick={() => move(1)}
      >
        ↓
      </button>
      <button title="Close search" onClick={onClose}>
        ×
      </button>
    </div>
  );
}
