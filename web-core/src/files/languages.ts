// CodeMirror support for each Language, loaded on demand. Derived from
// KelpHub's workspace editor (web/src/files/languages.ts).
import type { Extension } from "@codemirror/state";
import { StreamLanguage } from "@codemirror/language";
import type { Language } from "./model.ts";

/** Load a language's CodeMirror support on demand; each grammar is its own
 * chunk, fetched only when a file of that type is opened. */
export async function loadLanguage(language: Language): Promise<Extension> {
  switch (language) {
    case "javascript":
    case "jsx":
    case "typescript":
    case "tsx": {
      const { javascript } = await import("@codemirror/lang-javascript");
      return javascript({
        jsx: language === "jsx" || language === "tsx",
        typescript: language === "typescript" || language === "tsx",
      });
    }
    case "json":
      return (await import("@codemirror/lang-json")).json();
    case "markdown":
      return (await import("@codemirror/lang-markdown")).markdown();
    case "css":
      return (await import("@codemirror/lang-css")).css();
    case "html":
      return (await import("@codemirror/lang-html")).html();
    case "python":
      return (await import("@codemirror/lang-python")).python();
    case "rust":
      return (await import("@codemirror/lang-rust")).rust();
    case "yaml":
      return (await import("@codemirror/lang-yaml")).yaml();
    case "shell":
      return StreamLanguage.define(
        (await import("@codemirror/legacy-modes/mode/shell")).shell,
      );
    case "toml":
      return StreamLanguage.define(
        (await import("@codemirror/legacy-modes/mode/toml")).toml,
      );
    case "dockerfile":
      return StreamLanguage.define(
        (await import("@codemirror/legacy-modes/mode/dockerfile")).dockerFile,
      );
    default:
      return [];
  }
}
