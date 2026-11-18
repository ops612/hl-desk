//! Text recognition (OCR) of a screenshot capture, for the editor's Translate button (0.1.7).
//! Uses the system's own OCR, so nothing is downloaded:
//! - Windows: Windows.Media.Ocr. Chinese needs the OCR language pack (Settings > Language);
//!   nothing is installed from here.
//! - macOS: Vision (VNRecognizeTextRequest, accurate level; Chinese needs macOS 11+).
//!
//! One command, `ocr_capture(id)` (routed by its "ocr_" prefix in lib.rs). It reads the capture's
//! PNG as saved by the screenshot feature and returns the lines in image pixel coordinates
//! (top-left origin), in reading order. Errors are sentences the UI can show; the one error the
//! UI may want to recognise starts with `ocr-no-chinese:` (see ERR_NO_CHINESE).

use std::time::Instant;

use serde::Serialize;
use tauri::{ipc::Invoke, AppHandle};

use crate::applog;

/// Prefix of the error returned on Windows when no Chinese OCR language is installed and the
/// image looks like it holds text the installed recognizer could not read. The rest of the
/// message is a sentence for the user plus the installed recognizer languages.
pub(crate) const ERR_NO_CHINESE: &str = "ocr-no-chinese";

#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct OcrLine {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Serialize, Clone, Debug)]
pub(crate) struct OcrText {
    pub lines: Vec<OcrLine>,
    /// The recognizer language used (BCP-47 tag, e.g. "zh-Hant-HK", "en-US"; macOS: the first
    /// of the languages asked for, e.g. "zh-Hant").
    pub language: String,
}

/// Recognises the text in capture `id` (the screenshot feature's id). Runs off the main thread.
#[tauri::command]
async fn ocr_capture(app: AppHandle, id: i64) -> Result<OcrText, String> {
    let path = crate::screenshot::shot_path(&app, id).inspect_err(|e| {
        applog::error(&format!("ocr: capture {id}: {e}"));
    })?;
    applog::info(&format!("ocr: start, capture {id}"));
    let started = Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || platform::recognize(&path))
        .await
        .map_err(|e| format!("Text recognition stopped unexpectedly: {e}"))
        .and_then(|r| r);
    let ms = started.elapsed().as_millis();
    match &result {
        Ok(out) => applog::info(&format!(
            "ocr: capture {id}: {} lines, language {}, {ms} ms",
            out.lines.len(),
            out.language
        )),
        Err(e) => applog::error(&format!("ocr: capture {id} failed after {ms} ms: {e}")),
    }
    result
}

/// This feature's commands. Name each one "ocr_<name>" so lib.rs routes it here.
pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![ocr_capture]
}

// ---------- pure parts (tested below) ----------

/// Sorts lines into reading order: top to bottom, and left to right within a row. A line joins
/// the current row when its vertical centre lies inside the row's first line.
pub(crate) fn reading_order(mut lines: Vec<OcrLine>) -> Vec<OcrLine> {
    lines.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let mut rows: Vec<Vec<OcrLine>> = Vec::new();
    for line in lines {
        let centre = line.y + line.h / 2.0;
        match rows.last_mut() {
            Some(row) if centre >= row[0].y && centre <= row[0].y + row[0].h => row.push(line),
            _ => rows.push(vec![line]),
        }
    }
    rows.into_iter()
        .flat_map(|mut row| {
            row.sort_by(|a, b| a.x.total_cmp(&b.x));
            row
        })
        .collect()
}

/// Vision's box (normalised 0..1, origin bottom-left) to image pixels with a top-left origin.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn normalized_to_pixels(
    bx: f64,
    by: f64,
    bw: f64,
    bh: f64,
    width: f64,
    height: f64,
) -> (f64, f64, f64, f64) {
    (bx * width, (1.0 - by - bh) * height, bw * width, bh * height)
}

/// The Chinese recognizers to try first, in order.
#[cfg_attr(not(windows), allow(dead_code))]
const CHINESE_PREFERENCE: [&str; 3] = ["zh-Hant-HK", "zh-Hant-TW", "zh-Hans-CN"];

/// Lower-case tag with the script filled in for the short Chinese forms (zh-HK -> zh-hant-hk),
/// so "zh-HK" and "zh-Hant-HK" compare equal.
fn canonical(tag: &str) -> String {
    let t = tag.trim().replace('_', "-").to_ascii_lowercase();
    match t.as_str() {
        "zh-hk" => "zh-hant-hk".into(),
        "zh-mo" => "zh-hant-mo".into(),
        "zh-tw" => "zh-hant-tw".into(),
        "zh-cn" => "zh-hans-cn".into(),
        "zh-sg" => "zh-hans-sg".into(),
        _ => t,
    }
}

fn primary(tag: &str) -> String {
    canonical(tag).split('-').next().unwrap_or_default().to_string()
}

pub(crate) fn is_chinese(tag: &str) -> bool {
    primary(tag) == "zh"
}

/// Picks the recognizer language from the installed ones (`available`): Chinese first
/// (CHINESE_PREFERENCE), else the user's profile languages (exact tag, then same language,
/// e.g. en-GB -> en-US), else English (en-US first), else the first installed one.
/// Returns the tag as written in `available`; None when nothing is installed.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn choose_language(available: &[String], profile: &[String]) -> Option<String> {
    let find = |want: &str| {
        let want = canonical(want);
        available.iter().find(|a| canonical(a) == want).cloned()
    };
    for want in CHINESE_PREFERENCE {
        if let Some(found) = find(want) {
            return Some(found);
        }
    }
    for want in profile {
        if let Some(found) = find(want) {
            return Some(found);
        }
    }
    for want in profile {
        // Same language, other region (for Chinese, the same script too).
        let base = canonical(want);
        let script_ok = |a: &String| {
            !is_chinese(want) || canonical(a).split('-').nth(1) == base.split('-').nth(1)
        };
        if let Some(found) = available
            .iter()
            .find(|a| primary(a) == primary(want) && script_ok(a))
        {
            return Some(found.clone());
        }
    }
    find("en-US")
        .or_else(|| available.iter().find(|a| primary(a) == "en").cloned())
        .or_else(|| available.first().cloned())
}

/// The languages to ask Vision for: zh-Hant, zh-Hans, en-US, keeping only those this Mac
/// supports (`supported`, compared without case). Empty: use Vision's default.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn vision_languages(supported: &[String]) -> Vec<String> {
    ["zh-Hant", "zh-Hans", "en-US"]
        .iter()
        .filter(|want| supported.iter().any(|s| s.eq_ignore_ascii_case(want)))
        .map(|s| s.to_string())
        .collect()
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F   // CJK punctuation
        | 0x3400..=0x4DBF // Extension A
        | 0x4E00..=0x9FFF // Unified ideographs
        | 0xF900..=0xFAFF // Compatibility ideographs
        | 0xFF00..=0xFFEF // Full-width forms
        | 0x20000..=0x2FFFF)
}

/// Joins OCR words into a line: a space between words, except next to a Chinese character
/// (Windows OCR returns each Chinese character as its own word).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn join_words(words: &[String]) -> String {
    let mut out = String::new();
    for word in words.iter().filter(|w| !w.is_empty()) {
        let after_cjk = out.chars().last().is_some_and(is_cjk);
        let starts_cjk = word.chars().next().is_some_and(is_cjk);
        if !out.is_empty() && !after_cjk && !starts_cjk {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Short words that are real English, so a line like "OK" or "Go to" is not taken as noise.
#[cfg_attr(not(windows), allow(dead_code))]
const SHORT_WORDS: [&str; 30] = [
    "a", "i", "an", "am", "as", "at", "be", "by", "do", "go", "he", "if", "in", "is", "it", "me",
    "my", "no", "of", "ok", "on", "or", "so", "to", "up", "us", "we", "vs", "pm", "id",
];

/// Guess (Windows, no Chinese recognizer installed): the text may be Chinese that the installed
/// recognizer could not read. True when nothing was found, or when at least half of the words
/// look like noise (one or two letters that are not a common short word, or non-ASCII letters,
/// which is how a Latin recognizer reads Chinese characters).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn may_hold_unread_chinese(lines: &[OcrLine]) -> bool {
    let words: Vec<&str> = lines.iter().flat_map(|l| l.text.split_whitespace()).collect();
    if words.is_empty() {
        return true;
    }
    let noise = words
        .iter()
        .filter(|w| {
            let core = w.trim_matches(|c: char| c.is_ascii_punctuation());
            if core.is_empty() || core.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
            !core.is_ascii()
                || (core.chars().count() <= 2
                    && !SHORT_WORDS.contains(&core.to_ascii_lowercase().as_str()))
        })
        .count();
    noise * 2 >= words.len()
}

/// The text of the no-Chinese error, with the installed recognizer languages.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn no_chinese_error(available: &[String]) -> String {
    format!(
        "{ERR_NO_CHINESE}: Chinese text recognition isn't installed on this PC. Installed: {}.",
        if available.is_empty() {
            "none".to_string()
        } else {
            available.join(", ")
        }
    )
}

// ---------- Windows: Windows.Media.Ocr ----------

#[cfg(windows)]
mod platform {
    use windows::{
        core::HSTRING,
        Globalization::Language,
        Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap},
        Media::Ocr::OcrEngine,
        Storage::Streams::DataWriter,
        System::UserProfile::GlobalizationPreferences,
    };
    use windows_future::AsyncStatus;
    use xcap::image::imageops::{self, FilterType};

    use super::{
        choose_language, is_chinese, join_words, may_hold_unread_chinese, no_chinese_error,
        reading_order, OcrLine, OcrText,
    };
    use crate::applog;

    fn err(e: windows::core::Error) -> String {
        format!("Windows text recognition failed: {}", e.message())
    }

    fn available_languages() -> Result<Vec<String>, String> {
        let list = OcrEngine::AvailableRecognizerLanguages().map_err(err)?;
        let mut out = Vec::new();
        for i in 0..list.Size().map_err(err)? {
            out.push(list.GetAt(i).map_err(err)?.LanguageTag().map_err(err)?.to_string());
        }
        Ok(out)
    }

    fn profile_languages() -> Vec<String> {
        let Ok(list) = GlobalizationPreferences::Languages() else {
            return Vec::new();
        };
        let n = list.Size().unwrap_or(0);
        (0..n)
            .filter_map(|i| list.GetAt(i).ok().map(|t| t.to_string()))
            .collect()
    }

    pub fn recognize(path: &str) -> Result<OcrText, String> {
        let available = available_languages()?;
        let profile = profile_languages();
        applog::info(&format!(
            "ocr: installed recognizers {available:?}, profile languages {profile:?}"
        ));
        let tag = choose_language(&available, &profile)
            .ok_or("No text recognition language is installed on this PC.")?;
        applog::info(&format!("ocr: language {tag}"));
        let engine = OcrEngine::TryCreateFromLanguage(&Language::CreateLanguage(&HSTRING::from(
            tag.as_str(),
        ))
        .map_err(err)?)
        .map_err(err)?;

        let mut image = xcap::image::open(path)
            .map_err(|e| format!("Could not read the screenshot: {e}"))?
            .to_rgba8();
        // Windows OCR refuses images larger than MaxImageDimension (10000 px): scale down and
        // scale the boxes back up.
        let max = OcrEngine::MaxImageDimension().map_err(err)?.max(1);
        let mut scale = 1.0;
        if image.width() > max || image.height() > max {
            scale = max as f64 / image.width().max(image.height()) as f64;
            let w = ((image.width() as f64 * scale) as u32).max(1);
            let h = ((image.height() as f64 * scale) as u32).max(1);
            applog::info(&format!(
                "ocr: image {}x{} scaled to {w}x{h}",
                image.width(),
                image.height()
            ));
            image = imageops::resize(&image, w, h, FilterType::Triangle);
        }
        let (width, height) = image.dimensions();
        let mut bgra = image.into_raw();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let writer = DataWriter::new().map_err(err)?;
        writer.WriteBytes(&bgra).map_err(err)?;
        let buffer = writer.DetachBuffer().map_err(err)?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            width as i32,
            height as i32,
        )
        .map_err(err)?;

        // This is a worker thread: wait for the async call by polling its status.
        let op = engine.RecognizeAsync(&bitmap).map_err(err)?;
        while op.Status().map_err(err)? == AsyncStatus::Started {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let result = op.GetResults().map_err(err)?;
        let ocr_lines = result.Lines().map_err(err)?;
        let mut lines = Vec::new();
        for i in 0..ocr_lines.Size().map_err(err)? {
            let line = ocr_lines.GetAt(i).map_err(err)?;
            let words = line.Words().map_err(err)?;
            let mut texts = Vec::new();
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for j in 0..words.Size().map_err(err)? {
                let word = words.GetAt(j).map_err(err)?;
                let r = word.BoundingRect().map_err(err)?;
                x0 = x0.min(r.X as f64);
                y0 = y0.min(r.Y as f64);
                x1 = x1.max((r.X + r.Width) as f64);
                y1 = y1.max((r.Y + r.Height) as f64);
                texts.push(word.Text().map_err(err)?.to_string());
            }
            if texts.is_empty() {
                continue;
            }
            lines.push(OcrLine {
                text: join_words(&texts),
                x: x0 / scale,
                y: y0 / scale,
                w: (x1 - x0) / scale,
                h: (y1 - y0) / scale,
            });
        }
        let lines = reading_order(lines);

        if !available.iter().any(|a| is_chinese(a)) && may_hold_unread_chinese(&lines) {
            return Err(no_chinese_error(&available));
        }
        Ok(OcrText { lines, language: tag })
    }
}

// ---------- macOS: Vision ----------

#[cfg(target_os = "macos")]
mod platform {
    use objc2::{rc::autoreleasepool, AllocAnyThread};
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
    };

    use super::{normalized_to_pixels, reading_order, vision_languages, OcrLine, OcrText};
    use crate::applog;

    pub fn recognize(path: &str) -> Result<OcrText, String> {
        let (width, height) = xcap::image::image_dimensions(path)
            .map_err(|e| format!("Could not read the screenshot: {e}"))?;
        let bytes = std::fs::read(path).map_err(|e| format!("Could not read the screenshot: {e}"))?;
        autoreleasepool(|_| unsafe { run(&bytes, width as f64, height as f64) })
    }

    #[allow(deprecated, unused_unsafe)]
    unsafe fn run(bytes: &[u8], width: f64, height: f64) -> Result<OcrText, String> {
        let request = VNRecognizeTextRequest::init(VNRecognizeTextRequest::alloc());
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);

        // The class method works from macOS 10.15 (the instance one needs 12).
        let supported: Vec<String> =
            match VNRecognizeTextRequest::supportedRecognitionLanguagesForTextRecognitionLevel_revision_error(
                VNRequestTextRecognitionLevel::Accurate,
                request.revision(),
            ) {
                Ok(list) => (0..list.count())
                    .map(|i| list.objectAtIndex(i).to_string())
                    .collect(),
                Err(e) => {
                    applog::warn(&format!(
                        "ocr: could not list Vision languages: {}",
                        e.localizedDescription()
                    ));
                    Vec::new()
                }
            };
        let languages = vision_languages(&supported);
        applog::info(&format!(
            "ocr: Vision supports {supported:?}; asking for {languages:?}"
        ));
        if !languages.is_empty() {
            let tags: Vec<_> = languages.iter().map(|l| NSString::from_str(l)).collect();
            request.setRecognitionLanguages(&NSArray::from_retained_slice(&tags));
        }

        let data = NSData::with_bytes(bytes);
        let handler =
            VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &NSDictionary::new());
        let as_request: &VNRequest = &request;
        handler
            .performRequests_error(&NSArray::from_slice(&[as_request]))
            .map_err(|e| format!("macOS text recognition failed: {}", e.localizedDescription()))?;

        let mut lines = Vec::new();
        if let Some(results) = request.results() {
            for i in 0..results.count() {
                let observation = results.objectAtIndex(i);
                let Some(best) = observation.topCandidates(1).firstObject() else {
                    continue;
                };
                let text = best.string().to_string();
                if text.trim().is_empty() {
                    continue;
                }
                let b = observation.boundingBox();
                let (x, y, w, h) = normalized_to_pixels(
                    b.origin.x as f64,
                    b.origin.y as f64,
                    b.size.width as f64,
                    b.size.height as f64,
                    width,
                    height,
                );
                lines.push(OcrLine { text, x, y, w, h });
            }
        }
        Ok(OcrText {
            lines: reading_order(lines),
            language: languages.first().cloned().unwrap_or_else(|| "en-US".into()),
        })
    }
}

// ---------- elsewhere ----------

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    pub fn recognize(_path: &str) -> Result<super::OcrText, String> {
        Err("Text recognition isn't available on this system.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f64, y: f64, w: f64, h: f64) -> OcrLine {
        OcrLine { text: text.into(), x, y, w, h }
    }

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn converts_vision_boxes_to_top_left_pixels() {
        // A box in the top-left quarter of a 200x100 image.
        assert_eq!(
            normalized_to_pixels(0.0, 0.5, 0.5, 0.5, 200.0, 100.0),
            (0.0, 0.0, 100.0, 50.0)
        );
        // A box at the bottom: y = (1 - 0.1 - 0.2) * 100 = 70.
        let (x, y, w, h) = normalized_to_pixels(0.25, 0.1, 0.5, 0.2, 400.0, 100.0);
        assert_eq!((x, w), (100.0, 200.0));
        assert!((y - 70.0).abs() < 1e-9 && (h - 20.0).abs() < 1e-9);
    }

    #[test]
    fn orders_lines_top_to_bottom_then_left_to_right() {
        let lines = vec![
            line("second row right", 300.0, 52.0, 100.0, 20.0),
            line("first", 10.0, 10.0, 100.0, 20.0),
            line("second row left", 10.0, 50.0, 100.0, 20.0),
            line("third", 10.0, 100.0, 100.0, 20.0),
            line("first right, a bit higher", 200.0, 8.0, 100.0, 24.0),
        ];
        let order: Vec<String> = reading_order(lines).into_iter().map(|l| l.text).collect();
        assert_eq!(
            order,
            ["first", "first right, a bit higher", "second row left", "second row right", "third"]
        );
        assert!(reading_order(Vec::new()).is_empty());
    }

    #[test]
    fn prefers_chinese_in_order() {
        let available = tags(&["en-US", "zh-Hans-CN", "zh-Hant-TW"]);
        assert_eq!(choose_language(&available, &tags(&["en-US"])).as_deref(), Some("zh-Hant-TW"));
        // Short forms count as the same language.
        let available = tags(&["en-US", "zh-CN", "zh-HK"]);
        assert_eq!(choose_language(&available, &[]).as_deref(), Some("zh-HK"));
        let available = tags(&["en-US", "zh-Hans-CN"]);
        assert_eq!(choose_language(&available, &[]).as_deref(), Some("zh-Hans-CN"));
    }

    #[test]
    fn falls_back_to_profile_then_english() {
        let available = tags(&["en-US", "fr-FR", "tl"]);
        assert_eq!(choose_language(&available, &tags(&["fr-FR", "en-US"])).as_deref(), Some("fr-FR"));
        // Same language, other region.
        assert_eq!(choose_language(&available, &tags(&["fr-CA"])).as_deref(), Some("fr-FR"));
        assert_eq!(choose_language(&available, &tags(&["de-DE"])).as_deref(), Some("en-US"));
        assert_eq!(choose_language(&tags(&["en-GB", "de-DE"]), &[]).as_deref(), Some("en-GB"));
        assert_eq!(choose_language(&tags(&["de-DE"]), &[]).as_deref(), Some("de-DE"));
        assert_eq!(choose_language(&[], &tags(&["en-US"])), None);
    }

    #[test]
    fn keeps_supported_vision_languages() {
        let supported = tags(&["en-US", "fr-FR", "zh-Hans", "zh-Hant"]);
        assert_eq!(vision_languages(&supported), ["zh-Hant", "zh-Hans", "en-US"]);
        assert_eq!(vision_languages(&tags(&["en-US", "fr-FR"])), ["en-US"]);
        assert!(vision_languages(&[]).is_empty());
    }

    #[test]
    fn joins_chinese_without_spaces() {
        assert_eq!(join_words(&tags(&["香", "港", "OK"])), "香港OK");
        assert_eq!(join_words(&tags(&["Hello", "world"])), "Hello world");
        assert_eq!(join_words(&tags(&["Price", "價", "格", "HK$10"])), "Price價格HK$10");
    }

    #[test]
    fn guesses_unread_chinese() {
        assert!(may_hold_unread_chinese(&[]));
        assert!(may_hold_unread_chinese(&[line("ä¸ Ei 'Ll fE", 0.0, 0.0, 1.0, 1.0)]));
        assert!(!may_hold_unread_chinese(&[line("Go to Settings and click OK", 0.0, 0.0, 1.0, 1.0)]));
        assert!(!may_hold_unread_chinese(&[line("Total 12 34 HK$500", 0.0, 0.0, 1.0, 1.0)]));
    }

    /// Real Vision run on a committed 560x180 PNG with two lines:
    /// "報價單 Q-2311" above "Unit price HK$12.5".
    #[cfg(target_os = "macos")]
    #[test]
    fn vision_reads_the_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ocr-sample.png");
        let out = platform::recognize(path).expect("Vision OCR failed");
        println!("ocr fixture: language {}", out.language);
        for l in &out.lines {
            println!("ocr fixture: {:?} at {:.0},{:.0} {:.0}x{:.0}", l.text, l.x, l.y, l.w, l.h);
        }
        assert_eq!(out.language, "zh-Hant");
        let find = |needle: &str| {
            out.lines
                .iter()
                .find(|l| l.text.contains(needle))
                .unwrap_or_else(|| panic!("{needle:?} not found in {:?}", out.lines))
        };
        let quote = find("Q-2311");
        assert!(quote.text.contains("報價單"), "Chinese not read: {:?}", quote.text);
        let price = find("Unit price");
        assert!(price.text.contains("12.5"), "price not read: {:?}", price.text);
        // Top-left pixel boxes: the quote line is above the price line, both inside the image.
        assert!(quote.y + quote.h <= price.y + 1.0, "{quote:?} should be above {price:?}");
        for l in [quote, price] {
            assert!(l.x >= 0.0 && l.y >= 0.0 && l.x + l.w <= 561.0 && l.y + l.h <= 181.0, "{l:?}");
            assert!(l.x > 10.0 && l.x < 60.0, "left edge near 30 px: {l:?}");
        }
        assert!(quote.y < 60.0 && price.y > 80.0, "rows near 30 and 100 px: {quote:?} {price:?}");
    }

    #[test]
    fn no_chinese_error_lists_languages() {
        let e = no_chinese_error(&tags(&["en-US"]));
        assert!(e.starts_with("ocr-no-chinese: Chinese text recognition isn't installed on this PC."));
        assert!(e.ends_with("Installed: en-US."));
        assert!(no_chinese_error(&[]).ends_with("Installed: none."));
    }
}
