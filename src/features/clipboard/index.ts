// Clipboard feature. Called once, the first time the Clipboard tab is shown.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { icon, type IconName } from "../../icons";
import { pngUrl } from "../../png-url";
import "./clipboard.css";

type Kind = "text" | "image" | "files";

interface Item {
  id: number;
  kind: Kind;
  /** Text; for files their names, one per line; empty for images. */
  text: string;
  imagePath?: string | null;
  thumbPath?: string | null;
  width?: number | null;
  height?: number | null;
  /** Files: absolute paths. */
  files: string[];
  /** Files: how many no longer exist. */
  missing: number;
  createdAt: number; // unix seconds
  pinned: boolean;
}

/** Filter chips under the search: "" is All. */
const FILTERS: [Kind | "", string, IconName][] = [
  ["", "All", "grid"],
  ["text", "Text", "text"],
  ["image", "Images", "image"],
  ["files", "Files", "file"],
];
/** A text item that is just one web address gets the link tile. */
const LINK = /^(https?:\/\/|www\.)\S+$/i;

const EMPTY: Record<Kind | "", string> = {
  "": "Copy text, an image or files and it shows up here.",
  text: "Copy some text and it shows up here.",
  image: "Copy an image and it shows up here.",
  files: "Copy files in Explorer or Finder and they show up here.",
};

const KEEP_OPTIONS: [number, string][] = [
  [1, "1 hour"],
  [12, "12 hours"],
  [24, "1 day"],
  [72, "3 days"],
  [168, "1 week"],
  [720, "30 days"],
];

const PIN_ICON =
  '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M6 2h4l-.5 4 2.5 2.5V10H8.6v4h-1.2v-4H4V8.5L6.5 6z" fill="currentColor"/></svg>';
const DELETE_ICON =
  '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>';

const isMac = navigator.userAgent.includes("Mac");
/** How long "Item deleted · Undo" (or "N older items removed · Undo") stays. */
const UNDO_MS = 5000;
/** Pinned only: how long "Copied. Click your app, then Ctrl+V" shows (unpinned, the popover hides at once). */
const COPIED_PINNED_MS = 1500;
/** How long a copy error ("The file is no longer there") shows. */
const ERROR_MS = 2500;

const FOLDER_ICON =
  '<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M2 4.5A1.5 1.5 0 0 1 3.5 3h2.6l1.4 1.5h5A1.5 1.5 0 0 1 14 6v5.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5z" fill="none" stroke="currentColor" stroke-width="1.2"/></svg>';

function fileName(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** "PDF", "XLSX" ... for the tile; "" for a name without one (often a folder). */
function extension(name: string): string {
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || dot === name.length - 1) return "";
  return name.slice(dot + 1).toUpperCase().slice(0, 4);
}

export function mount(el: HTMLElement): void {
  el.innerHTML = `
    <div class="toolbar"><label class="search-field">${icon("search")}<input class="field" type="search" placeholder="Search clipboard" aria-label="Search clipboard" spellcheck="false" /></label></div>
    <div class="cb-chips" role="group" aria-label="Show">${FILTERS.map(
      ([kind, label, ico]) =>
        `<button type="button" class="cb-chip" data-kind="${kind}" aria-pressed="${kind === ""}">${icon(ico, 12)}${label}</button>`,
    ).join("")}</div>
    <ul class="scroll cb-list" role="listbox" aria-label="Clipboard history"></ul>
    <div class="undo-bar" role="status" hidden>
      <span class="cb-undo-text">Item deleted</span>
      <button type="button">Undo</button>
    </div>
    <div class="toast" role="status" hidden>Copied. Click your app, then <kbd>${isMac ? "⌘V" : "Ctrl+V"}</kbd></div>
    <footer class="footer">
      <span>Click to copy · ${isMac ? "⌘P" : "Ctrl+P"} pin</span>
      <label class="cb-keep">Keep <select class="select" aria-label="Keep items for"></select></label>
    </footer>`;
  const search = el.querySelector<HTMLInputElement>("input")!;
  const list = el.querySelector<HTMLUListElement>(".cb-list")!;
  const keep = el.querySelector<HTMLSelectElement>("select")!;
  const undoBar = el.querySelector<HTMLElement>(".undo-bar")!;
  const undoText = el.querySelector<HTMLElement>(".cb-undo-text")!;
  const toast = el.querySelector<HTMLElement>(".toast")!;
  const toastHtml = toast.innerHTML;
  const chips = [...el.querySelectorAll<HTMLButtonElement>(".cb-chip")];

  let items: Item[] = [];
  let selected = 0;
  let keepHours = 24;
  /** A deleted item still in its undo time: hidden from the list, deleted when the timer ends. */
  let pendingId: number | null = null;
  /** What the undo bar offers: commit runs when the 5 s end (or another undoable action starts). */
  let undo: { timer: number; commit: () => Promise<void>; revert: () => Promise<void> } | null = null;
  let copiedTimer: number | undefined;
  let filter: Kind | "" = "";
  /** Thumbnail object URLs by item id, made once per image. */
  const thumbs = new Map<number, Promise<string>>();
  /** The next render follows a search or filter change: the list fades in once. */
  let fadeNext = false;

  function timeLeft(item: Item): string {
    if (item.pinned) return "Pinned";
    const secs = item.createdAt + keepHours * 3600 - Date.now() / 1000;
    const hours = Math.floor(secs / 3600);
    if (hours >= 48) return `${Math.floor(hours / 24)}d`;
    if (hours >= 1) return `${hours}h`;
    return `${Math.max(1, Math.ceil(secs / 60))}m`;
  }

  function render(): void {
    list.replaceChildren();
    if (fadeNext) {
      fadeNext = false;
      list.classList.remove("cb-fade");
      void list.offsetWidth; // restarts the animation
      list.classList.add("cb-fade");
    }
    if (items.length === 0) {
      const empty = document.createElement("li");
      empty.className = "empty";
      empty.textContent = search.value.trim() ? "Nothing matches your search." : EMPTY[filter];
      list.append(empty);
      return;
    }
    items.forEach((item, i) => {
      const row = document.createElement("li");
      row.className = "row cb-row";
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", String(i === selected));
      const content = rowContent(item);
      row.title = content.title;

      const actions = document.createElement("span");
      actions.className = "cb-actions";
      const pin = document.createElement("button");
      pin.type = "button";
      pin.className = "icon-btn cb-icon" + (item.pinned ? " cb-on" : "");
      pin.title = item.pinned ? "Unpin" : `Pin (${isMac ? "Cmd" : "Ctrl"}+P)`;
      pin.innerHTML = PIN_ICON;
      pin.addEventListener("click", (e) => {
        e.stopPropagation();
        void togglePin(item);
      });
      const del = document.createElement("button");
      del.type = "button";
      del.className = "icon-btn cb-icon cb-del";
      del.title = "Delete";
      del.innerHTML = DELETE_ICON;
      del.addEventListener("click", (e) => {
        e.stopPropagation();
        void remove(item);
      });
      actions.append(pin, del);

      const left = document.createElement("span");
      left.className = "cb-left";
      left.textContent = timeLeft(item);

      row.append(...content.nodes, actions, left);
      row.addEventListener("click", () => void copy(item));
      row.addEventListener("mousemove", () => select(i));
      list.append(row);
    });
  }

  /** What a row shows before its actions: text; thumbnail + "Image · W × H"; tile + file name. */
  function rowContent(item: Item): { nodes: HTMLElement[]; title: string } {
    const text = document.createElement("span");
    text.className = "cb-text";
    if (item.kind === "image") {
      const img = document.createElement("img");
      img.className = "cb-thumb";
      img.alt = "";
      void thumbUrl(item.id)
        .then((url) => (img.src = url))
        .catch(() => img.classList.add("cb-broken"));
      const size = item.width && item.height ? ` · ${item.width} × ${item.height}` : "";
      text.textContent = `Image${size}`;
      return { nodes: [img, text], title: text.textContent };
    }
    if (item.kind === "files") {
      const name = fileName(item.files[0] ?? "");
      const ext = extension(name);
      const tile = document.createElement("span");
      tile.className = "cb-ext";
      if (ext) tile.textContent = ext;
      else tile.innerHTML = FOLDER_ICON;
      text.classList.add("cb-file");
      const nameEl = document.createElement("span");
      nameEl.className = "cb-name";
      nameEl.textContent = name;
      text.append(nameEl);
      if (item.files.length > 1) {
        const more = document.createElement("span");
        more.className = "cb-more";
        more.textContent = ` + ${item.files.length - 1} more`;
        text.append(more);
      }
      if (item.missing > 0) {
        const tag = document.createElement("span");
        tag.className = "cb-missing";
        tag.textContent = item.missing === item.files.length ? "missing" : `${item.missing} missing`;
        text.append(tag);
        if (item.missing === item.files.length) tile.classList.add("cb-gone");
      }
      return { nodes: [tile, text], title: item.files.join("\n") };
    }
    // Text and link rows: a tile the size of an image thumbnail, so every row's text lines up.
    const tile = document.createElement("span");
    tile.className = "cb-tile";
    tile.innerHTML = icon(LINK.test(item.text.trim()) ? "link" : "text");
    text.textContent = item.text;
    return { nodes: [tile, text], title: item.text.length > 500 ? item.text.slice(0, 500) + "..." : item.text };
  }

  function thumbUrl(id: number): Promise<string> {
    let url = thumbs.get(id);
    if (!url) {
      url = invoke<ArrayBuffer>("clipboard_thumb", { id }).then(pngUrl);
      url.catch(() => thumbs.delete(id));
      thumbs.set(id, url);
    }
    return url;
  }

  /** Frees the thumbnails of items that are gone (deleted or expired). */
  function dropThumbs(keep: Set<number>): void {
    for (const [id, url] of thumbs) {
      if (keep.has(id)) continue;
      thumbs.delete(id);
      void url.then((u) => URL.revokeObjectURL(u)).catch(() => {});
    }
  }

  function setFilter(kind: Kind | ""): void {
    if (kind === filter) return;
    filter = kind;
    fadeNext = true;
    for (const chip of chips) chip.setAttribute("aria-pressed", String(chip.dataset.kind === kind));
    selected = 0;
    void refresh();
  }
  for (const chip of chips) chip.addEventListener("click", () => setFilter((chip.dataset.kind ?? "") as Kind | ""));

  function showToast(html: string, ms: number): void {
    clearTimeout(copiedTimer);
    toast.innerHTML = html;
    toast.hidden = false;
    copiedTimer = window.setTimeout(() => {
      toast.hidden = true;
    }, ms);
  }

  function select(i: number): void {
    if (i === selected || i < 0 || i >= items.length) return;
    list.children[selected]?.setAttribute("aria-selected", "false");
    selected = i;
    const row = list.children[selected];
    row?.setAttribute("aria-selected", "true");
    row?.scrollIntoView({ block: "nearest" });
  }

  async function refresh(): Promise<void> {
    const all = await invoke<Item[]>("clipboard_list", { query: search.value, kind: filter });
    items = all.filter((item) => item.id !== pendingId);
    // Unfiltered and unsearched, the list is everything: thumbnails not in it are gone.
    if (!filter && !search.value.trim()) dropThumbs(new Set(all.map((i) => i.id)));
    selected = Math.min(selected, Math.max(0, items.length - 1));
    render();
  }

  /** Copies. Unpinned, hides the popover at once so Ctrl+V goes to the user's app; pinned, it
   *  stays open and shows "Copied. Click your app, then Ctrl+V". */
  async function copy(item: Item): Promise<void> {
    try {
      await invoke("clipboard_copy", { id: item.id });
    } catch (e) {
      // For example "The file is no longer there": say so and stay open.
      const message = document.createElement("span");
      message.textContent = String(e);
      showToast(message.outerHTML, ERROR_MS);
      return;
    }
    search.value = "";
    selected = 0;
    void refresh();
    clearTimeout(copiedTimer);
    if (document.body.dataset.pinned !== "true") {
      toast.hidden = true;
      getCurrentWindow().hide().catch(() => {});
      return;
    }
    showToast(toastHtml, COPIED_PINNED_MS);
  }

  async function togglePin(item: Item): Promise<void> {
    await invoke("clipboard_pin", { id: item.id, pinned: !item.pinned });
    await refresh();
  }

  // ---- Undo bar, shared by delete and the Keep change ----

  async function commitUndo(): Promise<void> {
    const u = undo;
    if (!u) return;
    clearTimeout(u.timer);
    undo = null;
    undoBar.hidden = true;
    await u.commit();
  }

  async function offerUndo(text: string, commit: () => Promise<void>, revert: () => Promise<void>): Promise<void> {
    await commitUndo();
    undoText.textContent = text;
    undo = { commit, revert, timer: undoTimer() };
    undoBar.hidden = false;
  }

  const undoTimer = () => window.setTimeout(() => void commitUndo().then(refresh), UNDO_MS);
  // The bar stays while the pointer is on it, so a slow click on Undo still lands.
  undoBar.addEventListener("pointerenter", () => {
    if (undo) clearTimeout(undo.timer);
  });
  undoBar.addEventListener("pointerleave", () => {
    if (!undo) return;
    clearTimeout(undo.timer);
    undo.timer = undoTimer();
  });

  undoBar.querySelector("button")!.addEventListener("click", async () => {
    const u = undo;
    if (!u) return;
    clearTimeout(u.timer);
    undo = null;
    undoBar.hidden = true;
    await u.revert().catch((e) => {
      void invoke("app_log", { level: "error", message: `clipboard: undo failed: ${String(e)}` }).catch(() => {});
    });
    void refresh();
    search.focus();
  });

  // Delete (like Notes): hide at once, delete for real after 5 s.
  async function remove(item: Item): Promise<void> {
    await offerUndo(
      "Item deleted",
      async () => {
        await invoke("clipboard_delete", { id: item.id }).catch(() => {});
        if (pendingId === item.id) pendingId = null;
      },
      async () => {
        pendingId = null;
      },
    );
    pendingId = item.id;
    items = items.filter((i) => i.id !== item.id);
    selected = Math.min(selected, Math.max(0, items.length - 1));
    render();
  }

  async function loadSettings(): Promise<void> {
    const settings = await invoke<{ keepHours: number }>("clipboard_get_settings");
    keepHours = settings.keepHours;
    const options = [...KEEP_OPTIONS];
    if (!options.some(([h]) => h === keepHours)) options.push([keepHours, `${keepHours} hours`]);
    keep.replaceChildren(
      ...options.map(([hours, label]) => new Option(label, String(hours), false, hours === keepHours)),
    );
  }

  // A shorter Keep removes older unpinned items at once, with "N older items removed · Undo".
  // Undo puts the items and the old Keep back (Rust: clipboard_undo_keep).
  keep.addEventListener("change", async () => {
    const before = keepHours;
    const hours = Number(keep.value);
    await commitUndo();
    let removed: Item[];
    try {
      removed = await invoke<Item[]>("clipboard_set_keep_hours", { hours });
    } catch {
      keep.value = String(before);
      return;
    }
    keepHours = hours;
    if (removed.length > 0) {
      const n = removed.length;
      await offerUndo(
        `${n} older ${n === 1 ? "item" : "items"} removed`,
        // Undo time over: now the removed images' files can go.
        async () => {
          if (removed.some((i) => i.kind === "image"))
            await invoke("clipboard_drop_files", { items: removed }).catch(() => {});
        },
        async () => {
          await invoke("clipboard_undo_keep", { hours: before, items: removed });
          keepHours = before;
          keep.value = String(before);
        },
      );
    }
    await refresh();
  });

  search.addEventListener("input", () => {
    selected = 0;
    fadeNext = true;
    void refresh();
  });

  document.addEventListener("keydown", (e) => {
    if (el.hidden) return;
    const item = items[selected];
    const mod = isMac ? e.metaKey : e.ctrlKey;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      select(selected + 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      select(selected - 1);
    } else if (e.key === "Enter" && item && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      void copy(item);
    } else if (mod && e.key.toLowerCase() === "p" && item) {
      e.preventDefault();
      void togglePin(item);
    } else if ((e.key === "Delete" || (mod && e.key === "Backspace")) && item && !search.value) {
      // With text in the search box these keys edit the search, not the list.
      e.preventDefault();
      void remove(item);
    }
  });

  // New copy recorded, or old items deleted by the timer.
  void listen("clipboard://changed", () => void refresh());

  // Window shown again: start fresh with an empty search and the newest item selected.
  // Window hidden or left: the undo bar stops its timer while the pointer is on it, and the
  // pointerleave may never come, so restart the 5 s and the pending delete really happens.
  void getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (!focused) {
      if (undo) {
        clearTimeout(undo.timer);
        undo.timer = undoTimer();
      }
      return;
    }
    if (document.body.dataset.pinned !== "true") toast.hidden = true;
    search.value = "";
    selected = 0;
    filter = "";
    for (const chip of chips) chip.setAttribute("aria-pressed", String(chip.dataset.kind === ""));
    if (!el.hidden) search.focus();
    void refresh();
  });

  // Time left changes while the window is open.
  setInterval(() => {
    if (!el.hidden && items.length > 0) render();
  }, 60_000);

  void loadSettings().then(refresh);
  search.focus();
}
