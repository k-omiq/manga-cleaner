//! Phase 1's first open question: does `ogkalu/image-script-identification`
//! answer correctly on a manga text crop?
//!
//! It was chosen as the only candidate with vertical-CJK labels. Its model
//! card is empty, plainly - the whole card is three lines of front matter -
//! so the input convention had to come from the only
//! implementation that uses it, `comic-translate`'s `script_detection.py`, and
//! the convention is not one anybody would guess:
//!
//! - The crop is **one text line**, not the whole block, and a vertical line is
//!   rotated 90° counter-clockwise first.
//! - Bicubic resize to 48 high, then Tesseract's own black/white
//!   normalisation - local minima and maxima along the **middle row only**,
//!   `ile(0.25)` and `ile(0.75)` over those histograms, then
//!   `(grey − black) / ((white − black) / 2) − 1`.
//! - A crop whose median is under 64 is **inverted** first, so white-on-black
//!   text reaches the model in the polarity it was trained on.
//! - The output is **CTC**, and class 2 - labelled `Broken` - is the blank.
//!   Averaging the score field, the obvious thing to do, returns `Broken` for
//!   every input in every value range, which is how this spike started.
//!
//! ```text
//! spike-script-id <page.png> <x> <y> <w> <h> [<x> <y> <w> <h> ...]
//! ```

use anyhow::{Context, Result, anyhow};
use cleaner_core::image::{Raster, decode};

const STRIP_HEIGHT: usize = 48;
/// Unicharset index 0. An empty answer, not a script.
const NULL_CHAR: usize = 0;
/// Unicharset index 2, labelled `Broken`. The CTC blank.
const BLANK_CHAR: usize = 2;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let page_path = args
        .next()
        .ok_or_else(|| anyhow!("usage: spike-script-id <page.png> <x> <y> <w> <h>..."))?;
    let numbers: Vec<i64> = args.map(|a| a.parse::<i64>()).collect::<Result<_, _>>()?;
    if numbers.is_empty() || numbers.len() % 4 != 0 {
        return Err(anyhow!("boxes come in fours: x y w h"));
    }

    let dylib = cleaner_core::runtime::find(None).context("locating ONNX Runtime")?;
    cleaner_core::runtime::load(&dylib)?;

    let labels = labels()?;
    let mut session = ort::session::Session::builder()?
        .commit_from_file("models/image-script-identification-osd_lstm.onnx")
        .context("loading the script-ID model")?;

    let page = decode(&std::fs::read(&page_path)?).map_err(|e| anyhow!("{e}"))?;

    for chunk in numbers.chunks(4) {
        let (x, y, w, h) = (chunk[0], chunk[1], chunk[2] as u32, chunk[3] as u32);
        println!("\n########## crop {x},{y} {w}×{h}");
        for rotate in [false, true] {
            let (strip, sw) = preprocess(&page, x, y, w, h, rotate);
            if sw < 3 {
                println!("  rotate={rotate:<5} too narrow after scaling");
                continue;
            }
            let input = ort::value::Tensor::from_array(([1usize, 1, STRIP_HEIGHT, sw], strip))?;
            let outputs = session.run(ort::inputs![input])?;
            let (shape, scores) = outputs["scores"].try_extract_tensor::<f32>()?;
            let classes = shape[shape.len() - 1] as usize;

            let (verdict, tally) = dominant_script(scores, classes, &labels);
            println!(
                "  rotate={:<5} width={:<4} → {:<16} {}",
                rotate,
                sw,
                if verdict.is_empty() { "(nothing)" } else { &verdict },
                tally
            );
        }
    }
    Ok(())
}

fn labels() -> Result<Vec<String>> {
    let raw = std::fs::read_to_string("models/image-script-identification-osd_labels.json")?;
    Ok(raw
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_owned())
        .collect())
}

/// CTC best path: argmax per timestep, collapse repeats, drop the blank, and
/// take the most frequent label that remains.
fn dominant_script(scores: &[f32], classes: usize, labels: &[String]) -> (String, String) {
    let steps = scores.len() / classes;
    let mut counts: Vec<(usize, usize)> = Vec::new();
    let mut previous = BLANK_CHAR;

    for t in 0..steps {
        let row = &scores[t * classes..(t + 1) * classes];
        let best = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap_or(BLANK_CHAR);
        if best != BLANK_CHAR && best != previous {
            match counts.iter_mut().find(|(i, _)| *i == best) {
                Some((_, n)) => *n += 1,
                None => counts.push((best, 1)),
            }
        }
        previous = best;
    }

    counts.sort_by(|a, b| b.1.cmp(&a.1));
    let tally = counts
        .iter()
        .take(4)
        .map(|(i, n)| format!("{}×{n}", labels.get(*i).map_or("?", |s| s.as_str())))
        .collect::<Vec<_>>()
        .join(" ");

    let verdict = counts
        .first()
        .filter(|(i, _)| *i != NULL_CHAR)
        .and_then(|(i, _)| labels.get(*i).cloned())
        .unwrap_or_default();
    (verdict, tally)
}

/// `(1, 1, 48, W)` in the convention above.
fn preprocess(page: &Raster, x: i64, y: i64, w: u32, h: u32, rotate: bool) -> (Vec<f32>, usize) {
    // Rotate first, then scale: rotating after would scale the wrong axis to 48.
    let (src_w, src_h) = if rotate { (h, w) } else { (w, h) };
    let scale = STRIP_HEIGHT as f32 / src_h as f32;
    let out_w = ((src_w as f32 * scale).round() as usize).max(1);

    let mut grey = vec![0u8; STRIP_HEIGHT * out_w];
    for oy in 0..STRIP_HEIGHT {
        for ox in 0..out_w {
            let sx = (ox as f32 + 0.5) / scale - 0.5;
            let sy = (oy as f32 + 0.5) / scale - 0.5;
            // `np.rot90(m, k=1)` is counter-clockwise, and for a source of
            // W×H it means `out[i, j] = m[j, W − 1 − i]`. Written out with
            // this module's names - `src_h` is the rotated height and so the
            // *original width* - the output pixel (sx, sy) reads the original
            // at row `sx`, column `src_h − 1 − sy`.
            //
            // Getting this backwards is not a subtle failure and it is not a
            // loud one either: the mirrored rotation returns `Hangul_vert` and
            // `Fraktur` on vertical Japanese, confidently, with the same shape
            // of output. It cost an hour here.
            let (cx, cy) = if rotate {
                (src_h as f32 - 1.0 - sy, sx)
            } else {
                (sx, sy)
            };
            let px = (x as f32 + cx).clamp(0.0, (page.width - 1) as f32) as u32;
            let py = (y as f32 + cy).clamp(0.0, (page.height - 1) as f32) as u32;
            grey[oy * out_w + ox] = (page.luma16_at(px, py) >> 8) as u8;
        }
    }

    // White text on a dark ground is inverted to the polarity the model saw in
    // training. Median over the whole strip, as upstream.
    let mut sorted = grey.clone();
    sorted.sort_unstable();
    if (sorted[sorted.len() / 2] as f32) < 64.0 {
        for value in grey.iter_mut() {
            *value = 255 - *value;
        }
    }

    let (black, white) = black_white(&grey, out_w, STRIP_HEIGHT);
    let mut contrast = (white - black) / 2.0;
    if contrast <= 0.0 {
        contrast = 1.0;
    }
    let normalised = grey
        .iter()
        .map(|v| (*v as f32 - black) / contrast - 1.0)
        .collect();
    (normalised, out_w)
}

/// Tesseract `networkio.cpp` `ComputeBlackWhite`, over the middle row only.
fn black_white(grey: &[u8], w: usize, h: usize) -> (f32, f32) {
    let mut mins = [0i64; 256];
    let mut maxes = [0i64; 256];
    if w >= 3 {
        let row = &grey[(h / 2) * w..(h / 2 + 1) * w];
        for x in 1..w - 1 {
            let (previous, current, next) = (row[x - 1], row[x], row[x + 1]);
            if (current < previous && current <= next) || (current <= previous && current < next) {
                mins[current as usize] += 1;
            }
            if (current > previous && current >= next) || (current >= previous && current > next) {
                maxes[current as usize] += 1;
            }
        }
    }
    if mins.iter().sum::<i64>() == 0 {
        mins[0] = 1;
    }
    if maxes.iter().sum::<i64>() == 0 {
        maxes[255] = 1;
    }
    (ile(&mins, 0.25), ile(&maxes, 0.75))
}

/// Tesseract `STATS::ile`, over 256 buckets.
fn ile(buckets: &[i64; 256], fraction: f32) -> f32 {
    let total: i64 = buckets.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let target = (fraction * total as f32).clamp(1.0, total as f32);
    let mut sum = 0i64;
    let mut index = 0usize;
    while index <= 255 && (sum as f32) < target {
        sum += buckets[index];
        index += 1;
    }
    if index > 0 {
        index as f32 - (sum as f32 - target) / buckets[index - 1] as f32
    } else {
        0.0
    }
}
