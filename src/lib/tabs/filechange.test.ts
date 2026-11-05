import { beforeEach, expect, test, vi } from "vitest";
import type { OpenedFile } from "../ipc";

vi.mock("../ipc", () => ({
  closeSession: vi.fn(),
  listConnections: vi.fn(),
  startupError: vi.fn(),
  readSqlFile: vi.fn(),
  writeSqlFile: vi.fn(),
  sqlFileModified: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: async () => () => {}, destroy: async () => {} }),
}));

import * as ipc from "../ipc";
import { connections } from "../connections/store.svelte";
import { t } from "../i18n/t";
import { tabs } from "./tabs.svelte";

const opened = (text: string, modifiedMs: number): OpenedFile => ({
  path: "/a.sql",
  name: "a.sql",
  text,
  encoding: "utf8",
  lineEnding: "lf",
  modifiedMs,
});

/** A file tab on /a.sql as read at time 100, with `text` typed over it when `edit` is set. */
async function fileTab(edit?: string) {
  vi.mocked(ipc.readSqlFile).mockResolvedValue(opened("SELECT 1", 100));
  await tabs.openFiles(["/a.sql"]);
  const tab = tabs.active!;
  if (edit !== undefined) {
    tab.pane!.initialDoc = edit;
    tab.pane!.edited();
  }
  return tab;
}

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.closeSession).mockResolvedValue();
  vi.mocked(ipc.writeSqlFile).mockResolvedValue(200);
  connections.list = [];
  await tabs.reset();
});

test("a file tab remembers the modified time it read, and the one it wrote", async () => {
  const tab = await fileTab("SELECT 2");
  expect(tab.file?.modifiedMs).toBe(100);
  vi.mocked(ipc.sqlFileModified).mockResolvedValue(100);
  await tabs.saveActive();
  expect(ipc.writeSqlFile).toHaveBeenCalledTimes(1);
  expect(tab.file?.modifiedMs).toBe(200);
  expect(tab.dirty).toBe(false);
});

test("saving a file another program changed asks first and writes nothing", async () => {
  const tab = await fileTab("SELECT 2");
  vi.mocked(ipc.sqlFileModified).mockResolvedValue(150);
  await tabs.saveActive();
  expect(ipc.writeSqlFile).not.toHaveBeenCalled();
  expect(tabs.fileConflict?.tabId).toBe(tab.id);
  expect(tab.dirty).toBe(true);
});

test("Overwrite writes the tab's text anyway", async () => {
  const tab = await fileTab("SELECT 2");
  vi.mocked(ipc.sqlFileModified).mockResolvedValue(150);
  await tabs.saveActive();
  await tabs.answerFileConflict("overwrite");
  expect(ipc.writeSqlFile).toHaveBeenCalledWith("/a.sql", "SELECT 2", "utf8", "lf");
  expect(tabs.fileConflict).toBeNull();
  expect(tab.dirty).toBe(false);
  expect(tab.file?.modifiedMs).toBe(200);
});

test("Reload shows the file's text and drops the tab's edits", async () => {
  const tab = await fileTab("SELECT 2");
  vi.mocked(ipc.sqlFileModified).mockResolvedValue(150);
  await tabs.saveActive();
  vi.mocked(ipc.readSqlFile).mockResolvedValue(opened("SELECT 'disk'", 150));
  await tabs.answerFileConflict("reload");
  expect(ipc.writeSqlFile).not.toHaveBeenCalled();
  expect(tab.pane!.text()).toBe("SELECT 'disk'");
  expect(tab.dirty).toBe(false);
  expect(tab.file?.modifiedMs).toBe(150);
});

test("Cancel leaves the tab and the file alone", async () => {
  const tab = await fileTab("SELECT 2");
  vi.mocked(ipc.sqlFileModified).mockResolvedValue(150);
  await tabs.saveActive();
  await tabs.answerFileConflict("cancel");
  expect(tabs.fileConflict).toBeNull();
  expect(ipc.writeSqlFile).not.toHaveBeenCalled();
  expect(tab.dirty).toBe(true);
  expect(tab.pane!.text()).toBe("SELECT 2");
});

test("a file that cannot be checked is written as before", async () => {
  await fileTab("SELECT 2");
  vi.mocked(ipc.sqlFileModified).mockRejectedValue("not_found");
  await tabs.saveActive();
  expect(ipc.writeSqlFile).toHaveBeenCalledTimes(1);
  expect(tabs.fileConflict).toBeNull();
});

test("on focus a clean tab whose file changed is reloaded quietly", async () => {
  const tab = await fileTab();
  vi.mocked(ipc.sqlFileModified).mockResolvedValue(300);
  vi.mocked(ipc.readSqlFile).mockResolvedValue(opened("SELECT 'new'", 300));
  await tabs.checkFilesOnDisk();
  expect(tab.pane!.text()).toBe("SELECT 'new'");
  expect(tab.file?.modifiedMs).toBe(300);
  expect(tab.dirty).toBe(false);
  expect(tab.pane!.notice).toBeNull();
});

test("on focus a clean tab whose file did not change is left alone", async () => {
  await fileTab();
  vi.mocked(ipc.sqlFileModified).mockResolvedValue(100);
  vi.mocked(ipc.readSqlFile).mockClear();
  await tabs.checkFilesOnDisk();
  expect(ipc.readSqlFile).not.toHaveBeenCalled();
});

test("on focus an unsaved tab whose file changed keeps its text and shows a notice", async () => {
  const tab = await fileTab("SELECT 2");
  vi.mocked(ipc.sqlFileModified).mockResolvedValue(300);
  vi.mocked(ipc.readSqlFile).mockClear();
  await tabs.checkFilesOnDisk();
  expect(ipc.readSqlFile).not.toHaveBeenCalled();
  expect(tab.pane!.text()).toBe("SELECT 2");
  expect(tab.dirty).toBe(true);
  expect(tab.pane!.notice).toBe(t("files.changedOnDisk"));
});

test("closing with Save over a changed file keeps the tab and says why", async () => {
  const tab = await fileTab("SELECT 2");
  vi.mocked(ipc.sqlFileModified).mockResolvedValue(150);
  await tabs.requestClose(tab.id);
  await tabs.answerUnsaved(true);
  expect(ipc.writeSqlFile).not.toHaveBeenCalled();
  expect(tabs.list.some((x) => x.id === tab.id)).toBe(true);
  expect(tabs.unsavedPrompt?.error).toBeTruthy();
});
