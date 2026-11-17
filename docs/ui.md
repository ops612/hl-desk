# HL Desk UI rules

Every feature follows these rules. Goal: simple, complete, easy. Someone who has never seen the app can use every feature without help.

## Theme (Hobbyland "Design B", the same tokens as the staff app)

| Token | Value | Use |
|---|---|---|
| ink | #0B1220 | main text |
| muted | #5B6474 | secondary text, hints, time left |
| border | #E3E6EB | all borders and dividers (1px) |
| bg | #F5F6F8 | footers, segmented control, empty canvas |
| surface | #FFFFFF | popover and window background |
| indigo | #2F4CC8 | the one primary action, selected tool, number markers, focus ring |
| indigo hover | #1E3494 | primary hover |
| indigo tint | #EEF1FB | selected row |
| red | #E5484D | pen colour option, delete confirm only |

- Font: IBM Plex Sans (bundled locally, OFL licence), falling back to -apple-system. Chinese falls back to PingFang TC. Sizes: 13px body, 12px rows, 11px hints. Weights 400 and 500 (600 for card titles only).
- Corners: 10px buttons and popover, 8px fields and thumbnails.
- Wordmark: "HL DESK" in 11px, 2px letter spacing, top left (echoes the HOBBYLAND x GROUP monoline logo).
- Light theme only in v1. Follow macOS dark mode later if asked.

## Layout

- Menu-bar popover, about 380 x 520 px. Header (wordmark and settings), then a 3-way segmented control: Clipboard, Screenshot, Notes, then content, then a footer hint line.
- The screenshot editor opens as its own window, sized to the capture: toolbar on top, image in the middle, a "Drag me" drop source on the right, shortcut hints in the footer.
- At most one indigo-filled button per view.

## UX rules

- One click does the obvious thing: clicking a clipboard row copies it and closes the popover; clicking a recent screenshot reopens it in the editor.
- Every list has search at the top and a friendly empty state ("Copy some text and it shows up here").
- Time left is always visible ("23h", "Pinned"). The retention setting sits in the footer, not hidden in settings.
- Keyboard first: arrow keys and Enter in lists; editor keys N (number), P (pen), C (crop), Cmd+Z (undo), Esc (close); Cmd+Shift+Space opens the popover; capture shortcut shown on the Screenshot tab.
- No confirm dialogs except delete-all. Undo instead of "are you sure".
- Settings stay minimal: keep-for time (1 hour to 30 days, or custom hours), launch at login, shortcuts.

## Reference apps (for behaviour, not look)

- Clipboard: Raycast clipboard history and Maccy (search first, click to paste, retention setting).
- Screenshot: CleanShot X and Shottr (counter tool, quick editor, drag-out thumbnail).
- Notes: Apple Notes Quick Note (instant open, autosave, no Save button).
