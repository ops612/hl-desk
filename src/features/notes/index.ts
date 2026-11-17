// Notes feature. Called once, the first time the Notes tab is shown.
// List view (search, +, rows) and editor view (plain textarea, autosave). No Save button.
import { invoke } from "@tauri-apps/api/core";
import "./notes.css";

interface NoteSummary {
  id: number;
  title: string;
  preview: string;
  updatedAt: number;
}

interface Note {
  id: number;
  title: string;
  body: string;
  updatedAt: number;
}

/** The note open in the editor. id is null until the first non-empty save creates it. */
interface Draft {
  id: number | null;
  body: string;
  dirty: boolean;
}

const AUTOSAVE_MS = 400;
const UNDO_MS = 5000;
const isMac = navigator.userAgent.includes("Mac");

const api = {
  list: (query: string) => invoke<NoteSummary[]>("notes_list", { query }),
  get: (id: number) => invoke<Note | null>("notes_get", { id }),
  create: () => invoke<Note>("notes_create"),
  save: (id: number, body: string) => invoke<number>("notes_save", { id, body }),
  remove: (id: number) => invoke<void>("notes_delete", { id }),
};

/** "Today 4:25 pm", "Yesterday", "12 Nov" (with the year when it is not this year). */
function editedLabel(ms: number, now = new Date()): string {
  const d = new Date(ms);
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(now) - day(d)) / 86_400_000);
  if (days === 0) {
    const h = d.getHours();
    return `Today ${h % 12 || 12}:${String(d.getMinutes()).padStart(2, "0")} ${h < 12 ? "am" : "pm"}`;
  }
  if (days === 1) return "Yesterday";
  const month = d.toLocaleString("en-GB", { month: "short" });
  return `${d.getDate()} ${month}${d.getFullYear() === now.getFullYear() ? "" : ` ${d.getFullYear()}`}`;
}

const ICON_BACK =
  '<svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M10 3L5 8l5 5"/></svg>';
const ICON_TRASH =
  '<svg width="15" height="15" viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 4h11M6.5 4V2.5h3V4M4 4l.7 9.5h6.6L12 4M6.8 6.5v4.5M9.2 6.5v4.5"/></svg>';

export function mount(el: HTMLElement): void {
  el.innerHTML = `
    <div class="notes-view notes-list-view">
      <div class="toolbar">
        <input class="field" type="search" placeholder="Search notes" aria-label="Search notes"
          aria-controls="notes-rows" autocomplete="off" spellcheck="false" data-autofocus />
        <button class="btn-primary notes-add" type="button" aria-label="New note"
          title="New note (${isMac ? "⌘" : "Ctrl+"}N)">+</button>
      </div>
      <div class="scroll" id="notes-rows" role="listbox" aria-label="Notes"></div>
      <div class="undo-bar" role="status" hidden>
        <span>Note deleted</span>
        <button type="button">Undo</button>
      </div>
    </div>
    <div class="notes-view notes-editor-view" hidden>
      <div class="toolbar">
        <button class="icon-btn notes-back" type="button" aria-label="Back to notes" title="Back to notes">${ICON_BACK}</button>
        <span class="hint notes-status" role="status"></span>
        <button class="icon-btn notes-trash" type="button" aria-label="Delete note" title="Delete note">${ICON_TRASH}</button>
      </div>
      <textarea class="field notes-body" aria-label="Note"
        placeholder="Start typing. The first line becomes the title."></textarea>
    </div>
    <div class="footer"><span class="notes-count">Saved automatically</span></div>`;

  const $ = <T extends HTMLElement>(sel: string) => el.querySelector<T>(sel)!;
  const listView = $(".notes-list-view");
  const editorView = $(".notes-editor-view");
  const search = $<HTMLInputElement>('input[type="search"]');
  const rowsEl = $("#notes-rows");
  const undoBar = $(".undo-bar");
  const textarea = $<HTMLTextAreaElement>(".notes-body");
  const status = $(".notes-status");
  const countEl = $(".notes-count");

  let notes: NoteSummary[] = [];
  let total = 0;
  let selected = -1;
  let draft: Draft | null = null;
  let saveTimer: number | undefined;
  let saveChain: Promise<void> = Promise.resolve();
  let pending: { id: number; timer: number } | null = null;
  let refreshToken = 0;

  // ---- List ----

  async function refresh(): Promise<void> {
    const token = ++refreshToken;
    const query = search.value.trim();
    try {
      const [found, all] = await Promise.all([api.list(query), query ? api.list("") : null]);
      if (token !== refreshToken) return;
      const hidden = pending?.id;
      notes = found.filter((n) => n.id !== hidden);
      total = (all ?? found).filter((n) => n.id !== hidden).length;
      selected = notes.length ? Math.min(Math.max(selected, 0), notes.length - 1) : -1;
      renderList();
    } catch (e) {
      if (token !== refreshToken) return;
      rowsEl.innerHTML = "";
      rowsEl.append(emptyState("Could not load notes", String(e)));
    }
    renderCount();
  }

  function renderCount(): void {
    countEl.textContent = `Saved automatically · ${total} ${total === 1 ? "note" : "notes"}`;
  }

  function emptyState(text: string, hint: string): HTMLElement {
    const box = document.createElement("div");
    box.className = "empty";
    const p = document.createElement("p");
    p.textContent = text;
    const h = document.createElement("p");
    h.className = "hint";
    h.textContent = hint;
    box.append(p, h);
    return box;
  }

  function renderList(): void {
    rowsEl.innerHTML = "";
    if (!notes.length) {
      const query = search.value.trim();
      rowsEl.append(
        query
          ? emptyState(`No notes match "${query}"`, "Try another word.")
          : emptyState("Write your first note", `Press + or ${isMac ? "⌘" : "Ctrl+"}N to start.`),
      );
      return;
    }
    notes.forEach((note, i) => {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "row notes-row" + (i === selected ? " selected" : "");
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", String(i === selected));
      row.tabIndex = -1;
      const line = document.createElement("span");
      line.className = "notes-line";
      const title = document.createElement("span");
      title.className = "notes-title";
      title.textContent = note.title || "New note";
      const date = document.createElement("span");
      date.className = "notes-date";
      date.textContent = editedLabel(note.updatedAt);
      line.append(title, date);
      const preview = document.createElement("span");
      preview.className = "notes-preview";
      preview.textContent = note.preview || "No more text";
      row.append(line, preview);
      row.addEventListener("click", () => void openNote(note.id));
      rowsEl.append(row);
    });
  }

  function select(i: number): void {
    if (!notes.length) return;
    selected = Math.min(Math.max(i, 0), notes.length - 1);
    rowsEl.querySelectorAll(".notes-row").forEach((row, j) => {
      row.classList.toggle("selected", j === selected);
      row.setAttribute("aria-selected", String(j === selected));
      if (j === selected) row.scrollIntoView({ block: "nearest" });
    });
  }

  let searchTimer: number | undefined;
  search.addEventListener("input", () => {
    clearTimeout(searchTimer);
    selected = 0;
    searchTimer = window.setTimeout(() => void refresh(), 120);
  });
  search.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      select(selected + (e.key === "ArrowDown" ? 1 : -1));
    } else if (e.key === "Enter" && selected >= 0 && notes[selected]) {
      e.preventDefault();
      void openNote(notes[selected].id);
    } else if (e.key === "Escape" && search.value) {
      // First Esc clears the search; the next one closes the window (main.ts).
      e.preventDefault();
      search.value = "";
      selected = 0;
      void refresh();
    }
  });
  $(".notes-add").addEventListener("click", () => void newNote());

  // ---- Editor ----

  function showView(editor: boolean): void {
    listView.hidden = editor;
    editorView.hidden = !editor;
    // main.ts focuses [data-autofocus] when the Notes tab is shown again.
    search.toggleAttribute("data-autofocus", !editor);
    textarea.toggleAttribute("data-autofocus", editor);
    status.textContent = "";
  }

  function openEditor(next: Draft): void {
    draft = next;
    textarea.value = next.body;
    showView(true);
    textarea.focus();
    textarea.setSelectionRange(next.body.length, next.body.length);
    textarea.scrollTop = 0;
  }

  async function openNote(id: number): Promise<void> {
    try {
      const note = await api.get(id);
      if (!note) return void refresh();
      openEditor({ id: note.id, body: note.body, dirty: false });
    } catch (e) {
      status.textContent = `Could not open: ${e}`;
    }
  }

  async function newNote(): Promise<void> {
    if (draft) {
      if (draft.id === null && !draft.body.trim()) return textarea.focus();
      await closeDraft();
    }
    openEditor({ id: null, body: "", dirty: false });
  }

  /** Queues a save of the draft; saves run one at a time, in order. */
  function save(): Promise<void> {
    clearTimeout(saveTimer);
    const d = draft;
    if (!d) return saveChain;
    saveChain = saveChain.then(async () => {
      if (!d.dirty) return;
      const body = d.body;
      try {
        if (d.id === null) {
          if (!body.trim()) return;
          d.id = (await api.create()).id;
          // The footer count includes the new note from its first character.
          total += 1;
          renderCount();
        }
        await api.save(d.id, body);
        if (d.body === body) d.dirty = false;
        if (draft === d) status.textContent = "";
      } catch (e) {
        if (draft === d) status.textContent = `Not saved: ${e}`;
      }
    });
    return saveChain;
  }

  /** Saves the open note and leaves the editor. An emptied note is removed (nothing to undo). */
  async function closeDraft(): Promise<void> {
    const d = draft;
    if (!d) return;
    await save();
    draft = null;
    if (d.id !== null && !d.body.trim()) {
      await api.remove(d.id).catch(() => {});
    }
  }

  async function backToList(): Promise<void> {
    await closeDraft();
    showView(false);
    await refresh();
    search.focus();
  }

  textarea.addEventListener("input", () => {
    if (!draft) return;
    draft.body = textarea.value;
    draft.dirty = true;
    clearTimeout(saveTimer);
    // A new note is created on its first character (so it counts at once); then autosave.
    if (draft.id === null && draft.body.trim()) void save();
    else saveTimer = window.setTimeout(() => void save(), AUTOSAVE_MS);
  });
  $(".notes-back").addEventListener("click", () => void backToList());
  $(".notes-trash").addEventListener("click", () => void deleteDraft());
  // Leaving the window (Esc, click outside, Quit) saves at once.
  window.addEventListener("blur", () => void save());
  document.addEventListener("visibilitychange", () => void save());

  // ---- Delete with undo ----

  async function commitPending(): Promise<void> {
    const p = pending;
    if (!p) return;
    clearTimeout(p.timer);
    pending = null;
    undoBar.hidden = true;
    await api.remove(p.id).catch(() => {});
  }

  async function deleteDraft(): Promise<void> {
    const d = draft;
    if (!d) return;
    clearTimeout(saveTimer);
    await save();
    draft = null;
    if (d.id !== null) {
      await commitPending();
      if (d.body.trim()) {
        pending = { id: d.id, timer: window.setTimeout(() => void commitPending().then(refresh), UNDO_MS) };
        undoBar.hidden = false;
      } else {
        await api.remove(d.id).catch(() => {});
      }
    }
    showView(false);
    await refresh();
    search.focus();
  }

  undoBar.querySelector("button")!.addEventListener("click", () => {
    if (!pending) return;
    clearTimeout(pending.timer);
    pending = null;
    undoBar.hidden = true;
    void refresh();
    search.focus();
  });

  // Cmd+N (Ctrl+N on Windows) makes a new note while the Notes tab is showing.
  document.addEventListener("keydown", (e) => {
    if (el.hidden || e.defaultPrevented) return;
    const cmd = isMac ? e.metaKey : e.ctrlKey;
    if (cmd && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "n") {
      e.preventDefault();
      void newNote();
    }
  });

  void refresh();
}
