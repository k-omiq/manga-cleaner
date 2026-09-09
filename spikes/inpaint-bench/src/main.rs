//! Two questions about the inpainter, measured rather than reasoned about.
//!
//! **Is a persistent session worth holding?** A session is built once and used
//! for every region of every page, so its build cost amortises - but only if it
//! actually amortises. This runs a long sequence through one session and reports
//! whether the per-run cost drifts, against the cost of rebuilding per run.
//!
//! **Does a cropped ROI help?** manga-LaMa's input is **fixed** at
//! `[batch, 3, 512, 512]` - spike 6's probe, and the export's own shape. There
//! is no smaller input to give it: a region of 160×290 is padded to 512 with
//! real surrounding page pixels, and that is already the smallest run the
//! model has. The dimension that *is*
//! dynamic is `batch`, so the question becomes whether putting several regions
//! through one call beats putting them through several - which is the same
//! optimisation, asked of the axis the model actually exposes.
//!
//! ```text
//! spike-inpaint-bench [--ep cpu|webgpu|coreml] [--runs N] [--batches 1,2,4,8]
//! ```

use std::path::Path;
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use cleaner_core::accel::{Accelerator, Preference};

const SIDE: usize = 512;

fn main() -> Result<()> {
    let mut argv = std::env::args().skip(1);
    let mut ep = "cpu".to_owned();
    let mut runs = 20usize;
    let mut batches = vec![1usize, 2, 4, 8];
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--ep" => ep = argv.next().ok_or_else(|| anyhow!("--ep needs a provider"))?,
            "--runs" => runs = argv.next().ok_or_else(|| anyhow!("--runs needs a count"))?.parse()?,
            "--batches" => {
                batches = argv
                    .next()
                    .ok_or_else(|| anyhow!("--batches needs a list"))?
                    .split(',')
                    .map(|s| s.trim().parse::<usize>())
                    .collect::<Result<_, _>>()?
            }
            other => bail!("unexpected argument {other}"),
        }
    }

    cleaner_core::runtime::load(&cleaner_core::runtime::find(None)?)?;
    let accelerator = named(&ep)?;
    let model = Path::new("models/lama-manga.onnx");

    println!("provider: {accelerator:?}");
    println!("available: {:?}", cleaner_core::accel::available());

    // ── what the session costs to build ────────────────────────────────────
    let mut builds = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let (session, selection) = cleaner_core::accel::open_session(
            model,
            &cleaner_core::accel::LAMA,
            Preference::Force(accelerator),
            Some(cleaner_core::constants::LAMA_INTRA_THREADS),
        )?;
        builds.push(started.elapsed().as_secs_f64() * 1000.0);
        if selection.accelerator != accelerator {
            return Err(anyhow!("{accelerator:?} was not usable: {:?}", selection.declined));
        }
        drop(session);
    }
    builds.sort_by(f64::total_cmp);
    println!("\nsession build: {:.0} / {:.0} / {:.0} ms (three cold builds)", builds[0], builds[1], builds[2]);

    // ── the persistent session ─────────────────────────────────────────────
    let (mut session, _) = cleaner_core::accel::open_session(
        model,
        &cleaner_core::accel::LAMA,
        Preference::Force(accelerator),
        Some(cleaner_core::constants::LAMA_INTRA_THREADS),
    )?;

    println!("\n── batch scaling, one persistent session");
    println!("{:<7} {:>10} {:>12} {:>10}", "batch", "per call", "per region", "vs batch 1");
    let mut baseline = 0f64;
    for &batch in &batches {
        let timings = time_runs(&mut session, batch, runs.max(4))?;
        let per_call = median(&timings);
        let per_region = per_call / batch as f64;
        if batch == batches[0] {
            baseline = per_region;
        }
        println!(
            "{:<7} {:>9.0}ms {:>11.0}ms {:>9.2}×",
            batch,
            per_call,
            per_region,
            baseline / per_region
        );
    }

    // ── drift over a long sequence ─────────────────────────────────────────
    // A chapter is ~200 pages and a page ~5 regions, so a session is asked for
    // a thousand runs. If it degrades, holding it is the wrong design.
    let long = runs.max(40);
    println!("\n── drift over {long} sequential runs at batch 1");
    let timings = time_runs(&mut session, 1, long)?;
    let head = median(&timings[..long / 4]);
    let tail = median(&timings[long - long / 4..]);
    println!("first quarter {head:.0} ms, last quarter {tail:.0} ms, drift {:+.1}%", (tail - head) / head * 100.0);

    // ── rebuilding per run, for contrast ───────────────────────────────────
    println!("\n── rebuilding the session for each run");
    let mut rebuilt = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let (mut fresh, _) = cleaner_core::accel::open_session(
            model,
            &cleaner_core::accel::LAMA,
            Preference::Force(accelerator),
            Some(cleaner_core::constants::LAMA_INTRA_THREADS),
        )?;
        run_once(&mut fresh, 1)?;
        rebuilt.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    rebuilt.sort_by(f64::total_cmp);
    println!("build + one run: {:.0} ms (median of three)", rebuilt[1]);

    Ok(())
}

fn named(name: &str) -> Result<Accelerator> {
    use Accelerator::*;
    Ok(match name {
        "cpu" => Cpu,
        "coreml" => CoreMl,
        "directml" | "dml" => DirectMl,
        "cuda" => Cuda,
        "webgpu" => WebGpu,
        other => bail!("unknown provider {other}"),
    })
}

fn time_runs(session: &mut ort::session::Session, batch: usize, runs: usize) -> Result<Vec<f64>> {
    run_once(session, batch)?; // warm
    let mut timings = Vec::with_capacity(runs);
    for _ in 0..runs {
        let started = Instant::now();
        run_once(session, batch)?;
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(timings)
}

/// One inference over `batch` crops. Values are mid-range rather than zero: a
/// tensor of zeros can take a different path through sparsity-aware kernels.
fn run_once(session: &mut ort::session::Session, batch: usize) -> Result<()> {
    let image: Vec<f32> = (0..batch * 3 * SIDE * SIDE).map(|i| ((i % 251) as f32) / 251.0).collect();
    // A mask covering the middle third, which is the shape of a real text
    // region inside its padded crop.
    let mut mask = vec![0f32; batch * SIDE * SIDE];
    for b in 0..batch {
        for y in SIDE / 3..2 * SIDE / 3 {
            for x in SIDE / 3..2 * SIDE / 3 {
                mask[b * SIDE * SIDE + y * SIDE + x] = 1.0;
            }
        }
    }
    let image = ort::value::Tensor::from_array(([batch, 3, SIDE, SIDE], image))?;
    let mask = ort::value::Tensor::from_array(([batch, 1, SIDE, SIDE], mask))?;
    let _ = session.run(ort::inputs![image, mask])?;
    Ok(())
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}
