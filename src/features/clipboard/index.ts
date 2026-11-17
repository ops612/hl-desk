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

export function mount(el: HTMLElement): void {
  el.innerHTML = `
    <div class="cb-search"><input type="search" placeholder="Search clipboard" aria-label="Search clipboard" spellcheck="false" /></div>
    <ul class="cb-list" role="listbox" aria-label="Clipboard history"></ul>
    <footer class="cb-footer">
      <span class="cb-hint">Click to copy</span>
      <label class="cb-keep">Keep: <select aria-label="Keep items for"></select></label>
    </footer>`;
  const search = el.querySelector<HTMLInputElement>("input")!;
  const list = el.querySelector<HTMLUListElement>(".cb-list")!;
  const keep = el.querySelector<HTMLSelectElement>("select")!;

  let items: Item[] = [];
  let selected = 0;
  let keepHours = 24;

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
      empty.className = "cb-empty";
      empty.textContent = search.value.trim()
        ? "Nothing matches your search."
        : "Copy some text and it shows up here.";
      list.append(empty);
      return;
    }
    items.forEach((item, i) => {
      const row = document.createElement("li");
      row.className = "cb-row";
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", String(i === selected));
      row.title = item.text.length > 500 ? item.text.slice(0, 500) + "..." : item.text;

      const text = document.createElement("span");
      text.className = "cb-text";
      text.textContent = item.text;

      const actions = document.createElement("span");
      actions.className = "cb-actions";
      const pin = document.createElement("button");
      pin.className = "cb-icon" + (item.pinned ? " cb-on" : "");
      pin.title = item.pinned ? "Unpin" : `Pin (${isMac ? "Cmd" : "Ctrl"}+P)`;
      pin.innerHTML = PIN_ICON;
      pin.addEventListener("click", (e) => {
        e.stopPropagation();
        void togglePin(item);
      });
      const del = document.createElement("button");
      del.className = "cb-icon";
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
    items = await invoke<Item[]>("clipboard_list", { query: search.value });
    selected = Math.min(selected, Math.max(0, items.length - 1));
    render();
  }

  async function copy(item: Item): Promise<void> {
    // Rust writes the text to the clipboard and hides the window.
    await invoke("clipboard_copy", { id: item.id });
    search.value = "";
    selected = 0;
  }

  async function togglePin(item: Item): Promise<void> {
    await invoke("clipboard_pin", { id: item.id, pinned: !item.pinned });
    await refresh();
  }

  async function remove(item: Item): Promise<void> {
    await invoke("clipboard_delete", { id: item.id });
    await refresh();
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

  keep.addEventListener("change", async () => {
    await invoke("clipboard_set_keep_hours", { hours: Number(keep.value) });
    keepHours = Number(keep.value);
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
    } else if ((e.key === "Delete" || (mod && e.key === "Backspace")) && item) {
      e.preventDefault();
      void remove(item);
    }
  });

  // New copy recorded, or old items deleted by the timer.
  void listen("clipboard://changed", () => void refresh());

  // Window shown again: start fresh with the newest item selected.
  void getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (!focused) return;
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
