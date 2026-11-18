# Third-party notices

## Translation engine and language models (Mozilla Public License 2.0)

The text translation feature uses software and data from Mozilla's Firefox Translations project,
licensed under the Mozilla Public License, v. 2.0 (MPL-2.0). A copy of the licence is at
https://mozilla.org/MPL/2.0/.

- **Bergamot translator** (bundled in the app as `public/translate/bergamot-translator.js` and
  `public/translate/bergamot-translator.wasm`, unmodified):
  - JavaScript glue: mozilla-central revision ab91df22f920 (bergamot-translator v0.6.0+eea6e5a8),
    https://hg-edge.mozilla.org/mozilla-central/raw-file/ab91df22f920/toolkit/components/translations/bergamot-translator/bergamot-translator.js
  - WebAssembly build: Firefox Remote Settings collection `translations-wasm`, record
    `bergamot-translator` v3.0, sha256 a3a89d9ad0a4ed8f27bf3e403701b23f5709816f6376438503f2fa5b0182c2dc.
  - Source code: https://github.com/mozilla/translations (inference/) and
    https://github.com/browsermt/bergamot-translator
- **Engine worker** (`public/translate/engine.worker.js`): adapted from Firefox
  `toolkit/components/translations/content/translations-engine.worker.js`, MPL-2.0; source at
  https://hg-edge.mozilla.org/mozilla-central/file/tip/toolkit/components/translations/content/
- **Language models** (downloaded on first use, not bundled): Firefox Remote Settings collection
  `translations-models`, Chinese (Traditional) to English, Chinese (Simplified) to English and
  English to Chinese (Traditional), version 2.0, served unmodified from the HL Desk releases
  mirror (release `models-v1`). Files, sizes and sha256 are pinned in
  `src-tauri/src/translate.rs` (`MODELS`). Training code and model sources:
  https://github.com/mozilla/translations and https://github.com/mozilla/firefox-translations-models

## Font

IBM Plex Sans, SIL Open Font License 1.1, bundled through `@fontsource/ibm-plex-sans`.
