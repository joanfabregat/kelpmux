// Production build of the web client. Besides keeping the @kelpcore/web
// terminal layer out of dependency pre-bundling, two additions to Vite's
// defaults, both for optional error reporting (docs/web.md, "Error
// reporting"):
// - `__KELPMUX_RELEASE__`, the release name events carry (scripts/release.mjs,
//   identical to the kelpmux executable's).
// - Hidden source maps: generated without a `sourceMappingURL` comment and
//   moved from dist/ to sourcemaps/ (with sourcemaps/RELEASE), so the relay
//   never embeds or serves them; the deploy uploads them for that release.
import { defineConfig, type Plugin } from "vite";
import {
  mkdirSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { releaseName } from "./scripts/release.mjs";

const sourcemaps = join(dirname(fileURLToPath(import.meta.url)), "sourcemaps");

function maps(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true, recursive: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".map"))
    .map((entry) => join(entry.parentPath, entry.name));
}

function hiddenSourceMaps(release: string): Plugin {
  let outDir = "";
  return {
    name: "kelpmux-hidden-sourcemaps",
    apply: "build",
    configResolved(config) {
      outDir = resolve(config.root, config.build.outDir);
    },
    buildStart() {
      rmSync(sourcemaps, { recursive: true, force: true });
    },
    closeBundle() {
      mkdirSync(sourcemaps, { recursive: true });
      for (const file of maps(outDir)) {
        const target = join(sourcemaps, relative(outDir, file));
        mkdirSync(dirname(target), { recursive: true });
        renameSync(file, target);
      }
      writeFileSync(join(sourcemaps, "RELEASE"), `${release}\n`);
    },
  };
}

export default defineConfig(({ command }) => {
  const release = releaseName();
  return {
    define: { __KELPMUX_RELEASE__: JSON.stringify(release) },
    // @kelpcore/web is TypeScript source that creates its SharedWorker with
    // `new URL("./session-worker.ts", import.meta.url)`; pre-bundling would
    // move the importing module away from the worker file.
    optimizeDeps: { exclude: ["@kelpcore/web"] },
    build: { sourcemap: "hidden" },
    plugins: command === "build" ? [hiddenSourceMaps(release)] : [],
  };
});
