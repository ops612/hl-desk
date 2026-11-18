# Offline translation and OCR research (2026-11-17, for 0.1.7)

Decisions that came from it are in plan.md's decision log (2026-11-18).


- Engine: Mozilla Bergamot + Mozilla models (MPL-2.0, company use OK; include notice). Pairs live in Firefox: en<->zh-Hant v2.0 (~44 MB +5-8 MB), en->zh-Hans v2.2 (~44+8), zh-Hans->en v2.0 (~60+10), en<->tl v2.0 (~32+3-6). Direct pairs, no pivot. HK vs TW flavour of zh-Hant unconfirmed.
- Models from Remote Settings firefox.settings.services.mozilla.com (collection translations-models); no third-party terms -> mirror pinned versions on our own host (e.g. hl-desk-releases), download on first use (~250 MB for all six).
- Run: try WASM single-threaded in the webview first (no COOP/COEP needed); no Firefox gemm intrinsic -> ~2x slower + known output bug on fallback path; npm @browsermt/bergamot-translator 0.4.9 (2022) may not load v2 models -> build via mozilla build-bergamot.py. Fallback: native C++ sidecar per OS (L). No maintained Rust crate.
- OpenCC (Apache-2.0) s2hk for Simplified->HK Traditional polish. NLLB is CC-BY-NC: not allowed.
- OCR: Windows.Media.Ocr (windows crate Media_Ocr); Chinese needs an OCR language pack (admin install; names unconfirmed, check Get-WindowsCapability). macOS Vision VNRecognizeTextRequest (zh needs macOS 11+, accurate level; check supportedRecognitionLanguages); objc2-vision or a small Swift helper.
- Effort: models S, WASM engine M (1-day spike first), native fallback L, OpenCC S, OCR S-M each, Ctrl+C-twice card M.
- Spike result (2026-11-18): Firefox's bergamot-translator WASM (v0.6.0, MPL-2.0) runs single-threaded with no COOP/COEP in Chromium and in Safari 27.2 on macOS 26.7; zh-Hant->en first translation 0.1-0.9 s, repeat under 0.3 s; about 330 MB heap per loaded pair. It needs WebAssembly SIMD (Safari 16.4+).
