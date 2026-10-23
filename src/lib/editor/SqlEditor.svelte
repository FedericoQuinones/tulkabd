<script lang="ts">
  import { onMount } from "svelte";
  import { EditorState, Transaction } from "@codemirror/state";
  import { EditorView, drawSelection, keymap, lineNumbers } from "@codemirror/view";
  import { defaultKeymap, history, historyField, historyKeymap } from "@codemirror/commands";
  import { HighlightStyle, bracketMatching, indentOnInput, syntaxHighlighting } from "@codemirror/language";
  import { MSSQL, sql } from "@codemirror/lang-sql";
  import { autocompletion, type CompletionContext, type CompletionSource } from "@codemirror/autocomplete";
  import { tags } from "@lezer/highlight";
  import { t } from "../i18n/t";
  import type { PaneState } from "../pane/pane.svelte";
  import { runTarget } from "./runTarget";
  import { completionSource, sharedMetadata } from "./completion";
  import type { CompletionMetadata } from "../ipc";

  interface Props {
    /** Supplies the starting text and keeps the editor state across tab switches. */
    pane: PaneState;
    /** Ctrl/Cmd+Enter: receives the selection (or whole doc) to execute. */
    onrun: (target: { sql: string; lineOffset: number }) => void;
    oncursor: (pos: { line: number; col: number }) => void;
    /** Takes the keyboard focus when mounted. */
    autofocus?: boolean;
  }
  let { pane, onrun, oncursor, autofocus = true }: Props = $props();

  let host: HTMLDivElement;
  let view: EditorView | undefined;

  const theme = EditorView.theme(
    {
      "&": { height: "100%", color: "#ffffff", backgroundColor: "transparent", fontSize: "13px" },
      "&.cm-focused": { outline: "none" },
      ".cm-scroller": { fontFamily: "var(--mono)", lineHeight: "22px", overflow: "auto" },
      ".cm-content": { padding: "12px 0", caretColor: "#ffffff" },
      ".cm-line": { padding: "0 14px 0 0" },
      ".cm-gutters": { backgroundColor: "transparent", color: "#55565b", border: "none" },
      ".cm-lineNumbers .cm-gutterElement": { width: "44px", padding: "0 14px 0 0", minWidth: "44px" },
      ".cm-cursor": { borderLeftColor: "#ffffff", borderLeftWidth: "2px" },
      "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": { backgroundColor: "#2c2b30" },
      "&.cm-focused .cm-selectionBackground": { backgroundColor: "#2f4a6e" },
      "&.cm-focused .cm-matchingBracket": { backgroundColor: "#2c2b30", outline: "none" },
      ".cm-tooltip": { backgroundColor: "var(--raised)", border: "1px solid var(--border)", color: "var(--soft)" },
      ".cm-tooltip.cm-tooltip-autocomplete > ul": { fontFamily: "var(--mono)", maxHeight: "16em" },
      ".cm-tooltip-autocomplete ul li[aria-selected]": { backgroundColor: "#2f4a6e", color: "var(--text)" },
      ".cm-completionDetail": { color: "var(--muted)", fontStyle: "normal", marginLeft: "1em" },
      ".cm-completionMatchedText": { textDecoration: "none", color: "var(--accent)" },
    },
    { dark: true },
  );

  const highlight = HighlightStyle.define([
    { tag: [tags.keyword, tags.standard(tags.name), tags.typeName], color: "#7fa7dc" },
    { tag: [tags.string, tags.special(tags.string)], color: "#c9a86a" },
    { tag: [tags.lineComment, tags.blockComment], color: "var(--muted)" },
    { tag: [tags.number, tags.bool, tags.null], color: "#ffffff" },
  ]);

  function report(v: EditorView) {
    const head = v.state.selection.main.head;
    const line = v.state.doc.lineAt(head);
    oncursor({ line: line.number, col: head - line.from + 1 });
  }

  /** Selection (or whole doc) that Run would execute. */
  export function target(): { sql: string; lineOffset: number } {
    const s = view!.state;
    const { from, to } = s.selection.main;
    return runTarget(s.doc.toString(), from, to);
  }

  /** Moves the caret to a 1-based editor line (used by message clicks in Task 10). */
  export function jumpToLine(line: number) {
    if (!view) return;
    const l = view.state.doc.line(Math.min(Math.max(line, 1), view.state.doc.lines));
    view.dispatch({ selection: { anchor: l.from }, scrollIntoView: true });
    view.focus();
  }

  // Completion metadata of the tab's database: fetched when the connection or database changes,
  // then read from the cache (dropped when that database is re-indexed).
  $effect(() => {
    const c = pane.connectionId;
    const d = pane.database;
    if (c && d) sharedMetadata().get(c, d).catch(() => {});
  });
  let built: { meta: CompletionMetadata; source: CompletionSource } | null = null;
  async function complete(ctx: CompletionContext) {
    const c = pane.connectionId;
    const d = pane.database;
    if (!c || !d) return null;
    let meta: CompletionMetadata;
    try {
      meta = await sharedMetadata().get(c, d);
    } catch {
      return null;
    }
    if (built?.meta !== meta) built = { meta, source: completionSource(meta) };
    return built.source(ctx);
  }

  let shownVersion = -1;
  // `pane.loadText` swapped the document while the editor is mounted: show it.
  $effect(() => {
    const v = pane.docVersion;
    if (!view || v === shownVersion) return;
    shownVersion = v;
    // Not undoable: Ctrl+Z must not empty a freshly loaded definition.
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: pane.initialDoc },
      selection: { anchor: 0 },
      annotations: Transaction.addToHistory.of(false),
    });
    pane.editorState = null;
  });

  onMount(() => {
    const extensions = [
          lineNumbers(),
          history(),
          drawSelection(),
          indentOnInput(),
          bracketMatching(),
          sql({ dialect: MSSQL }),
          autocompletion({ override: [complete] }),
          syntaxHighlighting(highlight),
          theme,
          EditorView.contentAttributes.of({ "aria-label": t("pane.editorLabel") }),
          keymap.of([
            {
              key: "Mod-Enter",
              preventDefault: true,
              run: () => {
                onrun(target());
                return true;
              },
            },
            // Ctrl+Shift+\ is the app's "split down", not "go to matching bracket".
            ...defaultKeymap.filter((b) => b.key !== "Shift-Mod-\\"),
            ...historyKeymap,
          ]),
          EditorView.updateListener.of((u) => {
            if (u.selectionSet || u.docChanged) report(u.view);
          }),
    ];
    // Restore from the JSON saved on unmount: it carries text, selection and undo history, while
    // the extensions (and their callbacks) are always the fresh ones from this instance.
    const saved = pane.editorState as Record<string, unknown> | null;
    let state: EditorState;
    try {
      state = saved
        ? EditorState.fromJSON(saved, { extensions }, { history: historyField })
        : EditorState.create({ doc: pane.initialDoc, extensions });
    } catch {
      state = EditorState.create({ doc: pane.initialDoc, extensions });
    }
    view = new EditorView({ parent: host, state });
    if (autofocus) view.focus();
    report(view);
    pane.liveDoc = () => view?.state.doc.toString() ?? "";
    shownVersion = pane.docVersion;
    return () => {
      pane.liveDoc = null;
      if (view) pane.editorState = view.state.toJSON({ history: historyField });
      view?.destroy();
    };
  });
</script>

<div class="editor" bind:this={host}></div>

<style>
  .editor {
    flex: 1;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }
</style>
