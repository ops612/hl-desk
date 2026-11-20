// What a toast page shows, from its query (toast.html?kind=...): pure, so it can be tested.

export interface ToastSetup {
  kind: string;
  /** The capture behind a "copied" toast (its Mark up opens editor-<id>); null otherwise. */
  id: number | null;
  /** Auto-close after this long (paused while the pointer is over the toast). */
  closeAfterMs: number;
}

const CLOSE_AFTER_MS: Record<string, number> = { cancelled: 2000, copied: 4000, shot: 8000 };

export function toastSetup(search: string): ToastSetup {
  const params = new URLSearchParams(search);
  const kind = params.get("kind") ?? "";
  const id = Number(params.get("id"));
  return {
    kind,
    id: kind === "copied" && Number.isInteger(id) && id > 0 ? id : null,
    closeAfterMs: CLOSE_AFTER_MS[kind] ?? 2000,
  };
}
