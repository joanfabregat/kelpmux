// Lets the Node unit tests import @kelpcore/web, the browser terminal layer
// that KelpCore publishes as TypeScript source. Node strips types from .ts
// files itself, but refuses to under node_modules
// (ERR_UNSUPPORTED_NODE_MODULES_TYPE_STRIPPING), so this load hook strips
// them for that one package with Node's own module.stripTypeScriptTypes.
// No dependency involved. Usage: node --import ./test/ts-loader.mjs --test.
// A .tsx file (JSX) cannot be type-stripped; tests must not import one.
import { register, stripTypeScriptTypes } from "node:module";
import { readFile } from "node:fs/promises";
import { isMainThread } from "node:worker_threads";

const scope = /\/node_modules\/@kelpcore\/web\/.+\.ts$/;

export async function load(url, context, nextLoad) {
  if (url.startsWith("file:") && scope.test(new URL(url).pathname)) {
    const source = await readFile(new URL(url), "utf8");
    return {
      format: "module",
      source: stripTypeScriptTypes(source),
      shortCircuit: true,
    };
  }
  return nextLoad(url, context);
}

// The hooks run in their own thread, which evaluates this module again.
if (isMainThread) register(import.meta.url);
