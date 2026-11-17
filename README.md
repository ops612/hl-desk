# HL Desk

A small menu-bar (macOS) / tray (Windows) app with three tabs: Clipboard, Screenshot and Notes.
Click the icon or press Cmd+Shift+Space (Ctrl+Shift+Space on Windows) to open it.
"HL Desk" is a placeholder name. All data stays on the computer. The plan is in `docs/plan.md`.

Built with Tauri 2 (Rust + the system web view) and a Vite + TypeScript front end with no framework.

## Local development

Node 22 is enough for the front end:

```
npm ci
npm run typecheck
npm run build
```

Running the whole app (`npm run tauri dev`) needs Rust and, on Windows, the MSVC build tools.

## How CI builds it

`.github/workflows/build.yml`:

- **macOS** (every push to `main`, and by hand): compiles a universal binary (Apple Silicon + Intel),
  then builds the `.app` and `.dmg`, signs them with the Developer ID certificate, notarizes and staples
  both, checks them with `spctl` and uploads the `.dmg` as the artifact `HL-Desk-macos-universal`.
  If a signing secret is missing the job fails and names it; it never ships an unsigned build.
  It also uploads the resolved `Cargo.lock`.
- **Windows** (only when started by hand, "Run workflow"): builds an unsigned NSIS installer,
  artifact `HL-Desk-windows`. Windows asks "More info, Run anyway" once.

Repository secrets used by the macOS job (names only):

- `MAC_DEVID_APP_P12`: base64 of the Developer ID Application .p12
- `MAC_CERT_PASSWORD`: the .p12 password
- `ASC_KEY_P8`: the App Store Connect API key (.p8 contents, raw or base64)
- `ASC_KEY_ID`: that key's ID
- `ASC_ISSUER_ID`: the App Store Connect issuer ID

## Architecture

Each feature lives in its own files and touches nothing else:

| Feature | Front end | Rust |
|---|---|---|
| Clipboard | `src/features/clipboard/` | `src-tauri/src/clipboard.rs` |
| Screenshot | `src/features/screenshot/` | `src-tauri/src/screenshot.rs` |
| Notes | `src/features/notes/` | `src-tauri/src/notes.rs` |

Shared files (change only in a separate, reviewed PR): `src/main.ts` (tabs), `src-tauri/src/lib.rs`
(tray, shortcut, plugins, routing), `src-tauri/src/db.rs`, `package.json`, `Cargo.toml`,
`tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/Info.plist`.

- **Front end:** `src/features/<feature>/index.ts` exports `mount(el)`. The tab calls it once, the
  first time it is shown, with the tab's panel element. The panel is a flex column filling the
  popover (about 380 x 520); a feature may import its own CSS file from its folder.
- **Shell (`index.html`, `src/main.ts`, `src/styles.css`):** header with the "HL DESK" wordmark and a
  settings sheet (shortcuts, version, Quit), the segmented tab control (arrow keys, Cmd+1/2/3, last
  tab kept in localStorage), and Esc to hide the window. Esc is handled on `document` in the bubble
  phase: a feature that wants Esc first (for example to clear its search) handles it on its own
  element and calls `preventDefault()`. When a tab is shown again, the element in its panel marked
  `data-autofocus` gets focus.
- **Theme tokens** (CSS variables on `:root`, values in `docs/ui.md`): `--ink`, `--muted`, `--border`,
  `--bg`, `--surface`, `--indigo`, `--indigo-hover`, `--indigo-tint`, `--red`, plus `--radius` (10px)
  and `--radius-sm` (8px). Font: IBM Plex Sans 400/500/600 bundled via `@fontsource/ibm-plex-sans`.
- **Shared classes** (`src/styles.css`):
  - `.btn-primary`: the one indigo-filled button per view. `.btn`: plain bordered button.
    `.icon-btn`: 28px square icon button (put an `aria-label` on it).
  - `.field`: text input or `textarea`, with the indigo focus ring.
  - `.toolbar`: a padded flex row, for search plus buttons at the top of a view.
  - `.scroll`: the part of a view that scrolls (flex: 1).
  - `.row`: a 12px list row (a `button` or `div`); `.row.selected` is the keyboard-selected row.
  - `.hint`: 11px muted text. `.empty`: centred empty-state message.
  - `.footer`: the bottom hint line of a panel (11px, grey band, pushed to the bottom).
- **Rust:** `<feature>.rs` has `init(app)`, called once at start-up, and `commands()`, which returns
  `tauri::generate_handler![...]`. To add a command, write `#[tauri::command] fn <feature>_<name>(...)`
  in the feature file and list it in that `generate_handler!`. `lib.rs` routes every command by its
  `<feature>_` prefix, so it never needs editing. Call it from the front end with
  `invoke("<feature>_<name>", args)` from `@tauri-apps/api/core`.
- **Storage:** `db::open(app, "<feature>.db")` opens that feature's own SQLite file (rusqlite, bundled)
  in the app data folder.
- **Already installed:** global-shortcut, clipboard-manager, fs (scoped to app data, temp, Pictures,
  Desktop), drag (`tauri-plugin-drag` by CrabNebula, drags files out of the window), `xcap` for screen
  capture, and on macOS `objc2-app-kit` for `NSPasteboard` (change count, concealed copies).
- **macOS:** `src-tauri/Info.plist` sets `LSUIElement` (no Dock icon) and the Screen Recording prompt text.
