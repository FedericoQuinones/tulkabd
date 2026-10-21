<script lang="ts">
  import { onMount } from "svelte";
  import { EditorState } from "@codemirror/state";
  import { EditorView, drawSelection, keymap, lineNumbers } from "@codemirror/view";
  import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
  import { HighlightStyle, bracketMatching, indentOnInput, syntaxHighlighting } from "@codemirror/language";
  import { MSSQL, sql } from "@codemirror/lang-sql";
  import { tags } from "@lezer/highlight";
  import { t } from "../i18n/t";
  import { runTarget } from "./runTarget";

  interface Props {
    /** Ctrl/Cmd+Enter: receives the selection (or whole doc) to execute. */
    onrun: (target: { sql: string; lineOffset: number }) => void;
    oncursor: (pos: { line: number; col: number }) => void;
  }
  let { onrun, oncursor }: Props = $props();

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

  onMount(() => {
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: "",
        extensions: [
          lineNumbers(),
          history(),
          drawSelection(),
          indentOnInput(),
          bracketMatching(),
          sql({ dialect: MSSQL }),
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
            ...defaultKeymap,
            ...historyKeymap,
          ]),
          EditorView.updateListener.of((u) => {
            if (u.selectionSet || u.docChanged) report(u.view);
          }),
        ],
      }),
    });
    view.focus();
    return () => view?.destroy();
  });
</script>

<div class="editor" bind:this={host}></div>

<style>
  .editor {
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }
</style>
