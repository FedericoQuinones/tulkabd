import { t } from "../i18n/t";

/** CodeMirror's search panel strings (keyed by its English phrase), taken from the i18n catalog. */
export function searchPhrases(): Record<string, string> {
  return {
    Find: t("editor.search.find"),
    Replace: t("editor.search.replace"),
    next: t("editor.search.next"),
    previous: t("editor.search.previous"),
    all: t("editor.search.all"),
    "match case": t("editor.search.matchCase"),
    regexp: t("editor.search.regexp"),
    "by word": t("editor.search.byWord"),
    replace: t("editor.search.replaceBtn"),
    "replace all": t("editor.search.replaceAll"),
    close: t("editor.search.close"),
    "Go to line": t("editor.search.goToLine"),
    go: t("editor.search.go"),
  };
}
