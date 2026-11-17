// Clipboard feature. Called once, the first time the Clipboard tab is shown.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./clipboard.css";

interface Item {
  id: number;
  text: string;
  createdAt: number; // unix seconds
  pinned: boolean;
}

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
/** How long "Copied" shows before the popover hides; pinned, it stays open and the notice longer. */
const COPIED_MS = 700;
const COPIED_PINNED_MS = 1500;

export function mount(el: HTMLElement): void {
  el.innerHTML = `
    <div class="toolbar"><input class="field" type="search" placeholder="Search clipboard" aria-label="Search clipboard" spellcheck="false" /></div>
    <ul class="scroll cb-list" role="listbox" aria-label="Clipboard history"></ul>
    <div class="undo-bar" role="status" hidden>
      <span class="cb-undo-text">Item deleted</span>
      <button type="button">Undo</button>
    </div>
    <div class="toast" role="status" hidden>Copied, press <kbd>${isMac ? "⌘V" : "Ctrl+V"}</kbd> to paste</div>
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

  let items: Item[] = [];
  let selected = 0;
  let keepHours = 24;
  /** A deleted item still in its undo time: hidden from the list, deleted when the timer ends. */
  let pendingId: number | null = null;
  /** What the undo bar offers: commit runs when the 5 s end (or another undoable action starts). */
  let undo: { timer: number; commit: () => Promise<void>; revert: () => Promise<void> } | null = null;
  let copiedTimer: number | undefined;

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
    if (items.length === 0) {
      const empty = document.createElement("li");
      empty.className = "empty";
      empty.textContent = search.value.trim()
        ? "Nothing matches your search."
        : "Copy some text and it shows up here.";
      list.append(empty);
      return;
    }
    items.forEach((item, i) => {
      const row = document.createElement("li");
      row.className = "row cb-row";
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", String(i === selected));
      row.title = item.text.length > 500 ? item.text.slice(0, 500) + "..." : item.text;

      const text = document.createElement("span");
      text.className = "cb-text";
      text.textContent = item.text;

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

      row.append(text, actions, left);
      row.addEventListener("click", () => void copy(item));
      row.addEventListener("mousemove", () => select(i));
      list.append(row);
    });
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
    const all = await invoke<Item[]>("clipboard_list", { query: search.value });
    items = all.filter((item) => item.id !== pendingId);
    selected = Math.min(selected, Math.max(0, items.length - 1));
    render();
  }

  /** Copies, shows "Copied, press Ctrl+V to paste", then hides the popover (unless pinned). */
  async function copy(item: Item): Promise<void> {
    await invoke("clipboard_copy", { id: item.id });
    search.value = "";
    selected = 0;
    void refresh();
    const pinned = document.body.dataset.pinned === "true";
    toast.hidden = false;
    clearTimeout(copiedTimer);
    copiedTimer = window.setTimeout(
      () => {
        toast.hidden = true;
        if (!pinned) getCurrentWindow().hide().catch(() => {});
      },
      pinned ? COPIED_PINNED_MS : COPIED_MS,
    );
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
    undo = { commit, revert, timer: window.setTimeout(() => void commitUndo().then(refresh), UNDO_MS) };
    undoBar.hidden = false;
  }

  undoBar.querySelector("button")!.addEventListener("click", async () => {
    const u = undo;
    if (!u) return;
    clearTimeout(u.timer);
    undo = null;
    undoBar.hidden = true;
    await u.revert().catch(() => {});
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
        async () => {},
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
    } else if (e.key === "Enter" && item) {
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

  // Window shown again: start fresh with the newest item selected.
  void getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (!focused) return;
    if (document.body.dataset.pinned !== "true") toast.hidden = true;
    selected = 0;
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
