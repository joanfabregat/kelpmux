// The error-reporting release name, computed exactly as build.rs computes it
// for the kelpmux executable, so browser and Rust events of one build share
// one release (and the deploy uploads source maps for it):
//   KELPMUX_RELEASE if set, else kelpmux@VERSION+SHA12 with the commit from
//   KELPMUX_GIT_SHA or Git, else kelpmux@VERSION (VERSION from Cargo.toml).
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const repository = new URL("../../", import.meta.url);

export function validRelease(name) {
  return (
    typeof name === "string" &&
    name.length > 0 &&
    name.length <= 200 &&
    name !== "." &&
    name !== ".." &&
    /^[A-Za-z0-9._@+-]+$/.test(name)
  );
}

/** The `[package]` version of the root Cargo.toml. */
export function crateVersion(manifest) {
  const match = /^\[package\][^[]*?^version = "([^"]+)"$/m.exec(manifest);
  if (!match) throw Error("no package version in Cargo.toml");
  return match[1];
}

function gitHead() {
  try {
    return execFileSync(
      "git",
      ["-c", "safe.directory=*", "rev-parse", "HEAD"],
      {
        cwd: repository,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      },
    ).trim();
  } catch {
    return undefined;
  }
}

export function releaseName({
  env = process.env,
  manifest = readFileSync(new URL("Cargo.toml", repository), "utf8"),
  head = gitHead,
} = {}) {
  if (env.KELPMUX_RELEASE) {
    if (!validRelease(env.KELPMUX_RELEASE))
      throw Error(
        `invalid KELPMUX_RELEASE ${JSON.stringify(env.KELPMUX_RELEASE)}`,
      );
    return env.KELPMUX_RELEASE;
  }
  const version = crateVersion(manifest);
  const sha = (env.KELPMUX_GIT_SHA || head() || "").toLowerCase();
  if (!sha) return `kelpmux@${version}`;
  if (!/^[0-9a-f]{12,}$/.test(sha))
    throw Error(`invalid KELPMUX_GIT_SHA ${JSON.stringify(sha)}`);
  return `kelpmux@${version}+${sha.slice(0, 12)}`;
}
