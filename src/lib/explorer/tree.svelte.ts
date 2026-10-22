// Explorer tree store: the lazily loaded metadata below the folder/connection
// hierarchy, connection state, the filter and folder commands. The components
// only render `explorer.rows` and forward user actions here; the pure helpers
// live in tree.ts.
import { SvelteSet } from "svelte/reactivity";
import { t } from "../i18n/t";
import * as ipc from "../ipc";
import type { AppEvent, CachedDatabase, ColumnInfo, ConnectionConfig, DbObject, Folder, ObjectKind, PaletteHit } from "../ipc";
import { connections } from "../connections/store.svelte";
import {
  actionError,
  buildMenu,
  buildTree,
  canMoveFolder,
  keyOf,
  OBJECT_KINDS,
  parseKey,
  quoteName,
  type ConnStatus,
  type DragItem,
  type MenuActions,
  type MenuItem,
  type NoticeAction,
  type Row,
  type TreeNode,
} from "./tree";

export interface ConnRuntime {
  status: ConnStatus;
  /** The inline error of the `error` status. */
  message: string | null;
  /** Cached databases; `null` until first read. */
  databases: CachedDatabase[] | null;
  /** Databases being indexed right now. */
  indexing: string[];
}


type Loaded<T> = T[] | { error: string };

interface FilterSets {
  /** Nodes whose name matches; their subtree shows as usual. */
  match: Set<string>;
  /** Ancestors of matches and search hits; shown open. */
  forced: Set<string>;
  /** Search hits by group key. */
  hits: Map<string, DbObject[]>;
}

class ExplorerStore {
  folders = $state<Folder[]>([]);
  readonly expanded = new SvelteSet<string>();
  conns = $state<Record<string, ConnRuntime>>({});
  /** Objects by group key; raw so 500-object lists are not proxied. */
  objects = $state.raw<Record<string, Loaded<DbObject>>>({});
  /** Columns by object key. */
  columns = $state.raw<Record<string, Loaded<ColumnInfo>>>({});
  filter = $state("");
  hits = $state.raw<PaletteHit[]>([]);
  focusKey = $state<string | null>(null);
  /** The last failed tree action (folder commands, copy); shown inline above the tree. */
  error = $state<string | null>(null);
  /** Short-lived confirmation, e.g. after Copy name. */
  toast = $state<string | null>(null);
  passwordRequest = $state<{ conn: ConnectionConfig } | null>(null);
  persistDelayMs = 400;

  readonly rows: Row[] = $derived.by(() => this.flatten());

  private gen = 0;
  private searchToken = 0;
  private attempts = new Map<string, number>();
  private dbLoads = new Map<string, { again: boolean; done: Promise<void> }>();
  private objLoads = new Map<string, Promise<void>>();
  private colLoads = new Map<string, Promise<void>>();
  private persistTimer: ReturnType<typeof setTimeout> | undefined;
  private toastTimer: ReturnType<typeof setTimeout> | undefined;
  private subscribed = false;

  /** Runtime state of a connection, created on first use. Not for use while deriving rows. */
  state(id: string): ConnRuntime {
    this.conns[id] ??= { status: "offline", message: null, databases: null, indexing: [] };
    return this.conns[id];
  }

  /**
   * Loads folders and the expanded set. Connections left expanded are shown
   * from the cache and are NOT connected: that would mean a burst of logins and
   * password prompts at every start.
   */
  async init(): Promise<void> {
    if (!this.subscribed) {
      this.subscribed = true;
      ipc.onAppEvent((e) => this.handleEvent(e)).catch(() => {});
    }
    const [folders, expanded] = await Promise.all([
      ipc.listFolders().catch(() => [] as Folder[]),
      ipc.listExpanded().catch(() => [] as string[]),
      connections.refresh(),
    ]);
    this.folders = folders;
    for (const id of expanded) this.expanded.add(id);
    await Promise.all(
      expanded.filter((id) => connections.list.some((c) => c.id === id)).map((id) => this.loadDatabases(id)),
    );
  }

  handleEvent(e: AppEvent): void {
    const st = this.state(e.connectionId);
    if (e.kind === "explorerStatus") {
      if (e.status === "connecting") {
        st.status = "connecting";
        st.message = null;
      } else if (e.status === "connected") {
        st.status = "connected";
        st.message = null;
        void this.loadDatabases(e.connectionId);
      } else {
        // No indexIdle follows an error or a disconnect.
        st.indexing = [];
        st.status = e.status === "error" ? "error" : "offline";
        st.message = e.status === "error" ? (e.message ?? null) : null;
      }
    } else if (e.kind === "indexProgress") {
      if (!e.done) {
        // Progress can trail an error status; it must not look like indexing then.
        const live = st.status === "connected" || st.status === "connecting";
        if (live && !st.indexing.includes(e.database)) st.indexing.push(e.database);
      } else {
        st.indexing = st.indexing.filter((d) => d !== e.database);
        void this.loadDatabases(e.connectionId);
        this.invalidate(e.connectionId, e.database);
      }
    } else {
      st.indexing = [];
      void this.loadDatabases(e.connectionId);
    }
  }

  // ---- expand / collapse -------------------------------------------------

  async expand(key: string): Promise<void> {
    this.expanded.add(key);
    this.schedulePersist();
    if (connections.list.some((c) => c.id === key)) {
      const cached = this.loadDatabases(key);
      if (this.state(key).status === "offline") await this.connect(key);
      await cached;
      return;
    }
    const k = parseKey(key);
    if (k?.[0] === "g") await this.loadObjects(k[1], k[2], k[3] as ObjectKind);
    else if (k?.[0] === "o") {
      const [, c, d, kind, schema, name] = k;
      await this.loadColumns(c, d, { schema, name, kind: kind as ObjectKind });
    }
  }

  collapse(key: string): void {
    const rows = this.rows;
    const i = rows.findIndex((r) => r.key === key);
    if (i >= 0 && this.focusKey !== key) {
      for (let j = i + 1; j < rows.length && rows[j].level > rows[i].level; j++) {
        if (rows[j].key === this.focusKey) {
          this.focusKey = key;
          break;
        }
      }
    }
    this.expanded.delete(key);
    this.schedulePersist();
  }

  toggle(key: string): void {
    if (this.rows.find((r) => r.key === key)?.expanded) this.collapse(key);
    else void this.expand(key);
  }

  private schedulePersist(): void {
    clearTimeout(this.persistTimer);
    this.persistTimer = setTimeout(() => {
      const ids = [...this.expanded].filter(
        (k) => this.folders.some((f) => f.id === k) || connections.list.some((c) => c.id === k),
      );
      ipc.setExpanded(ids).catch(() => {});
    }, this.persistDelayMs);
  }

  // ---- connecting --------------------------------------------------------

  async connect(id: string, password?: string): Promise<void> {
    const conn = connections.list.find((c) => c.id === id);
    if (!conn) return;
    const st = this.state(id);
    const attempt = (this.attempts.get(id) ?? 0) + 1;
    this.attempts.set(id, attempt);
    const gen = this.gen;
    const current = () => gen === this.gen && this.attempts.get(id) === attempt;
    st.status = "connecting";
    st.message = null;
    try {
      await ipc.connectExplorer(id, password);
      if (!current()) return;
      st.status = "connected";
      await this.loadDatabases(id);
    } catch (e) {
      if (!current()) return;
      const s = String(e);
      if (s === "password_required") {
        st.status = "offline";
        this.passwordRequest = { conn };
      } else if (s === "closed") {
        // A disconnect or a newer connect won; its status event says the rest.
        if (st.status === "connecting") st.status = "offline";
      } else if (s === "not_found") {
        st.status = "offline";
        void connections.refresh();
      } else {
        st.status = "error";
        st.message = s;
      }
    }
  }

  async submitPassword(password: string): Promise<void> {
    const req = this.passwordRequest;
    this.passwordRequest = null;
    if (req) await this.connect(req.conn.id, password);
  }

  cancelPassword(): void {
    const req = this.passwordRequest;
    this.passwordRequest = null;
    if (req) this.state(req.conn.id).status = "offline";
  }

  async disconnect(id: string): Promise<void> {
    this.attempts.set(id, (this.attempts.get(id) ?? 0) + 1);
    const st = this.state(id);
    st.status = "offline";
    st.message = null;
    st.indexing = [];
    try {
      await ipc.disconnectExplorer(id);
    } catch (e) {
      this.error = actionError(e);
    }
  }

  /** Re-indexes the whole server; connects first when there is no session. */
  async refresh(id: string): Promise<void> {
    if (this.state(id).status !== "connected") return this.connect(id);
    try {
      await ipc.refreshMetadata(id);
    } catch (e) {
      if (String(e) === "not_connected") await this.connect(id);
      else this.error = actionError(e);
    }
  }

  async refreshDatabase(id: string, database: string): Promise<void> {
    if (this.state(id).status !== "connected") return this.connect(id);
    try {
      await ipc.refreshMetadata(id, database);
    } catch (e) {
      if (String(e) === "not_connected") await this.connect(id);
      else this.error = actionError(e);
    }
  }

  // ---- lazy loads --------------------------------------------------------

  /** Reads the cached databases; overlapping calls coalesce into one more read. */
  loadDatabases(id: string): Promise<void> {
    const running = this.dbLoads.get(id);
    if (running) {
      running.again = true;
      return running.done;
    }
    const gen = this.gen;
    const entry = { again: false, done: Promise.resolve() };
    this.dbLoads.set(id, entry);
    entry.done = (async () => {
      try {
        do {
          entry.again = false;
          const list = await ipc.explorerDatabases(id);
          if (gen !== this.gen) return;
          this.state(id).databases = list;
        } while (entry.again);
      } catch {
        // The cache read failed: leave what is shown.
      } finally {
        if (this.dbLoads.get(id) === entry) this.dbLoads.delete(id);
      }
    })();
    return entry.done;
  }

  loadObjects(c: string, d: string, kind: ObjectKind): Promise<void> {
    const key = keyOf.group(c, d, kind);
    return this.once(this.objLoads, key, async (gen) => {
      let value: Loaded<DbObject>;
      try {
        value = await ipc.explorerObjects(c, d, kind);
      } catch (e) {
        value = { error: String(e) };
      }
      if (gen === this.gen) this.objects = { ...this.objects, [key]: value };
    });
  }

  loadColumns(c: string, d: string, o: DbObject): Promise<void> {
    const key = keyOf.object(c, d, o);
    return this.once(this.colLoads, key, async (gen) => {
      let value: Loaded<ColumnInfo>;
      try {
        value = await ipc.explorerColumns(c, d, o.schema, o.name);
      } catch (e) {
        value = { error: String(e) };
      }
      if (gen === this.gen) this.columns = { ...this.columns, [key]: value };
    });
  }

  private once(map: Map<string, Promise<void>>, key: string, run: (gen: number) => Promise<void>): Promise<void> {
    const running = map.get(key);
    if (running) return running;
    const p = run(this.gen).finally(() => map.delete(key));
    map.set(key, p);
    return p;
  }

  /** A database was re-indexed: reload what is open below it and drop the rest. */
  private invalidate(c: string, d: string): void {
    const sameDb = (key: string) => {
      const k = parseKey(key);
      if (k?.[0] === "g" || k?.[0] === "o") return k[1] === c && k[2] === d;
      return false;
    };
    const objects = { ...this.objects };
    for (const key of Object.keys(objects)) {
      if (!sameDb(key)) continue;
      if (this.expanded.has(key)) void this.loadObjects(c, d, parseKey(key)![3] as ObjectKind);
      else delete objects[key];
    }
    this.objects = objects;
    const columns = { ...this.columns };
    for (const key of Object.keys(columns)) {
      if (!sameDb(key)) continue;
      const [, , , kind, schema, name] = parseKey(key)!;
      if (this.expanded.has(key)) void this.loadColumns(c, d, { schema, name, kind: kind as ObjectKind });
      else delete columns[key];
    }
    this.columns = columns;
  }

  // ---- filter ------------------------------------------------------------

  /**
   * Filters folders, connections and loaded databases by name; with 2 or more
   * characters it also shows the cached objects `search_objects` finds.
   */
  async setFilter(query: string): Promise<void> {
    this.filter = query;
    const token = ++this.searchToken;
    const q = query.trim();
    if (q.length < 2) {
      this.hits = [];
      return;
    }
    try {
      const res = await ipc.searchObjects(q, 200);
      if (token === this.searchToken) this.hits = res.hits;
    } catch {
      if (token === this.searchToken) this.hits = [];
    }
  }

  private filterSets(): FilterSets | null {
    const q = this.filter.trim().toLowerCase();
    if (q === "") return null;
    const folders = this.folders;
    const parentOf = new Map(folders.map((f) => [f.id, f.parentId]));
    const match = new Set<string>();
    const forced = new Set<string>();
    const hits = new Map<string, DbObject[]>();
    const openChain = (folderId: string | null) => {
      for (let cur = folderId, steps = 0; cur && steps <= folders.length; steps++) {
        forced.add(cur);
        cur = parentOf.get(cur) ?? null;
      }
    };
    for (const f of folders) {
      if (f.name.toLowerCase().includes(q)) {
        match.add(f.id);
        openChain(f.parentId);
      }
    }
    const known = new Map(connections.list.map((c) => [c.id, c]));
    for (const c of connections.list) {
      if (c.name.toLowerCase().includes(q)) {
        match.add(c.id);
        openChain(c.folderId);
      }
      for (const d of this.conns[c.id]?.databases ?? []) {
        if (d.name.toLowerCase().includes(q)) {
          match.add(keyOf.db(c.id, d.name));
          forced.add(c.id);
          openChain(c.folderId);
        }
      }
    }
    for (const h of this.hits) {
      const c = known.get(h.connectionId);
      if (!c) continue;
      openChain(c.folderId);
      forced.add(c.id);
      forced.add(keyOf.db(c.id, h.database));
      const gk = keyOf.group(c.id, h.database, h.object.kind);
      forced.add(gk);
      let list = hits.get(gk);
      if (!list) hits.set(gk, (list = []));
      list.push(h.object);
    }
    return { match, forced, hits };
  }

  // ---- rows --------------------------------------------------------------

  private flatten(): Row[] {
    const tree = buildTree(this.folders, connections.list, this.expanded);
    const fs = this.filterSets();
    const out: Row[] = [];
    const ctx = { fs, out };
    for (const node of tree) this.walk(ctx, node, 1, null, fs === null);
    return out;
  }

  private visible(fs: FilterSets | null, key: string, inMatch: boolean): boolean {
    return !fs || inMatch || fs.match.has(key) || fs.forced.has(key);
  }

  private open(fs: FilterSets | null, key: string): boolean {
    return this.expanded.has(key) || (fs?.forced.has(key) ?? false);
  }

  private walk(ctx: { fs: FilterSets | null; out: Row[] }, node: TreeNode, level: number, parent: string | null, inMatch: boolean) {
    const { fs, out } = ctx;
    const key = node.id;
    if (!this.visible(fs, key, inMatch)) return;
    const childIn = inMatch || (fs?.match.has(key) ?? false);
    const open = this.open(fs, key);
    const base = { key, level, parent, expandable: true, expanded: open };
    if (node.kind === "folder") {
      out.push({ ...base, kind: "folder", folder: node.folder });
      if (open) for (const child of node.children) this.walk(ctx, child, level + 1, key, childIn);
      return;
    }
    const st = this.conns[key];
    const status = st?.status ?? "offline";
    out.push({ ...base, kind: "connection", conn: node.connection, status });
    if (open) this.connectionChildren(ctx, node.connection, st, level + 1, childIn);
  }

  private notice(parent: string, tag: string, level: number, tone: "error" | "muted", text: string, action: NoticeAction | null = null): Row {
    return { key: keyOf.notice(parent, tag), level, parent, expandable: false, expanded: false, kind: "notice", tone, text, action };
  }

  private later(load: () => Promise<void>) {
    // Rows are derived: loads start after the derivation, never inside it.
    queueMicrotask(() => void load());
  }

  private connectionChildren(
    ctx: { fs: FilterSets | null; out: Row[] },
    conn: ConnectionConfig,
    st: ConnRuntime | undefined,
    level: number,
    inMatch: boolean,
  ) {
    const { fs, out } = ctx;
    const id = conn.id;
    const status = st?.status ?? "offline";
    let dbs = st?.databases ?? null;
    if (dbs === null && !this.dbLoads.has(id)) this.later(() => this.loadDatabases(id));
    if (inMatch) {
      const connectAction = { label: t("explorer.connect"), run: () => this.connect(id) };
      if (status === "error") {
        out.push(this.notice(id, "status", level, "error", st?.message ?? t("explorer.connectError"), {
          label: t("explorer.retry"),
          run: () => this.connect(id),
        }));
      } else if (status === "offline") {
        const text = dbs && dbs.length > 0 ? t("explorer.offlineCached") : t("explorer.notConnected");
        out.push(this.notice(id, "status", level, "muted", text, connectAction));
      } else if (status === "connected" && dbs && dbs.length === 0) {
        out.push(this.notice(id, "status", level, "muted", t("explorer.noDatabases")));
      }
    }
    dbs ??= [];
    if (fs) {
      // Search hits can name databases not read yet.
      const extra = new Set<string>();
      for (const h of this.hits) {
        if (h.connectionId === id && !dbs.some((d) => d.name === h.database)) extra.add(h.database);
      }
      if (extra.size > 0) {
        dbs = [
          ...dbs,
          ...[...extra].map((name) => ({ name, noAccess: false, indexedAt: null, tables: 0, views: 0, procedures: 0 })),
        ];
      }
    }
    const live = status === "connected" || status === "connecting";
    for (const db of dbs) {
      const key = keyOf.db(id, db.name);
      if (!this.visible(fs, key, inMatch)) continue;
      const childIn = inMatch || (fs?.match.has(key) ?? false);
      const open = !db.noAccess && this.open(fs, key);
      const indexing = st?.indexing.includes(db.name) || (db.indexedAt === null && !db.noAccess && live);
      out.push({
        key,
        level,
        parent: id,
        expandable: !db.noAccess,
        expanded: open,
        kind: "database",
        connectionId: id,
        db,
        indexing: !!indexing,
      });
      if (open) this.groups(ctx, id, db, level + 1, childIn);
    }
  }

  private groups(ctx: { fs: FilterSets | null; out: Row[] }, c: string, db: CachedDatabase, level: number, inMatch: boolean) {
    const { fs, out } = ctx;
    const parent = keyOf.db(c, db.name);
    const counts: Record<ObjectKind, number> = { table: db.tables, view: db.views, procedure: db.procedures };
    for (const kind of OBJECT_KINDS) {
      const key = keyOf.group(c, db.name, kind);
      if (!this.visible(fs, key, inMatch)) continue;
      const open = this.open(fs, key);
      out.push({
        key,
        level,
        parent,
        expandable: true,
        expanded: open,
        kind: "group",
        connectionId: c,
        database: db.name,
        objectKind: kind,
        count: db.indexedAt === null ? null : counts[kind],
      });
      if (!open) continue;
      let objects: DbObject[];
      if (inMatch && this.expanded.has(key)) {
        const loaded = this.objects[key];
        if (loaded === undefined) {
          if (!this.objLoads.has(key)) this.later(() => this.loadObjects(c, db.name, kind));
          out.push(this.notice(key, "loading", level + 1, "muted", t("explorer.loading")));
          continue;
        }
        if (!Array.isArray(loaded)) {
          out.push(this.notice(key, "error", level + 1, "error", loaded.error, {
            label: t("explorer.retry"),
            run: () => this.reloadObjects(c, db.name, kind),
          }));
          continue;
        }
        if (loaded.length === 0) out.push(this.notice(key, "empty", level + 1, "muted", t("explorer.none")));
        objects = loaded;
      } else {
        objects = fs?.hits.get(key) ?? [];
      }
      for (const o of objects) this.objectRow(out, c, db.name, o, level + 1, key);
    }
  }

  private reloadObjects(c: string, d: string, kind: ObjectKind): Promise<void> {
    const { [keyOf.group(c, d, kind)]: _, ...rest } = this.objects;
    this.objects = rest;
    return this.loadObjects(c, d, kind);
  }

  private reloadColumns(c: string, d: string, o: DbObject): Promise<void> {
    const { [keyOf.object(c, d, o)]: _, ...rest } = this.columns;
    this.columns = rest;
    return this.loadColumns(c, d, o);
  }

  private objectRow(out: Row[], c: string, d: string, o: DbObject, level: number, parent: string) {
    const key = keyOf.object(c, d, o);
    const expandable = o.kind !== "procedure";
    const open = expandable && this.expanded.has(key);
    out.push({ key, level, parent, expandable, expanded: open, kind: "object", connectionId: c, database: d, object: o });
    if (!open) return;
    const cols = this.columns[key];
    if (cols === undefined) {
      if (!this.colLoads.has(key)) this.later(() => this.loadColumns(c, d, o));
      out.push(this.notice(key, "loading", level + 1, "muted", t("explorer.loading")));
    } else if (!Array.isArray(cols)) {
      out.push(this.notice(key, "error", level + 1, "error", cols.error, {
        label: t("explorer.retry"),
        run: () => this.reloadColumns(c, d, o),
      }));
    } else if (cols.length === 0) {
      out.push(this.notice(key, "empty", level + 1, "muted", t("explorer.none")));
    } else {
      for (const column of cols) {
        out.push({
          key: keyOf.column(key, column.name),
          level: level + 1,
          parent: key,
          expandable: false,
          expanded: false,
          kind: "column",
          column,
        });
      }
    }
  }

  // ---- folder and connection commands -------------------------------------

  private async reloadFolders(): Promise<void> {
    this.folders = await ipc.listFolders();
  }

  private async attempt(run: () => Promise<unknown>): Promise<boolean> {
    try {
      await run();
      this.error = null;
      return true;
    } catch (e) {
      this.error = actionError(e);
      return false;
    }
  }

  async createFolder(name: string, parentId: string | null): Promise<boolean> {
    return this.attempt(async () => {
      await ipc.createFolder(name, parentId);
      await this.reloadFolders();
      if (parentId) {
        this.expanded.add(parentId);
        this.schedulePersist();
      }
    });
  }

  async renameFolder(id: string, name: string): Promise<boolean> {
    return this.attempt(async () => {
      await ipc.renameFolder(id, name);
      await this.reloadFolders();
    });
  }

  /** Its subfolders and connections move up one level (done by the backend). */
  async deleteFolder(id: string): Promise<boolean> {
    return this.attempt(async () => {
      await ipc.deleteFolder(id);
      this.expanded.delete(id);
      await Promise.all([this.reloadFolders(), connections.refresh()]);
    });
  }

  async renameConnection(conn: ConnectionConfig, name: string): Promise<boolean> {
    return this.attempt(() => connections.save({ ...conn, name }));
  }

  async deleteConnection(id: string): Promise<boolean> {
    return this.attempt(async () => {
      await connections.remove(id);
      this.expanded.delete(id);
      delete this.conns[id];
      this.schedulePersist();
    });
  }

  /** Drag and drop or Move to folder…; a folder never goes inside itself. */
  async move(item: DragItem, target: string | null): Promise<void> {
    if (item.kind === "folder") {
      const f = this.folders.find((x) => x.id === item.id);
      if (!f || (f.parentId ?? null) === target || !canMoveFolder(this.folders, item.id, target)) return;
      if (!(await this.attempt(() => ipc.moveFolder(item.id, target)))) return;
      await this.reloadFolders().catch(() => {});
    } else {
      const c = connections.list.find((x) => x.id === item.id);
      if (!c || (c.folderId ?? null) === target) return;
      if (!(await this.attempt(() => ipc.moveConnection(item.id, target)))) return;
      await connections.refresh();
    }
    if (target) {
      this.expanded.add(target);
      this.schedulePersist();
    }
  }

  // ---- clipboard ---------------------------------------------------------

  /**
   * Copies `[schema].[name]`. Uses the web clipboard: a write started by a user
   * gesture needs no Tauri permission (the Rust clipboard is only wired for results).
   */
  async copyName(o: DbObject): Promise<void> {
    const text = quoteName(o.schema, o.name);
    try {
      await navigator.clipboard.writeText(text);
      this.showToast(t("explorer.copied", { name: text }));
    } catch (e) {
      this.error = t("explorer.copyFailed", { error: String(e) });
    }
  }

  private showToast(text: string): void {
    this.toast = text;
    clearTimeout(this.toastTimer);
    this.toastTimer = setTimeout(() => (this.toast = null), 2500);
  }

  /** Clears everything (used by tests). */
  reset(): void {
    this.gen++;
    this.searchToken++;
    clearTimeout(this.persistTimer);
    clearTimeout(this.toastTimer);
    this.folders = [];
    this.expanded.clear();
    this.conns = {};
    this.objects = {};
    this.columns = {};
    this.filter = "";
    this.hits = [];
    this.focusKey = null;
    this.error = null;
    this.toast = null;
    this.passwordRequest = null;
    this.attempts.clear();
    this.dbLoads.clear();
    this.objLoads.clear();
    this.colLoads.clear();
  }
}

export const explorer = new ExplorerStore();

/** The context menu of a row, wired to the explorer store. */
export const menuFor = (row: Row, ui: MenuActions): MenuItem[] => buildMenu(row, ui, explorer);
