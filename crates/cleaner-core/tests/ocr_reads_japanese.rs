//! The rescue reader against the real weights, on a real balloon.
//!
//! Everything else about [`cleaner_core::gate::ocr`] is unit-tested without a
//! model: the squish, the vocabulary, the log-probability, the share, and the
//! whole rescue decision. What none of that can tell you is whether the tensor
//! names are right, whether the preprocessing matches what the export was
//! trained on, and whether a page this application decodes produces text a
//! human would recognise. One end-to-end read answers all three, and nothing
//! else does.
//!
//! ## Why it skips instead of failing
//!
//! Two things it needs are deliberately not in the repository:
//!
//! - **The weights.** 460 MB, optional, fetched by `scripts/fetch-models.sh`.
//!   No model is vendored, for licensing reasons.
//! - **A page with real Japanese lettering on it.** `fixtures/pages/` is
//!   synthetic - it has the *shapes* of text, which is all the detector and the
//!   fitter need and is nothing at all to a reader. Real scans are the user's
//!   own and cannot be committed.
//!
//! Rust cannot make `#[ignore]` conditional, so the skip is an early return
//! that says which file was missing. Point it at a page with
//! `MANGA_CLEANER_OCR_PAGE` and its balloon with `MANGA_CLEANER_OCR_RECT`
//! (`x,y,w,h`); the defaults are the page and region the measurement was
//! recorded on.

use std::path::{Path, PathBuf};

use cleaner_core::accel::Preference;
use cleaner_core::gate::{Ocr, cjk_share};
use cleaner_core::image::decode;
use cleaner_core::mask::Rect;

const ENCODER: &str = "manga-ocr-encoder_model.onnx";
const DECODER: &str = "manga-ocr-decoder_model.onnx";
const VOCAB: &str = "manga-ocr-vocab.txt";

/// The balloon the rescue was measured on: 05.png at 511,125, the one the
/// script identifier returns `Uncertain` on. The rectangle is the region's
/// `text_bounds`, which is what the gate hands the reader.
const DEFAULT_RECT: Rect = Rect { x: 516, y: 130, w: 198, h: 191 };

fn models_dir() -> PathBuf {
    std::env::var_os("MANGA_CLEANER_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models"))
}

fn page_path() -> Option<PathBuf> {
    if let Some(page) = std::env::var_os("MANGA_CLEANER_OCR_PAGE") {
        return Some(PathBuf::from(page));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("dev/120 noisy png/05.png"))
}

fn rect() -> Rect {
    let Some(spec) = std::env::var_os("MANGA_CLEANER_OCR_RECT") else { return DEFAULT_RECT };
    let spec = spec.to_string_lossy().into_owned();
    let parts: Vec<&str> = spec.split(',').collect();
    if parts.len() != 4 {
        panic!("MANGA_CLEANER_OCR_RECT wants x,y,w,h, got {spec}");
    }
    Rect::new(
        parts[0].trim().parse().expect("x"),
        parts[1].trim().parse().expect("y"),
        parts[2].trim().parse().expect("w"),
        parts[3].trim().parse().expect("h"),
    )
}

fn skip(what: &Path) {
    eprintln!("skipping: {} is not here - see this file's docs", what.display());
}

#[test]
fn the_reader_reads_a_real_balloon_as_japanese() {
    let models = models_dir();
    for name in [ENCODER, DECODER, VOCAB] {
        if !models.join(name).exists() {
            skip(&models.join(name));
            return;
        }
    }
    let Some(page_path) = page_path() else { return };
    if !page_path.exists() {
        skip(&page_path);
        return;
    }
    let Ok(runtime) = cleaner_core::runtime::find(None) else {
        eprintln!("skipping: no ONNX Runtime - run scripts/fetch-runtime.sh");
        return;
    };
    if cleaner_core::runtime::load(&runtime).is_err() {
        eprintln!("skipping: the ONNX Runtime at {} would not load", runtime.display());
        return;
    }

    let bytes = std::fs::read(&page_path).expect("the page");
    let page = decode(&bytes).expect("the page decodes");
    let mut ocr = Ocr::open(
        &models.join(ENCODER),
        &models.join(DECODER),
        &models.join(VOCAB),
        Preference::Automatic,
    )
    .expect("the reader opens");

    let reading = ocr.read(&page, rect()).expect("the reader reads");
    let share = cjk_share(&reading.text);
    eprintln!("read {:?} share {share:.2} score {:.2}", reading.text, reading.score);

    // The rescue's own two conditions, on the region it was built for. Not the
    // exact characters: pinning a transcription would make this a test of the
    // model's weights rather than of this crate's use of them, and it would go
    // red on a re-export that reads the balloon just as well.
    assert!(
        reading.text.chars().count() >= 2,
        "the reader said nothing on a balloon full of dialogue: {:?}",
        reading.text
    );
    assert!(
        share >= 0.6,
        "the reading is not Japanese, which means the preprocessing or the tensor names are wrong: {:?}",
        reading.text
    );
    // A greedy decode that never meets its end-of-sequence token runs to the
    // cap and comes back as a wall of repeats. The balloon is a few lines.
    assert!(
        reading.text.chars().count() < 64,
        "the decode ran to the cap, which is what a broken stop condition looks like: {:?}",
        reading.text
    );
    assert!(reading.score <= 0.0 && reading.score.is_finite(), "score {}", reading.score);
}
