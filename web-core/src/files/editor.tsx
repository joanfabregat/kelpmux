// CodeMirror 6 text editor for files on the server host (KelpServer
// `files`). Derived from KelpHub's workspace editor (web/src/files/
// editor.tsx), without its Git diff view. Applications import it lazily
// (`import("@kelpcore/web/files/editor.tsx")`) so CodeMirror is fetched
// only when the first file opens. It holds the buffer only: loading,
// saving and conflicts belong to the host (see model.ts), so the same
// editor can sit in a main view, a side panel or a dialog.
import { useEffect, useMemo, useRef } from "react";
import {
  Compartment,
  EditorState,
  Prec,
  type Extension,
  type Text,
} from "@codemirror/state";
import {
  EditorView,
  crosshairCursor,
  drawSelection,
  dropCursor,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  rectangularSelection,
} from "@codemirror/view";
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
} from "@codemirror/commands";
import {
  bracketMatching,
  foldGutter,
  foldKeymap,
  indentOnInput,
} from "@codemirror/language";
import {
  highlightSelectionMatches,
  search,
  searchKeymap,
} from "@codemirror/search";
import { editorHighlight, editorTheme } from "./theme.ts";
import { loadLanguage } from "./languages.ts";
import type { Language } from "./model.ts";

/** What the host can ask of a mounted editor. */
export type EditorHandle = { content: () => string; focus: () => void };

function common(language: Compartment): Extension[] {
  return [
    lineNumbers(),
    highlightActiveLineGutter(),
    highlightSpecialChars(),
    foldGutter(),
    drawSelection(),
    dropCursor(),
    EditorState.allowMultipleSelections.of(true),
    indentOnInput(),
    bracketMatching(),
    rectangularSelection(),
    crosshairCursor(),
    highlightActiveLine(),
    highlightSelectionMatches(),
    search({ top: true }),
    editorTheme,
    editorHighlight,
    language.of([]),
    keymap.of([...defaultKeymap, ...searchKeymap, ...foldKeymap]),
  ];
}

/** Keep CRLF files CRLF: CodeMirror otherwise joins lines with "\n". */
const separator = (content: string) =>
  content.includes("\r\n") ? [EditorState.lineSeparator.of("\r\n")] : [];

/**
 * An editable buffer of `saved`. A new `version` replaces the buffer with
 * `saved` (a reload) and resets its history; a new `saved` with the same
 * version becomes the clean baseline (after a save). `onDirty` reports
 * when the buffer starts or stops differing from the baseline, `onSave`
 * runs on Ctrl/Cmd-S, and `register` hands the host a handle while mounted.
 */
export function TextEditor({
  path,
  saved,
  version,
  language,
  active,
  label,
  readOnly = false,
  onDirty,
  onSave,
  register,
}: {
  path: string;
  saved: string;
  version: number;
  language: Language;
  /** Focus the editor when it becomes active. */
  active: boolean;
  /** The editor's accessible name. */
  label: string;
  readOnly?: boolean;
  onDirty: (path: string, dirty: boolean) => void;
  onSave: (path: string) => void;
  register: (path: string, handle: EditorHandle | null) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const baseline = useRef<Text | null>(null);
  const reported = useRef(false);
  const callbacks = useRef({ onDirty, onSave });
  callbacks.current = { onDirty, onSave };
  const languageSlot = useMemo(() => new Compartment(), []);
  const editable = useMemo(() => new Compartment(), []);
  const report = (state: EditorState) => {
    const dirty = !!baseline.current && !state.doc.eq(baseline.current);
    if (dirty !== reported.current) {
      reported.current = dirty;
      callbacks.current.onDirty(path, dirty);
    }
  };
  const createState = (content: string) => {
    const state = EditorState.create({
      doc: content,
      extensions: [
        separator(content),
        history(),
        common(languageSlot),
        editable.of(EditorState.readOnly.of(readOnly)),
        Prec.high(
          keymap.of([
            {
              key: "Mod-s",
              preventDefault: true,
              run: () => {
                callbacks.current.onSave(path);
                return true;
              },
            },
          ]),
        ),
        keymap.of([...historyKeymap, indentWithTab]),
        EditorView.contentAttributes.of({ "aria-label": label }),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) report(update.state);
        }),
      ],
    });
    baseline.current = state.doc;
    return state;
  };
  useEffect(() => {
    const editor = new EditorView({
      state: createState(saved),
      parent: host.current!,
    });
    view.current = editor;
    register(path, {
      content: () => editor.state.doc.toString(),
      focus: () => editor.focus(),
    });
    return () => {
      register(path, null);
      editor.destroy();
      view.current = null;
    };
  }, []);
  // The grammar loads on demand, into the current state (after a reload
  // too: the compartment's content is kept below).
  useEffect(() => {
    let alive = true;
    loadLanguage(language)
      .then((extension) => {
        if (alive && view.current)
          view.current.dispatch({
            effects: languageSlot.reconfigure(extension),
          });
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [language]);
  // A new version replaces the buffer (reload from disk) and resets history.
  const firstVersion = useRef(version);
  useEffect(() => {
    if (!view.current || version === firstVersion.current) return;
    firstVersion.current = version;
    const languageNow = languageSlot.get(view.current.state) || [];
    view.current.setState(createState(saved));
    view.current.dispatch({ effects: languageSlot.reconfigure(languageNow) });
    reported.current = false;
  }, [version]);
  // After a save the saved text becomes the clean baseline.
  useEffect(() => {
    const editor = view.current;
    if (!editor) return;
    baseline.current = editor.state.toText(saved);
    report(editor.state);
  }, [saved]);
  useEffect(() => {
    view.current?.dispatch({
      effects: editable.reconfigure(EditorState.readOnly.of(readOnly)),
    });
  }, [readOnly]);
  useEffect(() => {
    if (active && view.current) {
      view.current.requestMeasure();
      view.current.focus();
    }
  }, [active]);
  return <div className="code-editor" ref={host} data-path={path} />;
}
