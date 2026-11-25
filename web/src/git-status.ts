// Keeps the projects' Git facts (bridge `git`, API minor 6) fresh while
// they can be seen: the project whose Repos view shows is asked again every
// ten seconds with its changed files, and every project is swept, one at a
// time and without files, once a minute for the sidebar's changes marks and
// the file browser's checkout lines. Nothing is asked while the page is
// hidden. The bridge bounds the work itself (a short queue and two
// threads, a time budget per project) and caches a checkout for a few
// seconds; `fresh` (opening the view, Refresh) bypasses that cache.
import { useEffect, useRef, useState } from "react";
import { readProjectGit, type ProjectGit } from "./git";

/** How often the Repos view's project is asked again. */
export const REPOS_REFRESH_MS = 10_000;
/** How often every project is swept for the sidebar's marks. */
export const SWEEP_MS = 60_000;

const pageVisible = () =>
  typeof document === "undefined" || document.visibilityState !== "hidden";

export function useProjectGit(options: {
  /** The bridge offers `git`. */
  enabled: boolean;
  /** Every project id (Scratch's included), in the sidebar's order. */
  projects: string[];
  /** The project whose Repos view shows ("" for none). */
  shown: string;
  /** A bridge request that rejects with the bridge's error. */
  query: (method: string, params: any) => Promise<any>;
}) {
  const { enabled, projects, shown } = options;
  const [byProject, setByProject] = useState<Record<string, ProjectGit>>({});
  const [loading, setLoading] = useState<Record<string, boolean>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const latest = useRef(options);
  latest.current = options;
  const inFlight = useRef(new Set<string>());

  async function load(project: string, files: boolean, fresh: boolean) {
    if (inFlight.current.has(project)) return;
    inFlight.current.add(project);
    setLoading((all) => ({ ...all, [project]: true }));
    try {
      const git = readProjectGit(
        await latest.current.query("git", { project, files, fresh }),
      );
      if (git) {
        setByProject((all) => ({ ...all, [project]: git }));
        setErrors((all) => {
          if (!(project in all)) return all;
          const { [project]: _, ...rest } = all;
          return rest;
        });
      }
    } catch (error: any) {
      setErrors((all) => ({
        ...all,
        [project]: error?.message || String(error),
      }));
    } finally {
      inFlight.current.delete(project);
      setLoading((all) => ({ ...all, [project]: false }));
    }
  }

  // The shown project: at once (fresh), then every ten seconds.
  useEffect(() => {
    if (!enabled || !shown) return;
    void load(shown, true, true);
    const timer = setInterval(() => {
      if (pageVisible()) void load(shown, true, false);
    }, REPOS_REFRESH_MS);
    return () => clearInterval(timer);
  }, [enabled, shown]);

  // Every other project, one at a time, now and once a minute.
  const projectKey = projects.join("\n");
  useEffect(() => {
    if (!enabled || !projectKey) return;
    let alive = true;
    const sweep = async () => {
      for (const project of latest.current.projects) {
        if (!alive || !pageVisible()) return;
        if (project === latest.current.shown) continue;
        await load(project, false, false);
      }
    };
    const first = setTimeout(sweep, 500);
    const timer = setInterval(sweep, SWEEP_MS);
    const visible = () => {
      if (pageVisible()) void sweep();
    };
    document.addEventListener("visibilitychange", visible);
    return () => {
      alive = false;
      clearTimeout(first);
      clearInterval(timer);
      document.removeEventListener("visibilitychange", visible);
    };
  }, [enabled, projectKey]);

  return {
    byProject,
    loading,
    errors,
    /** Asks for a project's facts again now, bypassing the cache. */
    refresh(project: string) {
      void load(project, true, true);
    },
  };
}
