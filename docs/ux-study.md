# UX study: how the leading tools work (2026-11-17)

How CleanShot X, Shottr, Snagit, ShareX, Windows Snipping Tool, macOS screenshots, Maccy, Raycast, Paste, Ditto, Win+V, Apple Quick Note, Raycast Notes and Tot handle each flow, compared with HL Desk 0.1.2. Done by a research agent from official docs, help centres, changelogs and READMEs (sources at the end). "Unconfirmed" means no source was found.

Scope rule (owner, 2026-11-17): build only what serves the director's three requirements (clipboard text history with a changeable auto-delete; screenshot with number, pen, crop, drag-and-drop; notes). Everything else is PARKED in [ideas.md](ideas.md).

## Capture

Leaders:
1. macOS Cmd+Shift+4: crosshair at once; drag; Space moves the selection; Esc cancels (Apple).
2. CleanShot X: optional crosshair and magnifier; "Freeze screen" (features page, changelog 3.4).
3. Snagit: crosshairs and magnifier (M toggles), arrow keys nudge 1 px; Print Screen is its default key (TechSmith).
4. ShareX: magnifier, click a detected window, Esc cancels (Esc first clears a selection while annotating); Ctrl+PrtSc is region capture.
5. Snipping Tool: Win+Shift+S; the screen "changes slightly to gray" (Microsoft).

HL Desk today: Mac uses `screencapture -i`. Windows freezes screens, encodes PNGs, then shows the overlay: a silent gap; a second press is ignored; cancel is silent.

In scope:
- Instant feedback (M, Windows): dim, crosshair cursor and a large "Drag to capture · Esc to cancel" hint immediately, before the frozen image is ready.
- Never cancel silently (S, both): a short "Capture cancelled" notice; a second press of the shortcut cancels or restarts.
- Use the device's keys (S-M, both): see Platform notes, options A/B/C.
- Crosshair lines and a magnifier with pixel size (M, Windows).

## After capture

Leaders:
1. macOS: thumbnail bottom-right for a few seconds; click opens markup, drag drops it into a document.
2. CleanShot Quick Access Overlay: corner pop-up you can drag out; Cmd+C copy, Cmd+S save, Cmd+E annotate; auto-close timer; restore recently closed.
3. Snipping Tool (Windows 11): copies to the clipboard, auto-saves to Pictures\Screenshots, notification opens the editor.
4. Shottr: opens the editor by default; thumbnail and auto copy/save are settings.

HL Desk today: the editor opens at once.

In scope:
- Keep editor-first (like Shottr and Snagit) and bring it to the front reliably (S, both).
- Floating corner thumbnail that drags out and opens the editor on click (M, both): only if the bigger Drag bar below is not enough.

## Editor

Leaders:
1. Snagit Move tool: select, move and delete objects with Delete.
2. Shottr: select + Delete; Opt+drag duplicates; redo with Cmd+Shift+Z.
3. ShareX: Select tool (V), Step tool (N).
4. CleanShot: a "Drag me" button in Annotate; counters stay on top. Object deletion unconfirmed.

HL Desk today: markers move but cannot be selected or deleted; "Drag me" is a 128 px thumbnail in a side column; Esc saves and closes.

In scope:
- Select and delete a marker, the rest renumber (S, both).
- A wide labelled "Drag to share" bar instead of the small thumbnail (S, both).
- Select and delete pen strokes; Esc deselects first (M, both).
- Crop handles to adjust before applying; redo (M, both).

## Clipboard

Leaders:
1. Maccy: type to search; Enter copies, Option+Enter pastes; "Paste automatically" setting; Option+P pins; ignores Concealed/Transient types and listed apps.
2. Raycast: Enter pastes into the active field (configurable); Keep History For from 1 day to unlimited; disabled apps.
3. Paste: lowering Keep History asks whether to delete older items, says it can't be undone, keeps pinned items. The only tool found that documents this case.
4. Ditto: Enter pastes; expiry off by default (third-party sources).
5. Win+V: 25 items, cleared on restart except pinned.

HL Desk today: click or Enter copies and hides; changing Keep deletes at once.

In scope:
- Safe Keep change (S, both). Option A (recommended, fits our "undo, not confirm" rule): apply and show "12 items removed · Undo" for 5 s. Option B: a Paste-style confirm with the count.
- "Copied, press Ctrl+V" notice as the popover hides (S, both).

## Notes

Leaders:
1. Apple Quick Note: Fn+Q or a hot corner; stays visible while open; resumes the last note.
2. Raycast Notes: one note at a time; Cmd+N new, Cmd+P browse; float above all windows; Recently Deleted.
3. Tot: "Display as Floating Window".

HL Desk today: list then editor in the popover; autosave; no dates; the popover hides on blur.

In scope:
- Stay open while writing (S, both): see Popover.
- Edited date on each row (S, both).
- Reopen where you left off (already does); optional global "new note" key (S, both).

## Popover / shell

Leaders: Apple NSPopover `transient` closes on outside click, `applicationDefined` leaves it to the app, a popover can detach into a panel; Raycast Notes and Tot stay on top until dismissed. Windows tray flyout conventions: unconfirmed.

HL Desk today: always hides on blur.

In scope:
- Pin button in the header (S, both): when pinned, no hide on blur and stays on top; Esc still closes. Alternative: keep open automatically only while a note is being edited.

## Top 10 in scope (value vs effort)

1. Select/delete a marker, with renumbering (S)
2. Safe Keep change with count and Undo (S)
3. Pin/keep-open popover (S)
4. Bigger "Drag to share" bar (S)
5. Instant capture feedback, no silent cancel (M)
6. Capture-key setting: Print Screen / Cmd+Shift+4, guided (S-M)
7. "Copied, press Ctrl+V" notice (S)
8. Dates on notes (S)
9. Crosshair and magnifier on the Windows overlay (M)
10. Catch Win+Shift+S results into the markup editor (M)

## Platform notes

- Win+Shift+S cannot be registered: Windows-key hotkeys are reserved for the OS (WM_HOTKEY docs).
- Print Screen can be registered (Snagit, ShareX, Greenshot), but on Windows 11 the user must turn off "Use the Print screen key to open Snipping Tool" (Settings > Accessibility > Keyboard, or Bluetooth & devices > Keyboard). Registry: `HKCU\Control Panel\Keyboard\PrintScreenKeyForSnippingEnabled` (community-documented). OneDrive can also hold the key. Windows 10 wording unconfirmed.
- Snipping Tool output: clipboard image via `AddClipboardFormatListener` / `WM_CLIPBOARDUPDATE`; Windows 11 also auto-saves to `Pictures\Screenshots`.
- macOS Cmd+Shift+4: the user turns off the system shortcut (System Settings > Keyboard > Keyboard Shortcuts > Screenshots), then the app registers it. Whether CleanShot does this itself conflicts between sources; treat it as guided.
- Watching macOS screenshots: `com.apple.screencapture location` (default Desktop) and the `kMDItemIsScreenCapture` attribute; the file is written after the floating thumbnail goes away; a Desktop-folder access prompt is likely (unconfirmed).
- Options for using the device's keys: A guided takeover of Print Screen / Cmd+Shift+4 with Ctrl+Shift+2 as fallback; B leave the OS keys alone and catch the result; C both, with B only for Win+Shift+S (recommended).
- Paste into the previous app (parked): macOS needs `CGEvent` Cmd+V and Accessibility permission (`AXIsProcessTrustedWithOptions`), confirmed in Maccy's FAQ; Windows SendInput equivalent unconfirmed.

## Sources

CleanShot: https://cleanshot.com/features · https://cleanshot.com/changelog · https://www.podfeet.com/blog/2022/04/cleanshot-x/ · https://www.imore.com/cleanshot-x-gains-new-keyboard-shortcuts-quick-access-overlay-more
Shottr: https://shottr.cc/ · https://buttondown.com/shottr/archive/shottr-v153-is-out
Snagit: https://www.techsmith.com/learn/tutorials/snagit/snagit-hotkeys/ · https://www.techsmith.com/learn/tutorials/snagit/image-capture/ · https://www.techsmith.com/learn/tutorials/snagit/move/ · https://www.techsmith.com/learn/tutorials/snagit/step-capture/ · https://support.techsmith.com/hc/en-us/articles/203731428-Snagit-Print-Screen-Hotkey-Not-Working
ShareX: https://getsharex.com/docs/region-capture
Windows: https://support.microsoft.com/en-us/windows/use-snipping-tool-to-capture-screenshots-00246869-1843-655f-f220-97299b865f6b · https://learn.microsoft.com/en-us/answers/a/12689731 · https://www.elevenforum.com/t/enable-or-disable-use-print-screen-key-to-open-screen-snipping-in-windows-11.520/latest · https://learn.microsoft.com/windows/win32/inputdev/wm-hotkey · https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-addclipboardformatlistener · https://learn.microsoft.com/en-in/windows/win32/dataxchg/clipboard-formats · https://support.microsoft.com/en-us/windows/clipboard-in-windows-c436501e-985d-1c8d-97ea-fe46ddf338c6
macOS: https://support.apple.com/en-us/102646 · https://support.apple.com/guide/mac-help/take-a-screenshot-mh26782/mac · https://discussions.apple.com/thread/254212073 · https://alexwlchan.net/2019/finding-the-latest-screenshot-in-macos-mojave/ · https://support.apple.com/guide/notes/create-quick-notes-apdf028f7034/mac · https://developer.apple.com/documentation/appkit/nspopover/behavior-swift.enum · https://developer.apple.com/forums/thread/708652
Clipboard: https://github.com/p0deje/Maccy · https://manual.raycast.com/clipboard-history · https://pasteapp.io/help/control-history-retention · https://deepwiki.com/sabrogden/Ditto/2.2.2-options-and-configuration
Notes: https://manual.raycast.com/notes · https://www.raycast.com/blog/raycast-notes · https://iconfactory.happyfox.com/kb/article/62-floating-window
