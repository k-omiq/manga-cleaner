//! Rung 3a - FLUX, through the sidecar.
//!
//! The other model rungs hold an `ort::Session` and run a tensor. This one
//! holds a **process** and runs an HTTP request, and almost everything else
//! about it is the same: it declines for the same reasons in the same
//! vocabulary, it writes through the same isolation ramp, it produces the same
//! [`Rendered`], and [`crate::quality::assess`] scores its output exactly as it
//! scores rung 2's. That is deliberate. A rung whose result is a different kind
//! of thing is a rung the review row cannot compare.
//!
//! ## No mask reaches the model, so our composite is the boundary
//!
//! §4a's candidate table is a table about one axis - *whether a mask reaches
//! the model at all* - and the answer for every model this rung can actually
//! use is no. `FLUX.1-Fill-dev` has a native mask channel and a non-commercial
//! licence; `FLUX.2 [klein]` is Apache-2.0 and edits **by reference image**,
//! with no Klein fill checkpoint in existence; `Z-Image-Edit` is unreleased.
//!
//! §5.2 already faced this for the cloud rung and answered it, and the answer
//! transfers whole: *"a mask sent as a reference image is advisory"*. So the
//! model is handed a crop and regenerates all of it, the sidecar is told in as
//! many words not to composite against the hint, and **the boundary is
//! enforced here** - [`crate::engines::model::AlphaRamp`] over the applied
//! mask, exactly as rungs 2 and 3 enforce it, with everything outside copied
//! from the page unchanged. §6's decline metric is what catches the failures
//! that get past that, and honestly it does not fire on the
//! failure this corpus contains.
//!
//! What is new relative to §5.2 is only that the choice is ours rather than a
//! provider's, and that the one candidate which *does* take a mask is the one
//! whose licence we cannot recommend.
//!
//! ## One crop, never a tiling
//!
//! Rungs 2 and 3 tile at 512² with 128 px of overlap because their graphs have
//! a fixed or near-fixed spatial input. This rung's model does not: it edits
//! whatever crop it is given, and running it twice over one region would mean
//! two independent regenerations of the same screentone joined by a cross-fade
//! - the failure [`crate::engines::model::tiles`] exists to avoid, with no
//! running-composite trick available to avoid it, because the model rewrites
//! the context as well as the hole.
//!
//! So a region is one crop, and [`crop_for`] sizes it. Regions past
//! [`MAX_BOX`] decline as they do on every other model rung, which is what
//! keeps the crop bounded.
//!
//! ## The crop is tight, and the *sidecar* owns the working resolution
//!
//! Where rungs 2 and 3 have a fixed spatial input, this one has none - and a
//! FLUX.2 edit resolves badly on a hole that is small in its frame. Measured on
//! `fixtures/pages/page-sfx.png`: a 40×140 balloon region inside the 512²-floored
//! crop this rung used to send came back as a halftone smudge painted over the
//! text, and the same region inside a crop sized to itself came back clean.
//!
//! The fix is split across the seam because each half belongs where it is.
//! [`crop_for`] sends the *page's own pixels* at the page's own scale, tightly -
//! resampling on this side is exactly what must be avoided. Upscaling to a working resolution and back is
//! the model's business, so it happens in the sidecar (`backend/mflux.py`), and
//! the reply still arrives at exactly the geometry this rung asked for - which
//! is what [`crate::sidecar::Client::render`] checks and refuses.
//!
//! ## Eight bits, out and back
//!
//! The wire is `rgb8` ([`crate::sidecar::wire`] says why it is raw samples and
//! not a PNG), so a 16-bit page cannot round-trip through this rung any more
//! than it can through rung 3's uint8 tensors - and it declines under the same
//! [`Decline::DepthBeyondEngine`], which is the shared vocabulary doing its
//! job. Everything else about the page's mode, depth, palette and profile
//! never leaves this process.

use crate::engines::model::{
    self, AlphaRamp, Decline, Error, MAX_BOX, Rendered, applied_mask, ceiling_for,
    dither_at, page_crop, source_channel,
};
use crate::fit::Fitted;
use crate::image::{BitDepth, ColorMode, Raster};
use crate::mask::Rect;
use crate::sidecar::hardware::{self, Budget, Demand, Verdict};
use crate::sidecar::wire::{Encoding, Image, RenderRequest};
use crate::sidecar::{Backend, Client, Floor, Install, Sidecar};
use crate::strip::window::EdgePad;

pub use crate::engines::model::write_bound;

/// What the instruction to the model says. **One prompt, and it is an
/// instruction rather than a content description.**
///
/// This rung shipped two content-description prompts - a screentone-flavoured
/// one for grey pages and a colour one for the rest - on the argument that
/// negative phrasing activates text concepts in the text encoder. The sweep
/// took that argument apart, and the measurement is the reason this const is
/// one line:
///
/// * On the sound effect over screentone, the content prompt left the patch
///   **20 grey levels darker** than the page's own tone with **24 levels of
///   excess variance** - a visible dark blotch - where this instruction landed
///   within **2 levels of mean and 1 of variance**.
/// * On the balloon, the content prompt was clean **at `SEED` 1 and at no other
///   seed tried**: seeds 2–5 came back 35 to 94 levels dark with invented
///   texture. This instruction held within ±6 levels at every one of the five.
///   A prompt whose only good result is the seed we happen to have frozen is a
///   prompt that has not worked.
///
/// The colourspace split went with it, and its own premise is why: the split
/// existed because "Japanese/screentone vocabulary drifts colour pages", and an
/// instruction carrying no such vocabulary has nothing to drift. No colour
/// page was ever run under *either* scheme.
///
/// Held here rather than in the sidecar so that changing it is a change to this
/// repository, reviewable in a diff, and not an edit to a file the user
/// installed.
pub const PROMPT: &str = "Remove all text.";

/// Denoising steps. Four, which is the distilled Klein models' own range
/// (1–12) at the fast end of it - this rung already costs ten to sixty seconds
/// a region and it is reached one region at a time by hand.
///
/// **Swept**: eight was tried against four at the winning prompt and
/// was not better on either fixture region - within 2 grey levels on the sound
/// effect and 9 levels *worse* on the balloon - for twice the wall clock.
pub const STEPS: u32 = 4;

/// The seed, fixed. A rung whose output cannot be reproduced cannot be
/// reviewed, and provenance is
/// written to be enough to reproduce a patch or to say that it cannot be.
///
/// It is no longer *load-bearing*, which is a different claim and a new one:
/// [`PROMPT`]'s sweep varied this across five values and the spread it found
/// was ±7 grey levels, where the prompt it replaced swung 100 across the same
/// five.
pub const SEED: u64 = 1;

/// Guidance. 1.0 - the distilled models are trained without classifier-free
/// guidance and a higher figure buys contrast the tone gate then has to undo.
///
/// **Swept**, and the argument above is what the measurement found:
/// 2.5 and 3.5 both replaced a flat white balloon with invented halftone
/// texture 60 grey levels dark, and both cost roughly twice the wall clock
/// because a guidance over 1.0 is a second forward pass per step. The
/// reference implementation's 2.5 belongs to its **FLUX.1 Kontext** inpainter;
/// its FLUX.2 Klein inpainter - the one holding the model this rung holds -
/// fixes guidance at 1.0 as well.
pub const GUIDANCE: f32 = 1.0;

/// The crop's dimensions are snapped up to a multiple of this.
///
/// Sixteen, because a latent-space model works in a multiple of its patch and
/// VAE downsampling factor, and a crop that is not one gets silently resized by
/// something - which is the resample §5.2 step 4 permits only as a last resort
/// with a named filter. Snapping here means the resize never happens.
pub const LATENT_STRIDE: u32 = 16;

/// How much real page the crop carries around the region, per side: half the
/// region's long side, clamped between [`CONTEXT_MIN`] and [`CONTEXT_MAX`].
///
/// **The context is proportional to the region, and it is tight.** An absolute
/// 64 px floored at a 512² crop - which is what this rung shipped - puts a line
/// of dialogue forty pixels wide in the middle of half a megapixel of
/// screentone, and a FLUX.2 edit at that framing does not remove the text, it
/// paints the surround *over* it: measured on `fixtures/pages/page-sfx.png`, a
/// 40×140 balloon region came back as a halftone smudge in the middle of a white
/// balloon. The same region at this contract came back clean. Half the long side
/// is the reference implementation's own figure, and the clamp is its clamp.
pub fn context_for(bounds: Rect) -> u32 {
    (bounds.w.max(bounds.h) / 2).clamp(CONTEXT_MIN, CONTEXT_MAX)
}

/// The least context a crop carries, per side.
///
/// Twenty-four. §5.2's tone fit and §6's surround annulus are sampled from the
/// **page** and not from this crop, so what this number has to buy is only the
/// model's own sense of what surrounds the hole - and for a region small enough
/// to hit this floor, twenty-four pixels is most of the region again.
pub const CONTEXT_MIN: u32 = 24;

/// The most context a crop carries, per side.
///
/// Eighty. Past here the surround stops telling the model anything new about
/// the hole and starts costing it resolution, because the crop is upscaled to a
/// fixed working long side before the edit and every pixel of context is a pixel
/// the glyphs do not get.
pub const CONTEXT_MAX: u32 = 80;

/// Whether this rung can run on this page at all.
///
/// Rung 3's answer, for rung 3's reason one protocol over: the wire is eight
/// bits per sample, so a 16-bit source has no path through here.
pub fn applies(page: &Raster) -> bool {
    model::applies(page) && page.depth != BitDepth::Sixteen
}

/// Every reason this region would be declined, before a process is spawned.
pub fn declines(page: &Raster, fitted: &Fitted) -> Option<Decline> {
    if let Some(decline) = model::declines(page, fitted) {
        return Some(decline);
    }
    (page.depth == BitDepth::Sixteen).then_some(Decline::DepthBeyondEngine(page.depth))
}

/// The crop the model is shown, in page coordinates.
///
/// The region's own bounds, grown by [`context_for`] on every side and snapped
/// up to [`LATENT_STRIDE`]. **No floor at [`crate::engines::model::MODEL_INPUT`]
/// and none anywhere else**: a crop this rung sends is sized to the *region*,
/// and the sidecar upscales a small one to its own working resolution before
/// the edit rather than this side padding it out with page the model then has
/// to spend its resolution on.
///
/// That floor used to be here, and the argument for it was rule 4's: the
/// decode window is sized against an `engine_context` of 512², so 512² of page around a region is
/// what the window guarantees is there to read. It is still guaranteed and this
/// rung now reads less of it, which the window permits - what it does not
/// permit is reading *more*, and nothing here does. Rungs 2 and 3 still take the
/// whole 512², because their graphs have a fixed spatial input and no working
/// resolution to upscale to.
///
/// The rectangle may extend past the page. That is the caller's problem to
/// handle by edge-replication (§5.2 step 2: "edge-replicate where the crop
/// abuts the true page edge. Never reflect, never upsample"), and it is handled
/// in [`Inpainter::render`] rather than by clamping here, because a clamped
/// crop would put the region off-centre and hand the model a lopsided context.
pub fn crop_for(bounds: Rect) -> Rect {
    let context = context_for(bounds);
    let side = |extent: u32| snap(extent.saturating_add(2 * context));
    let (w, h) = (side(bounds.w), side(bounds.h));
    Rect::new(
        bounds.x + bounds.w as i64 / 2 - w as i64 / 2,
        bounds.y + bounds.h as i64 / 2 - h as i64 / 2,
        w,
        h,
    )
}

fn snap(extent: u32) -> u32 {
    extent.div_ceil(LATENT_STRIDE) * LATENT_STRIDE
}

/// A held sidecar, and the model it was opened with.
///
/// Owned by whatever reached this rung, not by the region, for
/// [`crate::engines::lama::Inpainter`]'s reason with a heavier price attached:
/// opening costs a process start and several gigabytes read off disk. Dropping
/// it kills the child, which is what the "refuse and unload rung 3a"
/// pressure step actually does.
pub struct Inpainter {
    sidecar: Sidecar,
    model: String,
}

impl Inpainter {
    /// Spawn the sidecar, check what it says about itself against the gate, and
    /// load the model.
    ///
    /// The gate runs **twice** and that is not redundancy. [`Budget`] arrives
    /// from [`crate::sidecar::availability`], which answered before anything
    /// was spawned so that a machine that cannot carry the rung is never
    /// offered it. Here the sidecar has been asked what this model and this
    /// backend actually cost, and that answer is the authoritative one - so it
    /// is gated again, against the same room, before a single weight is read.
    pub fn open(
        install: &Install,
        backend: Backend,
        model: &str,
        budget: Budget,
    ) -> Result<Inpainter, Error> {
        // Before anything is spawned: a backend with no render path in this
        // build is refused for that, and not for whatever its memory turns out
        // to be. Since `sdnq` this fires for a *chosen* backend rather than for a
        // platform - see [`crate::sidecar::hardware::platform_decline_for`].
        if let Some(decline) = hardware::platform_decline_for(backend) {
            return Err(decline.into());
        }
        let mut sidecar = Sidecar::spawn(install, backend, model, budget)?;
        let health = sidecar.client().health()?;
        // **What the install can actually import**, which is the one question
        // this side cannot answer for itself: it found an interpreter beside a
        // `pyvenv.cfg` and a venv may hold either backend's packages. An empty
        // list is a sidecar too old to answer and is not a refusal.
        if !crate::sidecar::reports_backend(&health.backends, backend) {
            return Err(Decline::SidecarBackendMissing.into());
        }
        let demand =
            Demand::from_report(health.weights_bytes, health.working_set_bytes, health.basis);
        match hardware::gate_backend_within(backend, demand, Some(budget.room)) {
            Verdict::Admitted(_) => {}
            Verdict::Declined(decline) => return Err(decline.into()),
        }
        sidecar.client().open(model)?;
        // The open's reply carries the child's memory block, so this is the
        // first moment the loaded-models row can say what rung 3a costs.
        sidecar.note_memory();
        Ok(Inpainter { sidecar, model: model.to_owned() })
    }

    /// An inpainter over a sidecar somebody else is running.
    ///
    /// For a hand-run sidecar under a debugger, and for the tests - which is
    /// how a whole region goes through this protocol and this composite on a
    /// machine with no Python installed.
    pub fn attach(sidecar: Sidecar, model: &str) -> Inpainter {
        Inpainter { sidecar, model: model.to_owned() }
    }

    /// What the sidecar's memory has done across the regions so far.
    /// Rule 9's fifth guard is a
    /// measurement, and this is where it is read from.
    pub fn floor(&mut self) -> Floor {
        self.sidecar.client().floor()
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Whether the child should be given back at the holder's next safe point.
    /// [`crate::sidecar::Sidecar::spent`] is the whole of it.
    pub fn spent(&self) -> bool {
        self.sidecar.spent()
    }

    pub fn client(&mut self) -> &mut Client {
        self.sidecar.client()
    }

    /// Inpaint one region.
    ///
    /// `page` is the raster the region was decoded from, and its edges are the
    /// true page edge - rule 4's clamp is what makes that true.
    ///
    /// One request, one reply, one patch. The buffers are the crop out, the
    /// crop back and the patch; the first two are dropped before this returns,
    /// so what rule 6 asks of *this* process is satisfied by construction. What
    /// it asks of the process at the other end is
    /// [`crate::sidecar::Floor`]'s to record.
    pub fn render(&mut self, page: &Raster, fitted: &Fitted) -> Result<Rendered, Error> {
        if let Some(decline) = declines(page, fitted) {
            return Err(decline.into());
        }
        // **A second region needs neither a fresh process nor a fresh model.**
        // This rung used to
        // release and reopen the model here, because the text-encoder eviction
        // guard nulled an encoder `mflux` cannot rebuild and the second render
        // through one `open` failed with a `NoneType` where the encoder was.
        // That choice is now taken the other way: the encoder is kept, the
        // guard is gone from the `mflux` contract, and the standing footprint is
        // declared rather than evicted away. So a held child is a held *model*,
        // and every region after the first pays only the render.
        let applied = applied_mask(fitted, page.width, page.height);
        let bounds = applied.bounds;
        let crop = crop_for(bounds);
        let ceiling = ceiling_for(page.depth);
        let (pad, image) = crop_samples(page, crop, ceiling);
        let hint = hint_samples(fitted, crop);

        let request = RenderRequest {
            region: format!("{}x{}+{}+{}", bounds.w, bounds.h, bounds.x, bounds.y),
            image: Image::new(crop.w, crop.h, Encoding::Rgb8, &image),
            hint: Image::new(crop.w, crop.h, Encoding::Gray8, &hint),
            prompt: PROMPT.to_owned(),
            steps: STEPS,
            seed: SEED,
            guidance: GUIDANCE,
            deadline_ms: crate::sidecar::client::RENDER_TIMEOUT.as_millis() as u64,
        };
        // The request's own copy of the crop goes before the reply's arrives.
        // One region's pixels are in flight at a time, on this side as well as
        // on the other.
        drop(image);
        drop(hint);

        let reply = self.sidecar.client().render(&request)?;
        self.sidecar.note_memory();
        let edited = reply
            .image
            .samples()
            .ok_or_else(|| Error::Run("the sidecar's reply does not decode".to_owned()))?;
        drop(request);

        let mut patch = page_crop(page, bounds);
        let ramp = AlphaRamp::new(fitted, page.width, page.height);
        let samples = page.mode.samples();
        let alpha_channel = page.mode.alpha_channel();

        for y in bounds.y..bounds.bottom() {
            for x in bounds.x..bounds.right() {
                if !applied.contains(x, y) {
                    continue;
                }
                let alpha = ramp.at(x, y);
                if alpha == 0.0 {
                    continue;
                }
                let at = ((y - crop.y) * crop.w as i64 + (x - crop.x)) as usize * 3;
                let (lx, ly) = ((x - bounds.x) as u32, (y - bounds.y) as u32);
                let dither = dither_at(page.depth, alpha, x, y, ceiling);
                for channel in 0..samples {
                    // Alpha is copied, never produced.
                    if alpha_channel == Some(channel) {
                        continue;
                    }
                    let model = returned_sample(&edited, page.mode, channel, at);
                    let original = patch.sample(lx, ly, channel) as f64 / ceiling;
                    let blended = alpha * model + (1.0 - alpha) * original;
                    let value = (blended * ceiling + dither).round().clamp(0.0, ceiling) as u16;
                    patch.set_sample(lx, ly, channel, value);
                }
            }
        }

        // `pad` is [`crop_samples`]'s answer and nothing else's: it is set when
        // the crop read past the page, which is rule 4's question and the same
        // one rung 2 answers per tile. The crop is now sized to the region
        // rather than floored at 512², so this fires for a region genuinely
        // near the page edge instead of for every small region on a small page
        // - and a patch made partly against replicated pixels is still a patch
        // a reviewer should be able to see was.
        Ok(Rendered { mask: applied, pixels: patch, pad, tiles: 1 })
    }
}

/// The crop, as `rgb8`, edge-replicated where it runs off the page.
///
/// Edge-replicate and **never reflect**: §3 is absolute about it and
/// [`EdgePad`] has no `Reflect` variant to select. Never upsample either - the
/// crop is at the page's native resolution and stays there, which is §5.2 step
/// 2's other half.
fn crop_samples(page: &Raster, crop: Rect, ceiling: f64) -> (EdgePad, Vec<u8>) {
    let mut pad = EdgePad::None;
    let mut out = vec![0u8; (crop.w as usize) * (crop.h as usize) * 3];
    for y in 0..crop.h as i64 {
        for x in 0..crop.w as i64 {
            let (px, py) = (crop.x + x, crop.y + y);
            let cx = px.clamp(0, page.width as i64 - 1);
            let cy = py.clamp(0, page.height as i64 - 1);
            if (cx, cy) != (px, py) {
                pad = EdgePad::Replicate;
            }
            let at = (y * crop.w as i64 + x) as usize * 3;
            for c in 0..3 {
                let channel = source_channel(page.mode, c);
                let sample = page.sample(cx as u32, cy as u32, channel) as f64;
                // Scaled by the page's own ceiling rather than shifted, so a
                // depth below eight bits arrives as the full range the model
                // expects instead of as a dark image.
                out[at + c] = (sample / ceiling * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    (pad, out)
}

/// The hint: 255 where the text is, 0 elsewhere.
///
/// It is the **ink** mask - the lettering and its ring - and not the grown
/// fitted mask: the
/// write set was narrowed to `Fitted::ink` on every model rung, and a hint
/// pointing at the whole grown mask would zero real context the model is now
/// allowed to keep, and `model_hole`'s table says a wider hole invents more.
/// The hint reaches no mask channel at all; it matters only because a sidecar
/// may use it to place its own attention, and pointing at the isolation ring
/// would point at paper.
fn hint_samples(fitted: &Fitted, crop: Rect) -> Vec<u8> {
    let mut out = vec![0u8; (crop.w as usize) * (crop.h as usize)];
    for y in 0..crop.h as i64 {
        for x in 0..crop.w as i64 {
            if fitted.ink.contains(crop.x + x, crop.y + y) {
                out[(y * crop.w as i64 + x) as usize] = 255;
            }
        }
    }
    out
}

/// One returned sample, reduced back to the page's own channel.
///
/// A grayscale page takes the mean of the three, for
/// [`crate::engines::lama`]'s measured reason: a model is free to return
/// something slightly non-neutral, and picking one channel keeps a third more
/// of that noise than averaging does.
fn returned_sample(edited: &[u8], mode: ColorMode, channel: usize, at: usize) -> f64 {
    match mode {
        ColorMode::Gray | ColorMode::GrayAlpha => {
            (edited[at] as f64 + edited[at + 1] as f64 + edited[at + 2] as f64) / 3.0 / 255.0
        }
        _ => edited[at + channel.min(2)] as f64 / 255.0,
    }
}

/// The largest region this rung accepts, restated so a reader of this file does
/// not have to open [`crate::engines::model`] to find out that it is the same
/// number as every other model rung's.
pub const MAX_REGION: u32 = MAX_BOX;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::model::MODEL_INPUT;
    use crate::fit;
    use crate::image::BitDepth;
    use crate::mask::Mask;
    use crate::sidecar::hardware::RESIDENT_CLEANER;
    use crate::sidecar::wire::{
        Basis, Encoding, ErrorBody, ErrorDetail, ErrorKind, Health, Image, MemoryReport, OpenReply,
        RenderReply, State,
    };
    use crate::memory::GIB;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /* -- a sidecar that is not Python ---------------------------------- */

    /// What the fake sidecar does with the crop it is handed.
    #[derive(Clone, Copy)]
    enum Behaviour {
        /// Paint the whole crop black. Nothing a real model would do, and
        /// exactly what a composite test wants: every pixel this rung is
        /// *allowed* to change is unmistakable, and every pixel it is not must
        /// still be the page.
        Blacken,
        /// Answer 507 `out_of_memory` - rule 9's third guard from the far side.
        OutOfMemory,
        /// Report a peak over the budget, so the *parent's* cap is what fires.
        PeakOver(u64),
        /// Reply with a crop of the wrong size.
        WrongShape,
        /// Open without applying one of the required guards.
        ShortEcho,
    }

    /// A sidecar that speaks the protocol and holds no model.
    struct Fake {
        addr: SocketAddr,
        token: String,
        requests: Arc<AtomicU32>,
        /// How many `POST /v1/open`s have arrived. The instrument for
        /// [`one_child_serves_two_region_edits_and_reopens_the_model_between_them`]:
        /// a reopen is a fact about the wire, so it is counted on the wire.
        opens: Arc<AtomicU32>,
        /// Every prompt that has arrived on `/v1/render`, in order. The
        /// instrument for [`one_prompt_reaches_the_model_whatever_the_page_mode_is`]:
        /// which prompt the model is given is a fact about the wire too.
        prompts: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl Fake {
        fn start(behaviour: Behaviour) -> Fake {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let requests = Arc::new(AtomicU32::new(0));
            let opens = Arc::new(AtomicU32::new(0));
            let prompts = Arc::new(std::sync::Mutex::new(Vec::new()));
            let counter = requests.clone();
            let opened = opens.clone();
            let said = prompts.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    counter.fetch_add(1, Ordering::Relaxed);
                    serve(stream, behaviour, &opened, &said);
                }
            });
            Fake { addr, token: "0".repeat(48), requests, opens, prompts }
        }

        fn inpainter(&self, budget: Budget) -> Inpainter {
            Inpainter::attach(
                Sidecar::attach(self.addr, self.token.clone(), Backend::Mflux, budget),
                "flux2-klein-4b",
            )
        }

        /// The prompt of the most recent render, as it arrived on the wire.
        fn prompt(&self) -> Option<String> {
            self.prompts.lock().unwrap().last().cloned()
        }
    }

    fn serve(
        mut stream: std::net::TcpStream,
        behaviour: Behaviour,
        opens: &Arc<AtomicU32>,
        prompts: &Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let mut raw = Vec::new();
        let mut buffer = [0u8; 8192];
        // Read until the headers are complete, then until the declared body is.
        loop {
            let read = match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            raw.extend_from_slice(&buffer[..read]);
            let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
            let head = String::from_utf8_lossy(&raw[..split]).to_string();
            let length: usize = head
                .split("\r\n")
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse().ok())
                .unwrap_or(0);
            if raw.len() >= split + 4 + length {
                let body = raw[split + 4..split + 4 + length].to_vec();
                let (status, reply) = answer(&head, &body, behaviour, opens, prompts);
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                        reply.len()
                    )
                    .as_bytes(),
                );
                let _ = stream.write_all(&reply);
                let _ = stream.flush();
                break;
            }
        }
    }

    fn answer(
        head: &str,
        body: &[u8],
        behaviour: Behaviour,
        opens: &Arc<AtomicU32>,
        prompts: &Arc<std::sync::Mutex<Vec<String>>>,
    ) -> (u16, Vec<u8>) {
        let path = head.split(' ').nth(1).unwrap_or("");
        let json = |value: &serde_json::Value| serde_json::to_vec(value).unwrap();
        match path {
            "/v1/health" => (
                200,
                json(&serde_json::to_value(Health {
                    protocol: crate::sidecar::wire::PROTOCOL,
                    sidecar_version: "test".into(),
                    state: State::Idle,
                    backend: Some("mflux".into()),
                    model: None,
                    weights_bytes: Some(4600 * 1024 * 1024),
                    working_set_bytes: Some(4600 * 1024 * 1024),
                    basis: Some(Basis::Declared),
                    applied: Vec::new(),
                    backends: vec!["mflux".into(), "sdnq".into()],
                    memory: None,
                })
                .unwrap()),
            ),
            "/v1/open" => {
                opens.fetch_add(1, Ordering::Relaxed);
                let mut applied: Vec<String> = Backend::Mflux
                    .contract()
                    .required
                    .iter()
                    .map(|g| (*g).to_owned())
                    .collect();
                if matches!(behaviour, Behaviour::ShortEcho) {
                    applied.pop();
                }
                (
                    200,
                    json(&serde_json::to_value(OpenReply {
                        applied,
                        weights_bytes: Some(4600 * 1024 * 1024),
                        working_set_bytes: Some(4600 * 1024 * 1024),
                        basis: Some(Basis::Declared),
                        memory: None,
                    })
                    .unwrap()),
                )
            }
            "/v1/render" => {
                if let Behaviour::OutOfMemory = behaviour {
                    return (
                        507,
                        json(&serde_json::to_value(ErrorBody {
                            error: ErrorDetail {
                                kind: ErrorKind::OutOfMemory,
                                detail: Some("the cap fired".into()),
                            },
                        })
                        .unwrap()),
                    );
                }
                let request: crate::sidecar::wire::RenderRequest =
                    serde_json::from_slice(body).unwrap();
                prompts.lock().unwrap().push(request.prompt.clone());
                let (w, h) = match behaviour {
                    Behaviour::WrongShape => (request.image.width / 2, request.image.height),
                    _ => (request.image.width, request.image.height),
                };
                let pixels = vec![0u8; (w as usize) * (h as usize) * 3];
                let peak = match behaviour {
                    Behaviour::PeakOver(bytes) => bytes,
                    _ => 1024,
                };
                (
                    200,
                    json(&serde_json::to_value(RenderReply {
                        image: Image::new(w, h, Encoding::Rgb8, &pixels),
                        elapsed_ms: 1,
                        memory: MemoryReport {
                            rss_bytes: Some(peak),
                            peak_rss_bytes: peak,
                            cache_bytes: Some(0),
                            ..Default::default()
                        },
                    })
                    .unwrap()),
                )
            }
            _ => (202, b"{}".to_vec()),
        }
    }

    /* -- fixtures ------------------------------------------------------ */

    fn gray_page(w: u32, h: u32, level: u8) -> Raster {
        Raster {
            width: w,
            height: h,
            mode: ColorMode::Gray,
            depth: BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![level; (w * h) as usize],
        }
    }

    fn fitted_over(page: &Raster, rect: Rect) -> Fitted {
        let seed = Mask::filled(rect);
        fit::fit(page, &seed, 1.0, 0.0, &fit::EdgeMap::none(page.width, page.height), true)
    }

    fn generous() -> Budget {
        hardware::gate_within(
            Demand::from_report(Some(1024), Some(1024), Some(Basis::Declared)),
            Some(64 * GIB + RESIDENT_CLEANER),
        )
        .budget()
        .unwrap()
    }

    /* -- the crop ------------------------------------------------------ */

    /// The crop is centred on the region, is a multiple of the latent stride,
    /// and is **sized to the region** - proportional context, clamped at both
    /// ends, and no floor at the engine context.
    #[test]
    fn a_crop_is_centred_snapped_and_tight_around_the_region() {
        // A line of dialogue. Half of 40 is 20, under the floor, so the context
        // is `CONTEXT_MIN` - and the crop is nowhere near 512², which is the
        // whole of the smudge this rung used to produce.
        let small = crop_for(Rect::new(300, 400, 40, 24));
        assert_eq!((small.w, small.h), (snap(40 + 2 * CONTEXT_MIN), snap(24 + 2 * CONTEXT_MIN)));
        assert!(small.w < MODEL_INPUT && small.h < MODEL_INPUT);
        assert_eq!(small.x + small.w as i64 / 2, 300 + 20);
        assert_eq!(small.y + small.h as i64 / 2, 400 + 12);

        // A sound effect: half the long side is past `CONTEXT_MAX`, so the
        // context is capped rather than growing with the region.
        let sfx = crop_for(Rect::new(100, 100, 117, 434));
        assert_eq!(context_for(Rect::new(100, 100, 117, 434)), CONTEXT_MAX);
        assert_eq!((sfx.w, sfx.h), (snap(117 + 2 * CONTEXT_MAX), snap(434 + 2 * CONTEXT_MAX)));

        let large = crop_for(Rect::new(0, 0, 900, 1000));
        assert_eq!(large.w % LATENT_STRIDE, 0);
        assert_eq!(large.h % LATENT_STRIDE, 0);
        assert!(large.w >= 900 + 2 * CONTEXT_MAX);
        assert!(large.h >= 1000 + 2 * CONTEXT_MAX);

        // Every region this rung accepts produces a bounded crop, because
        // `MAX_BOX` is what bounds the region.
        let biggest = crop_for(Rect::new(0, 0, MAX_REGION, MAX_REGION));
        assert!(biggest.w <= MAX_REGION + 2 * CONTEXT_MAX + LATENT_STRIDE);
    }

    /* -- the composite ------------------------------------------------- */

    /// **Our composite is the boundary.** The sidecar returns a crop that is
    /// black everywhere - the model rewriting all of it, which §4a says it does -
    /// and the patch that comes back is black only inside the applied mask
    /// and is the page everywhere else.
    #[test]
    fn a_model_that_rewrites_the_whole_crop_only_reaches_inside_the_applied_mask() {
        let fake = Fake::start(Behaviour::Blacken);
        let page = gray_page(600, 700, 200);
        let fitted = fitted_over(&page, Rect::new(240, 300, 60, 40));
        let mut inpainter = fake.inpainter(generous());

        let rendered = inpainter.render(&page, &fitted).expect("the fake sidecar declined");
        let bounds = rendered.mask.bounds;
        assert_eq!(rendered.tiles, 1, "one crop, never a tiling");

        let mut inside_changed = 0usize;
        for y in 0..rendered.pixels.height {
            for x in 0..rendered.pixels.width {
                let value = rendered.pixels.sample(x, y, 0);
                let (px, py) = (bounds.x + x as i64, bounds.y + y as i64);
                if rendered.mask.contains(px, py) {
                    if value != 200 {
                        inside_changed += 1;
                    }
                } else {
                    assert_eq!(value, 200, "the page changed at {px},{py}, outside the mask");
                }
            }
        }
        assert!(inside_changed > 0, "nothing was written at all");
    }

    /// The ramp is the same ramp the other model rungs write through, so the
    /// edit reaches exactly as far as `EDIT_MARGIN` is derived to cover and no
    /// further.
    #[test]
    fn the_edit_stays_inside_the_write_bound_every_model_rung_shares() {
        let fake = Fake::start(Behaviour::Blacken);
        let page = gray_page(600, 700, 200);
        let fitted = fitted_over(&page, Rect::new(240, 300, 60, 40));
        let mut inpainter = fake.inpainter(generous());
        let rendered = inpainter.render(&page, &fitted).unwrap();

        let permitted = write_bound(&fitted, page.width, page.height);
        let bounds = rendered.mask.bounds;
        for y in 0..rendered.pixels.height {
            for x in 0..rendered.pixels.width {
                if rendered.pixels.sample(x, y, 0) == 200 {
                    continue;
                }
                let (px, py) = (bounds.x + x as i64, bounds.y + y as i64);
                assert!(permitted.contains(px, py), "wrote outside the bound at {px},{py}");
            }
        }
    }

    /// A region against the page's own edge: the crop runs off, the pixels are
    /// replicated rather than reflected, and the patch records it.
    #[test]
    fn a_region_at_the_page_edge_is_replicated_and_says_so() {
        let fake = Fake::start(Behaviour::Blacken);
        let page = gray_page(300, 300, 128);
        let fitted = fitted_over(&page, Rect::new(2, 2, 30, 30));
        let mut inpainter = fake.inpainter(generous());
        let rendered = inpainter.render(&page, &fitted).unwrap();
        assert_eq!(rendered.pad, EdgePad::Replicate);
    }

    /* -- rule 9, from this side ---------------------------------------- */

    /// Rule 9's third guard, from the far side: the sidecar's own cap fired, it
    /// answered 507, and the region declines rather than failing the page.
    #[test]
    fn a_sidecar_that_ran_out_of_its_allowance_declines_the_region() {
        let fake = Fake::start(Behaviour::OutOfMemory);
        let page = gray_page(600, 700, 200);
        let fitted = fitted_over(&page, Rect::new(240, 300, 60, 40));
        let mut inpainter = fake.inpainter(generous());
        match inpainter.render(&page, &fitted) {
            Err(Error::Declined(Decline::SidecarMemory)) => {}
            other => panic!("{other:?}"),
        }
    }

    /// And the same guard from *this* side, which is the whole of the bound for
    /// a backend whose allocator has no setter: the reply arrives, its reported
    /// peak is over the budget, and the region declines. The one after it
    /// declines too, without a request being made - a sidecar that has been
    /// over the cap once does not get a second chance at the machine.
    #[test]
    fn a_peak_over_the_budget_shuts_the_rung_down_from_this_side() {
        let fake = Fake::start(Behaviour::PeakOver(999 * GIB));
        let page = gray_page(600, 700, 200);
        let fitted = fitted_over(&page, Rect::new(240, 300, 60, 40));
        let mut inpainter = fake.inpainter(generous());

        assert!(matches!(
            inpainter.render(&page, &fitted),
            Err(Error::Declined(Decline::SidecarMemory))
        ));
        let after = fake.requests.load(Ordering::Relaxed);
        assert!(matches!(
            inpainter.render(&page, &fitted),
            Err(Error::Declined(Decline::SidecarMemory))
        ));
        assert_eq!(
            fake.requests.load(Ordering::Relaxed),
            after,
            "a second region was sent to a sidecar that had already been over the cap"
        );
    }

    /// A reply of the wrong size is a **fault**, not something to resample.
    /// §5.2 step 4 permits a resample only as a last resort with a named
    /// filter, which is not a decision to make inside a transport.
    #[test]
    fn a_reply_of_the_wrong_size_is_a_fault_and_never_a_resample() {
        let fake = Fake::start(Behaviour::WrongShape);
        let page = gray_page(600, 700, 200);
        let fitted = fitted_over(&page, Rect::new(240, 300, 60, 40));
        let mut inpainter = fake.inpainter(generous());
        match inpainter.render(&page, &fitted) {
            Err(Error::Run(detail)) => assert!(detail.contains("asked for"), "{detail}"),
            other => panic!("{other:?}"),
        }
    }

    /// Every guard but one applied and the last silently
    /// absent. The open fails and the missing guard is **named**.
    #[test]
    fn an_open_that_skipped_a_guard_fails_and_says_which_one() {
        let fake = Fake::start(Behaviour::ShortEcho);
        let mut sidecar = Sidecar::attach(
            fake.addr,
            fake.token.clone(),
            Backend::Mflux,
            generous(),
        );
        match sidecar.client().open("flux2-klein-4b") {
            Err(Error::Run(detail)) => {
                assert!(detail.contains("mx.set_cache_limit"), "{detail}");
                assert!(detail.contains("mlx"), "the allocator is named: {detail}");
            }
            other => panic!("{other:?}"),
        }
    }

    /* -- the child, across two edits ------------------------------------ */

    /// **One child, two region edits.**
    /// The inpainter is parked in [`crate::residency`] the way
    /// `region.rs#Bench` parks it at the end of a click, taken back out the way
    /// the next click takes it, and it renders again - against the same fake,
    /// which is how "the same process" is checkable here at all: a respawn
    /// would need an address this test never handed out twice.
    ///
    /// And the second render goes through **the model the first one left**.
    /// This rung used to release and reopen between regions because the
    /// text-encoder eviction guard destroyed an encoder `mflux` cannot rebuild;
    /// that is now taken the other way, so a reopen here would be a model load
    /// nothing asked for. The fake counts opens on the wire, so "no reopen" is
    /// asserted rather than hoped for.
    #[test]
    fn one_child_serves_two_region_edits_through_one_open() {
        let fake = Fake::start(Behaviour::Blacken);
        let page = gray_page(600, 700, 200);
        let fitted = fitted_over(&page, Rect::new(240, 300, 60, 40));
        let key = crate::residency::Key::new(crate::registry::Kind::Sidecar, "test-child");

        let mut first = fake.inpainter(generous());
        first.render(&page, &fitted).expect("the first edit declined");
        assert_eq!(fake.opens.load(Ordering::Relaxed), 0, "the first render reopened the model");
        // The click ends: the child is parked, not killed.
        crate::residency::checkin(key.clone(), first);

        let mut second: Inpainter =
            crate::residency::checkout(&key).expect("the child was killed with the click");
        second.render(&page, &fitted).expect("the second edit declined");
        assert_eq!(
            fake.opens.load(Ordering::Relaxed),
            0,
            "the second region reopened a model that was still loaded"
        );
        assert_eq!(second.floor().regions, 2, "two regions, one child, one open");
        crate::residency::clear();
    }

    /// Rule 9's fifth guard's instrument, over a real exchange.
    #[test]
    fn the_floor_is_recorded_across_regions() {
        let fake = Fake::start(Behaviour::Blacken);
        let page = gray_page(600, 700, 200);
        let fitted = fitted_over(&page, Rect::new(240, 300, 60, 40));
        let mut inpainter = fake.inpainter(generous());
        inpainter.render(&page, &fitted).unwrap();
        inpainter.render(&page, &fitted).unwrap();
        let floor = inpainter.floor();
        assert_eq!(floor.regions, 2);
        assert_eq!(floor.ratchet(), Some(0), "a flat sidecar does not ratchet");
    }

    /* -- the shared vocabulary ----------------------------------------- */

    /// Rung 3's refusal, for rung 3's reason: the wire is eight bits, so a
    /// 16-bit source has no path through this rung either - and it is named
    /// with the *same* decline, not a second spelling of it.
    #[test]
    fn a_sixteen_bit_page_is_declined_under_the_reason_rung_three_already_has() {
        let mut page = gray_page(600, 700, 200);
        page.depth = BitDepth::Sixteen;
        page.data = vec![0u8; (600 * 700 * 2) as usize];
        assert!(!applies(&page));
        let fitted = fitted_over(&page, Rect::new(240, 300, 60, 40));
        assert_eq!(
            declines(&page, &fitted),
            Some(Decline::DepthBeyondEngine(BitDepth::Sixteen))
        );
    }

    /// And the refusals every model rung shares are still this rung's, because
    /// they are asked of [`crate::engines::model`] and not restated.
    #[test]
    fn a_region_past_the_shared_ceiling_is_declined_before_a_process_is_spawned() {
        let page = gray_page(4000, 4000, 200);
        let fitted = fitted_over(&page, Rect::new(0, 0, MAX_REGION + 1, 10));
        assert!(matches!(declines(&page, &fitted), Some(Decline::TooLarge { .. })));
    }

    /* -- the prompt ---------------------------------------------------- */

    /// **The same instruction reaches the model whatever the page's colourspace
    /// is**, and it carries no vocabulary that could depend on one.
    ///
    /// This is the invariant [`PROMPT`]'s sweep bought, and it is worth a test
    /// because the thing it replaced was a *pair* of prompts selected by
    /// inspecting the crop's channel spread - so a regression here would not be
    /// a changed string, it would be the branch coming back. A grey page and a
    /// colour page are rendered through this rung and the request's prompt is
    /// compared, which is the only place the branch could reappear.
    #[test]
    fn one_prompt_reaches_the_model_whatever_the_page_mode_is() {
        // No screentone, no monochrome, no colour vocabulary - the premise
        // for splitting the prompt was that such words drift a colour page.
        for word in ["screentone", "black and white", "colored", "manga"] {
            assert!(
                !PROMPT.to_lowercase().contains(word),
                "the prompt carries colourspace vocabulary: {word:?}"
            );
        }

        let fake = Fake::start(Behaviour::Blacken);
        let mut sent = Vec::new();
        for mode in [ColorMode::Gray, ColorMode::Rgb] {
            let mut page = gray_page(600, 700, 200);
            if mode == ColorMode::Rgb {
                page.mode = ColorMode::Rgb;
                page.data = (0..600 * 700)
                    .flat_map(|i| [200, 60, (i % 251) as u8])
                    .collect();
            }
            let fitted = fitted_over(&page, Rect::new(240, 300, 60, 40));
            let mut inpainter = fake.inpainter(generous());
            inpainter.render(&page, &fitted).expect("the fake sidecar declined");
            sent.push(fake.prompt());
        }
        assert_eq!(sent[0].as_deref(), Some(PROMPT));
        assert_eq!(sent[0], sent[1], "the prompt still depends on the page's mode");
    }
}
