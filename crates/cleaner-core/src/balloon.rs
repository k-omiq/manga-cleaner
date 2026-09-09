//! What decides "in a balloon".
//!
//! The script gate is scoped by whether text sits inside a speech balloon:
//! inside, a confident verdict cleans or skips; outside, the gate does not
//! decide and the region goes to review. Revisions 1-3 read that off a
//! `comic_text_detector` class, and Phase 0 spike 6 found the class does
//! not exist - that model emits `eng`/`ja`.
//!
//! The three names those revisions used are real and belong here:
//! `ogkalu/comic-text-and-bubble-detector`, Apache-2.0, RT-DETR-v2 (so clear
//! of the YOLOv8 AGPL trap), `detector-v4-s_int8.onnx` at 11.1 MB, classes
//! `bubble` / `text_bubble` / `text_free`. It was set aside as a
//! *replacement* for the detector because it has no segmentation mask; that
//! objection does not apply to running it beside one purely for this question.
//!
//! It answers directly: a region overlapping a `text_bubble` box is in a
//! balloon, one overlapping `text_free` is not.
//!
//! ## The model is not the whole answer, and the page can be asked too
//!
//! One box at 0.35 confidence decides whether a region is gated or sent to
//! review as out of a balloon, and the misses are one-sided in the direction
//! that costs the most: a balloon the detector did not emit a box for reads as
//! `OutOfBalloon`, so dialogue in an ordinary bubble arrives in review with
//! *"text outside a speech bubble"* beside it. The other candidate
//! mechanism - the segmentation mask's own geometry - was set aside as a
//! heuristic that "fails on open-tail balloons and on white text over
//! black". Neither objection applies to it as a *second*
//! opinion beside the model, and the second of the two does not apply at all to
//! the test [`interior`] actually makes.
//!
//! That test is one sentence: **the paper immediately outside the text is a
//! balloon's interior when it is a uniform fill**, walked outward ring by ring
//! from the text's own box, with the text's strokes excluded and a thin ink
//! line tolerated where it is the outline the interior ends at. Uniform, not
//! white - a black balloon under white lettering is as uniform as a white one,
//! which is the *"white text over black"* half of §3's objection answered by
//! measuring spread rather than level. What the test refuses is picture:
//! screentone, hatching and art all break the uniformity in the same way, by
//! putting many separate runs of off-tone pixels on one ring.
//!
//! [`Interior::settles`] is where the two opinions meet, and it is deliberately
//! asymmetric about which of them may be overruled. Solid paper carries a
//! region into the gate whatever the detector said, because that is the
//! false positive this exists to remove; picture carries it out unless the
//! detector answered the balloon question itself and confidently
//! ([`Detected::TextInBubble`]), because a heuristic that fails on noisy scans
//! must not veto a model that does not; and anything the page cannot answer  - 
//! a region at the page edge, a box that abuts ink at once - leaves the
//! detector's answer standing rather than replacing it with a guess.
//!
//! ## Enclosure is the model answering the question too
//!
//! That asymmetry was written as though `text_bubble` were the only confident
//! answer the model gives, and on real pages it is not. A balloon with a jagged
//! or wobbly outline whose lettering nearly fills it comes back as `bubble` at
//! 0.86–0.96 with **no** `text_bubble` box at or above [`SURE_SCORE`]: the
//! outline is close enough to the text that the paper walk hits it on its first
//! ring and reads [`Interior::Textured`], and picture was allowed to veto a
//! [`Detected::Bubble`]. Six regions across eighteen probe pages went to review
//! as *"text outside a speech bubble"* that way - 02.png 414,115 177×430 over
//! `Bubble@0.92`; 03.png 126,105 295×254 over `Bubble@0.92 Bubble@0.91` and
//! 779,982 197×402 over `Bubble@0.92 Bubble@0.86`; 04.png 786,923 101×378 over
//! `Bubble@0.69 TextInBubble@0.48`; 10.png 483,1018 262×365 over two
//! `Bubble@0.96`; and 15.png 819,559 157×383 over `Bubble@0.94 Bubble@0.95`.
//! Every one is ordinary dialogue in a white balloon.
//!
//! So [`detected`] grades sure `bubble` boxes that **cover** the region as
//! [`Detected::TextInBubble`]. Coverage rather than centre-containment is what
//! keeps this narrow: a large `bubble` box reaching across a sound effect holds
//! that effect's centre and not its extent, and `text_free` still wins outright
//! wherever the model drew it. What is left is the model saying *this text is
//! enclosed by a balloon*, which is the balloon question answered, and picture
//! may not veto it.
//!
//! Coverage is taken over the **union of the boxes**, and that is not a detail.
//! The detector emits one `bubble` box per lobe, and every one of the six cases
//! above is a two-lobed balloon: on 02.png the region is under two boxes that
//! only meet at y=319, and no single box covers a third of it. A rule reading
//! one box at a time answers *bubble* on all six.
//!
//! The tolerance the walk reads fill against is the band's own spread
//! ([`fill_band`]), floored at [`FILL_TOLERANCE`] and capped at
//! [`MAX_FILL_TOLERANCE`]. A fixed six levels read every white balloon on a
//! noisy scan as picture, and the page's flattest-decile noise could not stand
//! in for it because a scan's flattest tiles are saturated white.

use std::path::Path;

use crate::detect::{DetectedLanguage, Region, Segmentation, overlaps_enough};
use crate::image::Raster;
use crate::mask::{Mask, Rect};

/// The model's fixed input side.
const INPUT: usize = 640;
/// Below this the box is not evidence of anything. RT-DETR emits a fixed 300
/// queries per image and most of them are noise.
const SCORE_THRESHOLD: f32 = 0.35;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalloonClass {
    /// A balloon shape, whether or not text was found in it.
    Bubble,
    /// Text that is inside a balloon.
    TextInBubble,
    /// Text that is not.
    TextFree,
}

impl BalloonClass {
    fn from_label(label: i64) -> Option<BalloonClass> {
        match label {
            0 => Some(BalloonClass::Bubble),
            1 => Some(BalloonClass::TextInBubble),
            2 => Some(BalloonClass::TextFree),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BalloonBox {
    pub rect: Rect,
    pub class: BalloonClass,
    pub score: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum BalloonError {
    #[error("the balloon detector could not be loaded: {0}")]
    Model(String),
    #[error("balloon detection failed: {0}")]
    Inference(String),
    #[error("the balloon detector's outputs are not the expected shape: {0}")]
    Contract(String),
}

pub struct BalloonDetector {
    session: ort::session::Session,
    selection: crate::accel::Selection,
    /// This session's row in [`crate::registry`]. See
    /// [`crate::detect::Detector`], which carries the same field for the same
    /// reason.
    lease: crate::registry::Lease,
}

impl BalloonDetector {
    /// This is the int8 model CoreML **cannot build a session for**
    /// ([`crate::accel::BALLOON`]), so `Automatic` keeps it on the CPU where it
    /// is fastest anyway. A user who forces CoreML gets the 8-second failure
    /// and then the CPU, reported rather than silent.
    pub fn open(model: &Path, preference: crate::accel::Preference) -> Result<BalloonDetector, BalloonError> {
        let (session, selection) =
            crate::accel::open_session(model, &crate::accel::BALLOON, preference, None)
                .map_err(|e| BalloonError::Model(e.to_string()))?;
        let lease = crate::registry::register(
            crate::registry::Kind::BalloonDetector,
            crate::registry::Footprint::weights(model),
            crate::registry::Device::accelerator(selection.accelerator),
        );
        Ok(BalloonDetector { session, selection, lease })
    }

    pub fn selection(&self) -> &crate::accel::Selection {
        &self.selection
    }

    /// Whether the owner should give this session back at its next safe point.
    /// [`crate::detect::Detector::spent`] states the trade.
    pub fn spent(&self) -> bool {
        self.lease.spent()
    }

    /// Give this session up for good, because it has failed in a way another
    /// run on it cannot survive - a reset adapter reported as device-removed.
    /// [`crate::registry::Lease::poison`] states what that costs and why it is
    /// not the same signal as an unload the user asked for.
    pub fn poison(&self) {
        self.lease.poison();
    }

    pub fn detect(&mut self, page: &Raster) -> Result<Vec<BalloonBox>, BalloonError> {
        self.lease.touch();
        // `preprocessor_config.json`: resize to 640×640, `do_rescale` with
        // 1/255, and `do_normalize: false` - the mean and std listed beside it
        // are not applied, which is easy to miss because they are there.
        let mut tensor = vec![0f32; 3 * INPUT * INPUT];
        let sx = page.width as f32 / INPUT as f32;
        let sy = page.height as f32 / INPUT as f32;
        for y in 0..INPUT {
            let py = (((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (page.height - 1) as f32)) as u32;
            for x in 0..INPUT {
                let px = (((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (page.width - 1) as f32)) as u32;
                let [r, g, b] = page.rgb8_pixel(px, py);
                tensor[y * INPUT + x] = r as f32 / 255.0;
                tensor[INPUT * INPUT + y * INPUT + x] = g as f32 / 255.0;
                tensor[2 * INPUT * INPUT + y * INPUT + x] = b as f32 / 255.0;
            }
        }

        let images = ort::value::Tensor::from_array(([1usize, 3, INPUT, INPUT], tensor))
            .map_err(|e| BalloonError::Inference(e.to_string()))?;
        // The model scales its own boxes back to this size, which is why the
        // page's own dimensions go in rather than the input's.
        //
        // **Width first.** Hugging Face's `RTDetrImageProcessor` documents
        // `orig_target_sizes` as `(height, width)`, and this export wants the
        // other order: with `(height, width)` every box comes back with its x
        // multiplied by `H/W` and its y by `W/H`, which on a 1600×2400 page is
        // 1.5 and 0.667 - a set of boxes that looks like plausible detections
        // in the wrong places rather than like an error.
        let sizes = ort::value::Tensor::from_array((
            [1usize, 2],
            vec![page.width as i64, page.height as i64],
        ))
        .map_err(|e| BalloonError::Inference(e.to_string()))?;

        let outputs = self
            .session
            .run(ort::inputs![images, sizes])
            .map_err(|e| BalloonError::Inference(e.to_string()))?;

        let (_, labels) = outputs["labels"]
            .try_extract_tensor::<i64>()
            .map_err(|e| BalloonError::Contract(e.to_string()))?;
        let (_, boxes) = outputs["boxes"]
            .try_extract_tensor::<f32>()
            .map_err(|e| BalloonError::Contract(e.to_string()))?;
        let (_, scores) = outputs["scores"]
            .try_extract_tensor::<f32>()
            .map_err(|e| BalloonError::Contract(e.to_string()))?;

        let mut found = Vec::new();
        for i in 0..labels.len().min(scores.len()) {
            if scores[i] < SCORE_THRESHOLD {
                continue;
            }
            let Some(class) = BalloonClass::from_label(labels[i]) else {
                continue;
            };
            let (x1, y1, x2, y2) = (boxes[i * 4], boxes[i * 4 + 1], boxes[i * 4 + 2], boxes[i * 4 + 3]);
            let x = x1.round().clamp(0.0, page.width as f32) as i64;
            let y = y1.round().clamp(0.0, page.height as f32) as i64;
            let right = x2.round().clamp(0.0, page.width as f32) as i64;
            let bottom = y2.round().clamp(0.0, page.height as f32) as i64;
            if right <= x || bottom <= y {
                continue;
            }
            found.push(BalloonBox {
                rect: Rect::new(x, y, (right - x) as u32, (bottom - y) as u32),
                class,
                score: scores[i],
            });
        }

        // Sorted so two runs list them the same way.
        found.sort_by(|a, b| {
            a.rect.y.cmp(&b.rect.y).then(a.rect.x.cmp(&b.rect.x)).then(b.score.total_cmp(&a.score))
        });
        Ok(found)
    }
}

/// A `text_bubble` box at or above this score is the model answering the
/// balloon question about *this text*, confidently, and [`Interior::settles`]
/// lets that answer stand against a paper reading of *picture*. Above
/// [`SCORE_THRESHOLD`] rather than equal to it: a box that only just cleared
/// emission is evidence, not a verdict. Measured on real scans: every
/// `text_bubble` box the paper reading wrongly vetoed scored 0.88–0.94, and the
/// one it rightly left alone scored 0.38 beside a `text_free` at 0.74.
const SURE_SCORE: f32 = 0.5;

/// What the balloon detector said about one region, graded by how much of the
/// question it actually answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detected {
    /// The model placed this text in a balloon, confidently, and no `text_free`
    /// box contains the region's centre. Two boxes say that:
    ///
    /// - a `text_bubble` box at or above [`SURE_SCORE`] containing the centre  - 
    ///   the model asked "is this text in a balloon?" and answering yes;
    /// - `bubble` boxes at or above [`SURE_SCORE`], one of them containing the
    ///   centre, together covering [`ENCLOSED_SHARE_PERCENT`] of the region  - 
    ///   the model saying this text is enclosed by a balloon, which is the same
    ///   answer arrived at from the other side.
    ///
    /// The second is not a softening of the first. Covering the whole rectangle
    /// is a much stronger claim than holding its centre, and it is the claim
    /// `text_free` cannot also be making: a caption over art has no balloon drawn
    /// around all of it. What it buys is the balloon whose text nearly fills it,
    /// where the model emits `bubble` at 0.86–0.96 and no `text_bubble` at all  - 
    /// see the module note.
    TextInBubble,
    /// A `bubble` shape contains the centre but does not cover the region, or is
    /// too weak to be sure of; or a `text_bubble` box too weak to be sure of
    /// contains the centre. A statement about a shape near the text rather than
    /// about the text, and the one the paper reading may overrule either way.
    Bubble,
    /// A `text_free` box contains the centre, or nothing does.
    Outside,
}

impl Detected {
    pub fn inside(self) -> bool {
        !matches!(self, Detected::Outside)
    }
}

/// What the balloon detector said about a text region.
///
/// The test is containment of the region's **centre**, not overlap: a balloon
/// box and a text box that merely touch are two different things a page corner
/// away, and a large `bubble` box overlapping the edge of an out-of-balloon
/// sound effect would otherwise pull it in.
///
/// `text_free` wins over everything else when both contain the centre, because
/// `text_free` is a statement about *this text* and `bubble` is a statement
/// about a shape that happens to be behind it.
///
/// **The one place a `bubble` box is more than that**: when the sure `bubble`
/// boxes cover the region's whole rectangle - one of them holding its centre  - 
/// they grade [`Detected::TextInBubble`] rather than [`Detected::Bubble`].
/// Enclosure is not proximity: a shape the text sits wholly inside is a shape
/// the text is *in*, and the balloons this rescues are the ones the paper
/// reading is worst at.
pub fn detected(region: Rect, balloons: &[BalloonBox]) -> Detected {
    let cx = region.x + region.w as i64 / 2;
    let cy = region.y + region.h as i64 / 2;

    let mut answer = Detected::Outside;
    let mut sure_shape_over_centre = false;
    for balloon in balloons {
        if !balloon.rect.contains(cx, cy) {
            continue;
        }
        match balloon.class {
            BalloonClass::TextFree => return Detected::Outside,
            BalloonClass::TextInBubble if balloon.score >= SURE_SCORE => {
                answer = Detected::TextInBubble;
            }
            BalloonClass::Bubble if balloon.score >= SURE_SCORE => {
                sure_shape_over_centre = true;
                if answer == Detected::Outside {
                    answer = Detected::Bubble;
                }
            }
            BalloonClass::TextInBubble | BalloonClass::Bubble => {
                if answer == Detected::Outside {
                    answer = Detected::Bubble;
                }
            }
        }
    }

    if answer == Detected::Bubble && sure_shape_over_centre {
        // The union is one balloon's lobes, not every sure shape on the page:
        // a shape counts only if it touches or overlaps one that holds the
        // centre. Lobes of one balloon meet (02.png's two boxes share a row);
        // a neighbouring balloon whose box merely reaches across this region
        // does not, and must not lend its area to the share.
        let sure: Vec<Rect> = balloons
            .iter()
            .filter(|b| b.class == BalloonClass::Bubble && b.score >= SURE_SCORE)
            .map(|b| b.rect)
            .collect();
        let holders: Vec<Rect> = sure.iter().copied().filter(|r| r.contains(cx, cy)).collect();
        let shapes: Vec<Rect> = sure
            .into_iter()
            .filter(|r| holders.iter().any(|h| touches(*h, *r)))
            .collect();
        if covered_percent(region, &shapes) >= ENCLOSED_SHARE_PERCENT {
            answer = Detected::TextInBubble;
        }
    }
    answer
}

/// How much of a region the sure `bubble` shapes must cover before the model is
/// read as having enclosed it, in percent.
///
/// Not 100, and the reason is a shape rather than a tolerance: the balloon
/// detector emits one `bubble` box per **lobe**, and the balloons this rule
/// exists for are the two-lobed ones a letterer draws for a long line. Two
/// overlapping boxes over one balloon leave the concave notches where the lobes
/// meet uncovered, and the region's own rectangle - the lettering's box grown by
/// the seven pixels [`crate::detect`] adds - reaches into them. Measured over
/// twelve probe pages: the four balloons wrongly sent to review covered 0.79,
/// 0.87, 0.93 and 0.97 of their regions, and the two regions that must stay
/// outside covered 0.00 and 0.34. Three quarters is the middle of that gap, and
/// it is also below every one-box case, which covers all of it.
const ENCLOSED_SHARE_PERCENT: i64 = 75;

/// What share of `region`, in percent, the union of `rects` covers.
///
/// The union, not the sum: two `bubble` boxes over one two-lobed balloon overlap
/// heavily, and adding their areas would call a third of a region a whole one.
/// Coordinate compression rather than a raster - a page holds tens of balloon
/// boxes, so the grid is small and the answer is exact.
/// Whether two boxes overlap or share an edge. Inclusive on purpose: the two
/// lobes of a balloon drawn as two shapes can meet at exactly one row.
fn touches(a: Rect, b: Rect) -> bool {
    a.x <= b.right() && b.x <= a.right() && a.y <= b.bottom() && b.y <= a.bottom()
}

fn covered_percent(region: Rect, rects: &[Rect]) -> i64 {
    let area = region.w as i64 * region.h as i64;
    if area == 0 {
        return 0;
    }
    let clipped: Vec<Rect> = rects
        .iter()
        .filter_map(|r| {
            let x = r.x.max(region.x);
            let y = r.y.max(region.y);
            let right = r.right().min(region.right());
            let bottom = r.bottom().min(region.bottom());
            (right > x && bottom > y)
                .then(|| Rect { x, y, w: (right - x) as u32, h: (bottom - y) as u32 })
        })
        .collect();
    if clipped.is_empty() {
        return 0;
    }
    let mut xs: Vec<i64> = clipped.iter().flat_map(|r| [r.x, r.right()]).collect();
    let mut ys: Vec<i64> = clipped.iter().flat_map(|r| [r.y, r.bottom()]).collect();
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();

    let mut covered = 0i64;
    for x in xs.windows(2) {
        for y in ys.windows(2) {
            let inside = clipped.iter().any(|r| {
                r.x <= x[0] && r.right() >= x[1] && r.y <= y[0] && r.bottom() >= y[1]
            });
            if inside {
                covered += (x[1] - x[0]) * (y[1] - y[0]);
            }
        }
    }
    covered * 100 / area
}

/// Whether a text region sits inside a balloon, by the detector alone.
/// [`detected`] with the grade dropped.
pub fn in_balloon(region: Rect, balloons: &[BalloonBox]) -> bool {
    detected(region, balloons).inside()
}

/// The balloon detector's text boxes that the text detector missed, as regions.
///
/// **A second source of boxes**, and it exists because the first source has a
/// blind spot with a shape. Over the 28 real scans the text detector emits a
/// box for every `text_bubble` box the balloon detector sees, so inside a
/// balloon the two agree completely; outside one they do not. Seven boxes the
/// balloon detector scored 0.53 to 0.88 had no region at all - not a region
/// sent to review, where a wrong call can be undone, but no region: 19.png
/// 152,81 238×146 @0.85, 25.png 293,963 311×174 @0.88 and 629,936 176×122
/// @0.80, and 18.png 751,859 147×108 @0.76 are rectangular narration boxes of
/// horizontal Japanese on white; 28.png 113,1447 757×53 @0.71 is a caption
/// line; 24.png 823,1393 171×60 @0.53 is a stylised sound effect; and 01.png
/// 1007,77 63×1132 and 563,1469 422×45 are the chapter title strip and the
/// credits line. All of it is text a cleaner should at least list.
///
/// Only boxes nothing already covers are adopted, on two tests taken together:
/// the box's **centre** must fall in no region's masking rect - the same
/// containment question [`detected`] asks of a region, from the other side  - 
/// and its overlap with every region must stay at or under
/// [`crate::detect::MERGE_OVERLAP_SHARE`], which is the share the extended tier
/// merges on ([`crate::detect::overlaps_enough`] is that test).
/// Centre-containment alone would adopt a wide caption whose middle lands in a
/// gap between two boxes of the same caption; overlap alone would adopt a small
/// box sitting in the corner of a large region.
///
/// *Already covers* includes the boxes this function has itself accepted, and
/// candidates are taken strongest first so that pairing is decided by the
/// model's own confidence rather than by the order the detector returned them
/// in. Without the chain a duplicate pair becomes two regions over one piece of
/// text - see the loop.
///
/// The result is unsorted and holds only the new regions. A caller appends them
/// and then calls [`crate::detect::sort_regions`]: region ids are list indices.
pub fn adopt_uncovered_text(
    regions: &[Region],
    balloons: &[BalloonBox],
    page_w: u32,
    page_h: u32,
    median_area: i64,
) -> Vec<Region> {
    let mut candidates: Vec<&BalloonBox> = balloons
        .iter()
        .filter(|b| matches!(b.class, BalloonClass::TextInBubble | BalloonClass::TextFree))
        .filter(|b| b.score >= SURE_SCORE)
        .collect();
    // **Strongest first, and then each is asked about the ones already taken.**
    // The detector emits overlapping boxes for one piece of text - on 01.png
    // nine `text_bubble` boxes stand over six regions - and a pass that asked
    // only about the text detector's regions would adopt every one of a
    // duplicate pair as its own region, leaving two masks over one caption
    // where the extended tier would have merged two detector boxes into one.
    // Descending score is what decides which of a pair survives: the box the
    // model believes most is the box to keep, and the other is then covered by
    // it under the same two tests the regions are asked.
    //
    // The tie-break is positional and total, so a page with two equal scores
    // adopts the same one on every run.
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.rect.y.cmp(&b.rect.y))
            .then(a.rect.x.cmp(&b.rect.x))
            .then(a.rect.h.cmp(&b.rect.h))
            .then(a.rect.w.cmp(&b.rect.w))
    });

    let mut adopted: Vec<Region> = Vec::new();
    for balloon in candidates {
        let cx = balloon.rect.x + balloon.rect.w as i64 / 2;
        let cy = balloon.rect.y + balloon.rect.h as i64 / 2;
        let covered = regions.iter().chain(adopted.iter()).any(|region| {
            region.masking.contains(cx, cy) || overlaps_enough(&balloon.rect, &region.masking)
        });
        if covered {
            continue;
        }
        adopted.push(Region::from_box(
            balloon.rect,
            balloon.score,
            // The text detector's two classes are languages and upstream
            // annotates them "cls could give wrong result"; the balloon
            // detector does not guess a language at all. Japanese is the
            // weakest thing that can be said here, and nothing routes on it  - 
            // [`crate::gate`] reads the script from the pixels.
            DetectedLanguage::Japanese,
            page_w,
            page_h,
            median_area,
        ));
    }
    adopted
}

/// The narrowest tolerance a luma may sit from the band's own fill and still be
/// that fill, in 16-bit luma. Six 8-bit levels: wider than a clean scan's noise
/// floor - the pages [`crate::fit::ring`] is calibrated against sit at a few
/// levels - and far under the contrast of anything that is not the fill. An
/// anti-aliased glyph edge crosses it, which is why the text mask is dilated
/// before it is removed rather than taken as the model drew it.
///
/// **A floor, not the tolerance.** It was the tolerance, and on a noisy scan
/// that read every balloon as picture: white paper with a spread of 7–10
/// levels puts a fifth of a ring past six, in dozens of separate runs, which is
/// exactly what [`RingRead::is_outline`] refuses and [`Interior::Textured`]
/// means. Measured on real pages, the page's own flattest-decile noise
/// ([`crate::fit::page_noise_sigma`]) was **0** on three of the six - their
/// flattest tiles are saturated white - so the floor cannot be scaled from
/// there either. [`fill_band`] measures the spread of the innermost rings
/// instead, and widens the tolerance to it up to [`MAX_FILL_TOLERANCE`].
const FILL_TOLERANCE: u16 = 6 * 257;

/// The widest tolerance [`fill_band`] will grant, in 16-bit luma. Twenty 8-bit
/// levels: above the spread of any scan that is still paper, and below the
/// contrast of ink, a halftone dot, or hatching against the paper they sit on.
/// Without a cap a screentone's own spread would be measured as its tolerance
/// and every ring of tone would read as fill.
const MAX_FILL_TOLERANCE: u16 = 20 * 257;

/// The tolerance is this share of the innermost rings' deviations, in percent,
/// scaled by [`TOLERANCE_MARGIN`]. The same share [`FILL_SHARE_PERCENT`] asks a
/// ring to keep within it, so that the rings the tolerance was read from would
/// pass their own test; the margin is what keeps the rings *beyond* them, with
/// the same noise, from failing it by a coin toss.
const TOLERANCE_PERCENTILE: usize = FILL_SHARE_PERCENT;

/// Numerator over denominator: three halves.
const TOLERANCE_MARGIN: (u32, u32) = (3, 2);

/// The text mask is grown by this before its pixels are dropped from the band.
/// [`crate::fit::ring`]'s reason, one ring further in: an un-grown contour
/// "lands on the anti-aliasing fringe and biases the median 5–20 levels dark",
/// and here a biased pixel is not a shifted statistic but a fill that is not
/// the fill.
const TEXT_HALO: u32 = 2;

/// Below this many readable pixels a ring is not a ring. A 20×20 box's first
/// ring is 84 pixels before anything is dropped from it, so this is reached
/// only by a box at a page edge or one whose surround is nearly all text.
const MIN_RING_SAMPLES: usize = 16;

/// And below this share of the ring's own on-page pixels, in percent, what is
/// left of it is not a sample of it. The rings nearest a masking box are mostly
/// glyph - the box is drawn around the lettering, so its first two rings are
/// the lettering's own halo - and a dozen surviving pixels in a corner will
/// read as whatever that corner happens to be. Measured against the *on-page*
/// perimeter rather than the whole one, because a region at a page edge has a
/// short ring honestly.
const MIN_RING_COVERAGE_PERCENT: usize = 50;

/// And below this share of the box's own on-page pixels, in percent, the paper
/// *between* the strokes is not a sample of the box.
///
/// The inside's counterpart to [`MIN_RING_COVERAGE_PERCENT`], and a fifth
/// rather than that constant's half, because the two are shaped differently. A
/// ring is drawn where the letterer left clearance, so half of it surviving the
/// text mask is a modest ask; the inside of a text box is mostly lettering by
/// construction, and after [`TEXT_HALO`] a dense block leaves much less. Over
/// the 28 reference scans the narration boxes this reading exists for keep 50%
/// to 62% of their pixels, and the least any of the 254 regions keeps is 15%.
/// So this is a floor against a box that is *all* stroke and not a threshold
/// anything real is near: under it, what is left is the gaps inside the glyphs,
/// and a reading from those is the lettering's own antialiasing.
const MIN_INNER_PAPER_PERCENT: usize = 20;

/// A ring's off-fill pixels may form at most this many separate runs and still
/// be read as an outline. A rim crossing one of [`ring_sides`]' four segments
/// enters and leaves it, so a balloon whose rim dips inside the walk on every
/// side is eight runs; an open tail adds one more, and a scan's own speckle adds
/// a few. A halftone puts one run on every dot it crosses, which on any
/// realistic segment is tens.
const MAX_OUTLINE_RUNS: usize = 10;

/// A ring is the fill when this share of it is within [`FILL_TOLERANCE`], in
/// percent. Not 100: a scan speckles, and a ring that crosses a balloon's
/// interior antialiasing is still that interior.
const FILL_SHARE_PERCENT: usize = 92;

/// Of the pixels that are *not* the fill, this share must fall on one side of
/// it - all darker, or all lighter - for the ring to read as an outline rather
/// than as picture. Ink is one-sided; art is not.
const ONE_SIDED_PERCENT: usize = 95;

/// How much of a box's **inside** may sit off its own fill and the box still be
/// read as paper with lettering on it, in percent of the readable pixels.
///
/// A ring gets [`FILL_SHARE_PERCENT`], which leaves it an eighth of itself.
/// That is the right budget for a ring - a one-pixel line of clearance where
/// anything off the fill is a defect in it - and the wrong one for the inside
/// of a text box, which contains ink by construction: the frame a narration box
/// is drawn with, furigana, a stroke thinner than the segmentation caught, an
/// antialiased edge [`TEXT_HALO`] did not reach. Twice a ring's allowance, and
/// the measurement is what makes that a number rather than a gesture. Over the
/// 28 reference scans, of the regions whose off-fill pixels are one-sided the
/// four narration boxes this reading exists for sit at 6%, 10%, 10% and 14%,
/// and the nearest thing that must **not** read as paper - a sound effect over
/// art, a caption over tone - is at 19%. The gap runs from 15 to 17 and twice
/// the ring's eighth lands in it.
///
/// It is only ever reached by a one-sided population. Art strays both ways and
/// is refused by [`RingRead::is_one_sided`] before this is asked; what this
/// bounds is how much *ink* a box may hold, and a screentone is ink that covers
/// far more of its box than any lettering does.
const INNER_OFF_PERCENT: usize = 2 * (100 - FILL_SHARE_PERCENT);

/// Each of [`ring_sides`]' segments is its side less one of these fractions off
/// each end - a quarter each way, so the middle half. Four rather than three or
/// six because half a side is the largest span whose ends stay inside every
/// balloon whose text box fits it: on an ellipse circumscribing the box, a
/// point a quarter-side off the axis is at 0.87 of the rim's distance where the
/// corner itself is at 1.0.
const SIDE_INSET_QUARTERS: u32 = 4;

/// The shallowest band of fill that means anything at all. Under three pixels
/// the reading is the box's own growth margin rather than a balloon.
const MIN_FILL_DEPTH: u32 = 3;

/// The deepest band worth asking for. A balloon's padding around its text is
/// tens of pixels at most, and asking for more of it only loses the tight ones.
const MAX_FILL_DEPTH: u32 = 8;

/// How the paper immediately outside a text box reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interior {
    /// A band of uniform fill, deep enough to be a balloon's interior. `level`
    /// is that fill in 16-bit luma - the *page's* tone, never snapped to white,
    /// for the same reason [`crate::fit::ring`] refuses to snap one - and
    /// `depth` is how many native pixels of it were walked.
    ///
    /// **`depth: 0` means the fill was read *inside* the box and not walked
    /// outside it** ([`inner_paper`]). There is no band around the lettering to
    /// report a depth for - the answer came from the paper between the strokes -
    /// and zero is the honest number for it rather than a walk that found
    /// nothing. A caller reading `depth` as *how much clearance this text has*
    /// should treat 0 as unknown, not as none.
    Solid { level: u16, depth: u32 },
    /// Picture: screentone, hatching, or art. Not a balloon's interior.
    Textured,
    /// Not enough paper to say. A region at a page edge, a box that abuts ink
    /// at once, or a band that ran out before it was deep enough to mean
    /// anything.
    Unreadable,
}

impl Interior {
    /// The page's reading and the detector's, combined.
    ///
    /// Asymmetric on purpose - see the module note. Uniform fill overrules an
    /// *outside*, picture overrules a *bubble*, and [`Interior::Unreadable`]
    /// leaves the detector's answer exactly as it arrived.
    ///
    /// Picture does **not** overrule [`Detected::TextInBubble`]. That grade is
    /// the model answering this exact question about this exact text with a
    /// score at or above [`SURE_SCORE`], and the paper reading is a heuristic
    /// whose own note admits the scans it fails on. Measured on six real pages
    /// of a noisy scan: nine regions the detector placed in a balloon at
    /// 0.88–0.94 were vetoed as picture, every one of them ordinary dialogue in
    /// an ordinary white balloon, and every one of them went to review under
    /// *"text outside a speech bubble"*.
    pub fn settles(self, detector: Detected) -> bool {
        match self {
            Interior::Solid { .. } => true,
            Interior::Textured => detector == Detected::TextInBubble,
            Interior::Unreadable => detector.inside(),
        }
    }
}

/// How deep a band of fill this box needs before it counts, in native pixels.
///
/// Scaled by the box's short side, because the quantity being measured is a
/// balloon's padding around its own lettering and that padding scales with the
/// lettering. Clamped at both ends: [`MIN_FILL_DEPTH`] because a shallower
/// reading is the box's growth margin rather than a balloon, and
/// [`MAX_FILL_DEPTH`] because past it the only balloons still answering are the
/// roomy ones.
fn required_depth(bbox: Rect) -> u32 {
    (bbox.w.min(bbox.h) / 10).clamp(MIN_FILL_DEPTH, MAX_FILL_DEPTH)
}

/// How far out the walk may go before it gives up. Three times the depth it is
/// looking for, so a band interrupted by an outline still has room to have been
/// deep enough before it.
fn scan_depth(bbox: Rect) -> u32 {
    (required_depth(bbox) * 3).clamp(9, 32)
}

/// The pixels of one ring, as four segments: the paper directly above, below,
/// left of and right of the box, each exactly as long as the side it faces.
///
/// The order within a segment is what makes the run count in [`RingRead`] mean
/// anything: an outline crosses a segment in a contiguous arc and a halftone
/// crosses it once per dot, and the two are told apart by walking the segment
/// rather than by counting its pixels.
///
/// **Nothing near a corner is walked, and that is the whole shape of this
/// function.** A closed rectangular ring around a text box inside a *rounded*
/// balloon leaves the balloon at its four corners long before it leaves it
/// anywhere else: a text block filling two thirds of an ellipse has tens of
/// pixels of clearance on the axes and none at all on the diagonal, so ring 1
/// already reads the art beyond the rim, that art is two-sided screentone, and
/// the walk calls picture at depth 0 - [`Interior::Textured`], which overrules a
/// detector that was right.
///
/// Every balloon shape this application meets is convex or nearly so, and on a
/// convex shape the clearance from a box's side is largest at that side's
/// midpoint and falls away towards both of its ends. So each segment is the
/// **middle half** of the side it faces, given by [`SIDE_INSET_QUARTERS`]: the
/// part of the paper the balloon actually reserves for the text, and the part a
/// letterer leaves room in. The ends of a full-length segment are still a box
/// corner's own distance off the axis, which on an eccentric ellipse is already
/// outside the rim at offset 1 - half a side is not a softening of the corner
/// rule but the rest of it.
fn ring_sides(bbox: Rect, offset: u32) -> [Vec<(i64, i64)>; 4] {
    let r = offset as i64;
    let inset = |side: u32| (side / SIDE_INSET_QUARTERS) as i64;
    let (ix, iy) = (inset(bbox.w), inset(bbox.h));
    let (x0, x1) = (bbox.x + ix, bbox.right() - ix);
    let (y0, y1) = (bbox.y + iy, bbox.bottom() - iy);
    let top = (x0..x1).map(|x| (x, bbox.y - r)).collect();
    let bottom = (x0..x1).map(|x| (x, bbox.bottom() - 1 + r)).collect();
    let left = (y0..y1).map(|y| (bbox.x - r, y)).collect();
    let right = (y0..y1).map(|y| (bbox.right() - 1 + r, y)).collect();
    [top, bottom, left, right]
}

/// One ring measured against a fill level.
struct RingRead {
    /// On-page pixels of the ring, text included: what the ring would have held
    /// if none of it were lettering.
    on_page: usize,
    samples: usize,
    off: usize,
    darker: usize,
    lighter: usize,
    /// Maximal contiguous runs of off-fill pixels, counted along each of
    /// [`ring_sides`]' four segments and summed. A run that would have spanned
    /// two segments counts once in each, which is the conservative direction:
    /// it can only move a reading away from *outline* and towards *picture*.
    runs: usize,
}

impl RingRead {
    /// Whether enough of the ring survived the text mask to be a sample of it.
    fn is_readable(&self) -> bool {
        self.samples >= MIN_RING_SAMPLES
            && self.samples * 100 >= self.on_page * MIN_RING_COVERAGE_PERCENT
    }

    fn is_fill(&self) -> bool {
        self.off * 100 <= self.samples * (100 - FILL_SHARE_PERCENT)
    }

    /// Whether what is off the fill is all on one side of it - all darker, or
    /// all lighter. Ink is one-sided; art is not. Vacuously true when nothing
    /// is off the fill at all.
    fn is_one_sided(&self) -> bool {
        self.darker.max(self.lighter) * 100 >= self.off * ONE_SIDED_PERCENT
    }

    /// Ink: few runs, and all of it on one side of the fill. A ring lying
    /// wholly on a thick outline is one run of one-sided pixels and reads here,
    /// which is the case a share threshold would have thrown away - the balloon
    /// whose lettering nearly fills it.
    fn is_outline(&self) -> bool {
        if self.runs > MAX_OUTLINE_RUNS {
            return false;
        }
        self.off > 0 && self.is_one_sided()
    }
}

fn read_ring(
    page: &Raster,
    text: &Mask,
    bbox: Rect,
    offset: u32,
    level: u16,
    tolerance: u16,
) -> RingRead {
    let mut read = RingRead { on_page: 0, samples: 0, off: 0, darker: 0, lighter: 0, runs: 0 };
    for side in ring_sides(bbox, offset) {
        // Each segment is walked on its own, so a run does not carry across the
        // gap between two of them.
        let mut previous_off = false;
        for (x, y) in side {
            if x < 0 || y < 0 || x >= page.width as i64 || y >= page.height as i64 {
                continue;
            }
            read.on_page += 1;
            if text.contains(x, y) {
                continue;
            }
            let luma = page.luma16_at(x as u32, y as u32);
            let off = luma.abs_diff(level) > tolerance;
            read.samples += 1;
            if off {
                read.off += 1;
                if luma < level {
                    read.darker += 1;
                } else {
                    read.lighter += 1;
                }
                if !previous_off {
                    read.runs += 1;
                }
            }
            previous_off = off;
        }
    }
    read
}

/// The fill the band is measured against, and how far from it the band may
/// stray: the median of the innermost rings, and their spread.
///
/// Both taken from the page rather than assumed. The level, because "white or
/// black" is not the choice - cream paper, newsprint and a screened grey
/// balloon are all fills, and [`crate::fit::ring`]'s refusal to snap a
/// near-white median to white is the same refusal one ring further in. The
/// tolerance, because a scan's noise is not the choice either: the
/// [`TOLERANCE_PERCENTILE`] of the rings' own deviations from the median, given
/// [`TOLERANCE_MARGIN`], and held between [`FILL_TOLERANCE`] and
/// [`MAX_FILL_TOLERANCE`]. A clean page measures a spread of zero and keeps the
/// floor; a noisy one widens to what its paper actually does; a screentone
/// measures its dots and is stopped at the cap, where the dots are still off
/// the fill.
fn fill_band(page: &Raster, text: &Mask, bbox: Rect) -> Option<(u16, u16)> {
    let mut values: Vec<u16> = Vec::new();
    for offset in 1..=MIN_FILL_DEPTH {
        for (x, y) in ring_sides(bbox, offset).into_iter().flatten() {
            if x < 0 || y < 0 || x >= page.width as i64 || y >= page.height as i64 {
                continue;
            }
            if text.contains(x, y) {
                continue;
            }
            values.push(page.luma16_at(x as u32, y as u32));
        }
    }
    band_of(values)
}

/// The level and tolerance of a set of paper samples, however they were
/// gathered.
///
/// Lifted out of [`fill_band`] so that the reading *inside* a box
/// ([`inner_paper`]) is measured by the same machinery as the rings around it:
/// the median as the fill, and the [`TOLERANCE_PERCENTILE`] of the samples'
/// own deviations from it - given [`TOLERANCE_MARGIN`] and held between
/// [`FILL_TOLERANCE`] and [`MAX_FILL_TOLERANCE`] - as how far from it a pixel
/// may sit and still be that fill.
fn band_of(mut values: Vec<u16>) -> Option<(u16, u16)> {
    if values.len() < MIN_RING_SAMPLES {
        return None;
    }
    values.sort_unstable();
    let level = values[values.len() / 2];

    let mut deviations: Vec<u16> = values.iter().map(|v| v.abs_diff(level)).collect();
    deviations.sort_unstable();
    let at = (deviations.len() * TOLERANCE_PERCENTILE / 100).min(deviations.len() - 1);
    let spread = deviations[at] as u32 * TOLERANCE_MARGIN.0 / TOLERANCE_MARGIN.1;
    let tolerance = (spread.min(MAX_FILL_TOLERANCE as u32) as u16).max(FILL_TOLERANCE);
    Some((level, tolerance))
}

/// The text pixels around a box, grown by [`TEXT_HALO`], over the area the walk
/// will read.
///
/// **Every** text pixel, not this region's: a neighbouring column of lettering
/// standing in the band is the region's own problem to ignore in exactly the
/// same way, and the segmentation mask does not distinguish them.
/// Grown by scattering rather than by [`Mask::dilated_within`], which is the
/// same disc and the same result: a `flagged_large` region's box is a quarter of
/// a page, and a dilation that visits every pixel of it would pay a
/// twenty-five-fold cost over an interior this walk never reads.
fn text_halo(seg: &Segmentation, area: Rect) -> Mask {
    let radius = TEXT_HALO as i64;
    let mut mask = Mask::empty(area);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if x < 0 || y < 0 || x >= seg.width as i64 || y >= seg.height as i64 {
                continue;
            }
            if !seg.is_text(x as u32, y as u32) {
                continue;
            }
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx * dx + dy * dy <= radius * radius {
                        mask.set(x + dx, y + dy, true);
                    }
                }
            }
        }
    }
    mask
}

/// The paper **between** the strokes, if it is one uniform fill.
///
/// The ring walk asks what is around a text box. That question has an answer
/// only where there is room for one, and a rectangular narration box is drawn
/// with none: its frame sits tight against the lettering, so ring 1 is already
/// on the frame, the frame is not deep enough to be a band, and the walk
/// returns [`Interior::Textured`] - picture - about a white box with black
/// text in it. Four of them across the reference scans, every one of them real
/// narration going to review under *"text outside a speech bubble"* while the
/// page's balloons cleaned.
///
/// So this reads the other side of the same paper. Every on-page pixel of the
/// box that the (already grown) text mask does not claim is a sample; the
/// samples must number at least [`MIN_RING_SAMPLES`], make up at least
/// [`MIN_INNER_PAPER_PERCENT`] of the box, and leave *something* to the text
/// mask; and they are then measured by the
/// machinery the rings are measured by - [`band_of`] for the level and the
/// tolerance, [`ONE_SIDED_PERCENT`] for what falls off it, and
/// [`INNER_OFF_PERCENT`] for how much may.
///
/// **The two thresholds do different jobs and both are needed.** One-sidedness
/// refuses art, which strays both ways, while admitting the thing this exists
/// for: a frame line clipped into the box, or an antialiased edge the halo did
/// not reach, is ink, and ink is darker than the paper and nothing else.
/// [`INNER_OFF_PERCENT`] then refuses screentone, which is one-sided too - a
/// tone's dots are far off the fill, the tolerance is capped at
/// [`MAX_FILL_TOLERANCE`] so it cannot widen to swallow them, and any visible
/// density covers more of its box than lettering and its frame do.
///
/// Returns the fill level. `None` is *not flat* and *not enough to say* alike  - 
/// the caller has the ring walk's answer for both.
fn inner_paper(page: &Raster, text: &Mask, bbox: Rect) -> Option<u16> {
    let (level, read) = inner_read(page, text, bbox)?;
    // `runs` is left at zero and `is_outline` is never asked: a run is a fact
    // about walking a line, and this is a set of scattered pixels with no line
    // to walk. The one-sidedness is asked directly instead, and the share it is
    // asked alongside is the inside's own.
    let within = read.off * 100 <= read.samples * INNER_OFF_PERCENT;
    (within && read.is_one_sided()).then_some(level)
}

/// The counts [`inner_paper`] decides on: the fill level of the paper between
/// the strokes and how the rest of it sits against that fill. Separate so a
/// test can say *which* count refused a page rather than only that one did.
fn inner_read(page: &Raster, text: &Mask, bbox: Rect) -> Option<(u16, RingRead)> {
    let mut on_page = 0usize;
    let mut values: Vec<u16> = Vec::new();
    for y in bbox.y..bbox.bottom() {
        for x in bbox.x..bbox.right() {
            if x < 0 || y < 0 || x >= page.width as i64 || y >= page.height as i64 {
                continue;
            }
            on_page += 1;
            if text.contains(x, y) {
                continue;
            }
            values.push(page.luma16_at(x as u32, y as u32));
        }
    }
    if values.len() < MIN_RING_SAMPLES
        || values.len() * 100 < on_page * MIN_INNER_PAPER_PERCENT
        // The paper *between the strokes* presupposes strokes. A rectangle the
        // text mask claims nothing in is not a text box being read from the
        // inside, it is a patch of the page - and a blank patch of any picture
        // is uniform. The walk outside is the only opinion worth having about
        // one, so this hands it back rather than answering *balloon* about
        // every flat rectangle on the page.
        || values.len() == on_page
    {
        return None;
    }
    let (level, tolerance) = band_of(values.clone())?;

    let mut read =
        RingRead { on_page, samples: values.len(), off: 0, darker: 0, lighter: 0, runs: 0 };
    for luma in values {
        if luma.abs_diff(level) <= tolerance {
            continue;
        }
        read.off += 1;
        if luma < level {
            read.darker += 1;
        } else {
            read.lighter += 1;
        }
    }
    Some((level, read))
}

/// [`interior`], with the text mask taken from the detector's segmentation.
pub fn interior_of(page: &Raster, seg: &Segmentation, bbox: Rect) -> Interior {
    let reach = scan_depth(bbox) + TEXT_HALO + 1;
    let area = bbox.grown(reach, page.width, page.height);
    interior(page, &text_halo(seg, area), bbox)
}

/// Walk outward from `bbox` and say what the paper around it is.
///
/// `text` is a mask of the page's text strokes, already grown - the pixels this
/// walk must not read, because a glyph in the band is not evidence about the
/// band. [`interior_of`] builds it from a [`Segmentation`]; a caller with a
/// mask of its own passes that.
///
/// The walk takes one ring per native pixel of offset and stops at the first
/// one that is not the fill:
///
/// - **Fill** - [`FILL_SHARE_PERCENT`] of the ring within [`FILL_TOLERANCE`] of
///   the level the innermost rings set. Deepens the band.
/// - **Outline** - off-fill pixels in at most [`MAX_OUTLINE_RUNS`] runs, all on
///   one side of the fill. Stops the walk *and is evidence*: a thin ink line
///   with uniform fill behind it is what a balloon's edge is. Nothing beyond it
///   is read at all, which is the point - beyond a balloon's outline is not the
///   balloon.
/// - **Picture** - anything else. Stops the walk, and with no band behind it
///   the region is out of a balloon.
///
/// **The walk is the first opinion, not the only one.** Where it finds a band
/// it is answered and nothing else is asked; where it does not - picture at
/// depth 0, or too little paper to say - the paper *between* the strokes is
/// read instead ([`inner_paper`]), and a box whose inside is one uniform fill
/// is [`Interior::Solid`] at `depth: 0`. The order matters in one direction
/// only: a walk that found a real band has measured a real balloon, and the
/// inside can add nothing to that. The reverse is not true, which is the whole
/// reason for the second reading - a narration box has no band to find and is
/// still white paper with black text on it.
pub fn interior(page: &Raster, text: &Mask, bbox: Rect) -> Interior {
    if bbox.w == 0 || bbox.h == 0 {
        return Interior::Unreadable;
    }
    let walked = ring_walk(page, text, bbox);
    if matches!(walked, Interior::Solid { .. }) {
        return walked;
    }
    match inner_paper(page, text, bbox) {
        Some(level) => Interior::Solid { level, depth: 0 },
        None => walked,
    }
}

/// The walk itself: [`interior`] less its second opinion.
fn ring_walk(page: &Raster, text: &Mask, bbox: Rect) -> Interior {
    let Some((level, tolerance)) = fill_band(page, text, bbox) else {
        return Interior::Unreadable;
    };
    let wanted = required_depth(bbox);

    let mut depth = 0u32;
    let mut stopped_on_outline = false;
    let mut stopped_on_picture = false;
    for offset in 1..=scan_depth(bbox) {
        let read = read_ring(page, text, bbox, offset, level, tolerance);
        if !read.is_readable() {
            // Nothing to read at this offset - a page edge, or a ring the
            // lettering fills. Neither is evidence either way, and the band is
            // not broken by it: the walk carries its depth past it.
            continue;
        }
        if read.is_fill() {
            depth += 1;
            if depth >= wanted {
                return Interior::Solid { level, depth };
            }
            continue;
        }
        stopped_on_outline = read.is_outline();
        stopped_on_picture = !stopped_on_outline;
        break;
    }

    if depth >= wanted || (stopped_on_outline && depth >= MIN_FILL_DEPTH) {
        return Interior::Solid { level, depth };
    }
    if stopped_on_picture && depth < MIN_FILL_DEPTH {
        return Interior::Textured;
    }
    // A band that was going the right way and ran out. Not a verdict: the
    // detector's answer is better than this one.
    Interior::Unreadable
}

/// A line across the strip between two boxes counts as crossing something when
/// this many of its pixels are off the fill. Three rather than one: a scan
/// speckles, and one pixel is a speckle where three in a row is a stroke.
const MIN_OFF_PER_LINE: usize = 3;

/// And the strip is a boundary when this share of its lines, in percent, cross
/// something. A rim runs the whole length of the strip it stands in, so a
/// balloon's edge is near every line of it; a stray mark on the paper is on a
/// few.
const BOUNDARY_LINE_PERCENT: usize = 60;

/// How much of a line may be sampled off-fill and the strip still be one fill.
/// Read as the complement: a strip whose lines are clean is one balloon's
/// paper.
const CLEAN_LINE_PERCENT: usize = 100 - BOUNDARY_LINE_PERCENT;


/// How thick a slice is taken off the far end of an overhanging box, and the
/// largest share of that box's own extent it may be. A slice is a place to
/// stand, not a measurement of its own.
const FAR_SLICE: i64 = 20;

/// Whether `other`'s far end is on different paper from `near`.
///
/// Only asked of a box that overhangs - one wholly inside the other has no far
/// end to reach anywhere - and answered by the same strip reading as the rest of
/// [`merge_crosses_a_balloon`], between `near` and a slice at that far end.
fn far_edge_crosses(page: &Raster, seg: &Segmentation, near: Rect, other: Rect) -> bool {
    let thickness = |overhang: i64| FAR_SLICE.min(overhang / 2).max(1);
    // The overhang must be worth reading: a box a few pixels longer than its
    // neighbour is the same box. A box wholly inside the other has no far end
    // to reach anywhere and never gets here.
    let slice = if other.right() - near.right() > FAR_SLICE {
        let t = thickness(other.right() - near.right());
        Rect { x: other.right() - t, y: other.y, w: t as u32, h: other.h }
    } else if near.x - other.x > FAR_SLICE {
        let t = thickness(near.x - other.x);
        Rect { x: other.x, y: other.y, w: t as u32, h: other.h }
    } else if other.bottom() - near.bottom() > FAR_SLICE {
        let t = thickness(other.bottom() - near.bottom());
        Rect { x: other.x, y: other.bottom() - t, w: other.w, h: t as u32 }
    } else if near.y - other.y > FAR_SLICE {
        let t = thickness(near.y - other.y);
        Rect { x: other.x, y: other.y, w: other.w, h: t as u32 }
    } else {
        return false;
    };
    // If `near` still meets the slice there is no reach to measure, and that is
    // also what keeps the recursion one level deep.
    let x_gap = near.x.max(slice.x) - near.right().min(slice.right());
    let y_gap = near.y.max(slice.y) - near.bottom().min(slice.bottom());
    if x_gap <= 0 && y_gap <= 0 {
        return false;
    }
    merge_crosses_a_balloon(page, seg, near, slice)
}

/// Whether merging two boxes would cross a balloon.
///
/// **The question [`crate::detect::build_separated`] asks before it merges.**
/// Two text boxes in one balloon have that balloon's own fill between them; two
/// text boxes in *different* balloons have a rim, the art beyond it, and a
/// second rim. The merge rules in [`crate::detect`] cannot tell those apart -
/// they see two rectangles and an overlap fraction - and when they join the
/// second pair the result is one region whose box spans both balloons and the
/// artwork between them. That region then fails this module's other test too:
/// its surround is picture, so the gate sends ordinary dialogue to review under
/// *"text outside a speech bubble"*.
///
/// **Counted line by line, not as a share of the strip.** Two white balloons
/// have the *same* interior level, so a strip spanning both is nine tenths that
/// one level and the rim between them is a rounding error in any area
/// statistic. What separates them is that the rim is on every line of the strip
/// and a balloon's own fill is on none, so the strip is read as a bundle of
/// crossings: each line perpendicular to the gap either meets something or does
/// not, and [`BOUNDARY_LINE_PERCENT`] of them meeting something is a boundary.
///
/// Two boxes sharing no extent in either axis are refused whatever the paper
/// says: they are joined by a diagonal rather than by a strip, their hull covers
/// ground belonging to neither, and there is no reading of that hull which is a
/// reading of the thing being merged.
///
/// Boxes that already overlap have no strip between them and are never refused
/// here. That is not a gap in the test but the reason [`crate::detect`] asks it
/// of **every member** of a group rather than only of the box being absorbed: a
/// detector box bridging two balloons overlaps both, and it is the two text
/// blocks either side of it, not the bridge, that the strip is read between.
pub fn merge_crosses_a_balloon(page: &Raster, seg: &Segmentation, a: Rect, b: Rect) -> bool {
    let x_gap = a.x.max(b.x) - a.right().min(b.right());
    let y_gap = a.y.max(b.y) - a.bottom().min(b.bottom());
    if x_gap <= 0 && y_gap <= 0 {
        // Overlapping. There is no strip between them, but a merge claims more
        // than "these touch": it claims one text block on one piece of paper,
        // and the claim has to hold at the far end of the box that reaches
        // furthest. A detector box bridging two balloons overlaps the lettering
        // in the first one and ends inside the second, and this is where that
        // is caught - the strip between the near box and the far box's own far
        // edge is the rim, the art, and the second rim.
        return far_edge_crosses(page, seg, a, b) || far_edge_crosses(page, seg, b, a);
    }
    // Side by side: the strip is the columns between them over the rows they
    // share, and each of those rows is one crossing. Stacked: the other way
    // about. Diagonal: no strip at all.
    let (strip, across_rows) = if y_gap <= 0 && x_gap > 0 {
        let y = a.y.max(b.y);
        let bottom = a.bottom().min(b.bottom());
        (
            Rect { x: a.right().min(b.right()), y, w: x_gap as u32, h: (bottom - y) as u32 },
            true,
        )
    } else if x_gap <= 0 && y_gap > 0 {
        let x = a.x.max(b.x);
        let right = a.right().min(b.right());
        (
            Rect { x, y: a.bottom().min(b.bottom()), w: (right - x) as u32, h: y_gap as u32 },
            false,
        )
    } else {
        return true;
    };
    if strip.w == 0 || strip.h == 0 {
        return true;
    }

    let text = text_halo(seg, strip.grown(TEXT_HALO, page.width, page.height));
    let readable = |x: i64, y: i64| {
        x >= 0
            && y >= 0
            && x < page.width as i64
            && y < page.height as i64
            && !text.contains(x, y)
    };

    // The fill the crossings are measured against: the strip's own median, the
    // same refusal to assume white that [`fill_level`] makes.
    let mut values: Vec<u16> = Vec::new();
    for y in strip.y..strip.bottom() {
        for x in strip.x..strip.right() {
            if readable(x, y) {
                values.push(page.luma16_at(x as u32, y as u32));
            }
        }
    }
    if values.len() < MIN_RING_SAMPLES {
        // Nothing readable between them. Not evidence of a boundary.
        return false;
    }
    values.sort_unstable();
    let level = values[values.len() / 2];

    let (lines, along): (Vec<i64>, Vec<i64>) = if across_rows {
        ((strip.y..strip.bottom()).collect(), (strip.x..strip.right()).collect())
    } else {
        ((strip.x..strip.right()).collect(), (strip.y..strip.bottom()).collect())
    };
    let mut crossed = 0usize;
    let mut read = 0usize;
    for line in &lines {
        let mut off = 0usize;
        let mut samples = 0usize;
        for step in &along {
            let (x, y) = if across_rows { (*step, *line) } else { (*line, *step) };
            if !readable(x, y) {
                continue;
            }
            samples += 1;
            if page.luma16_at(x as u32, y as u32).abs_diff(level) > FILL_TOLERANCE {
                off += 1;
            }
        }
        if samples == 0 {
            continue;
        }
        read += 1;
        if off >= MIN_OFF_PER_LINE || off * 100 > samples * CLEAN_LINE_PERCENT {
            crossed += 1;
        }
    }
    read > 0 && crossed * 100 >= read * BOUNDARY_LINE_PERCENT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn balloon(x: i64, y: i64, w: u32, h: u32, class: BalloonClass) -> BalloonBox {
        BalloonBox { rect: Rect::new(x, y, w, h), class, score: 0.9 }
    }

    /* ---- adopt_uncovered_text ---- */

    use crate::detect::{DetBox, build_regions};

    fn text_region(x: i64, y: i64, w: u32, h: u32) -> Vec<Region> {
        build_regions(
            vec![DetBox {
                rect: Rect::new(x, y, w, h),
                confidence: 0.9,
                language: DetectedLanguage::Japanese,
            }],
            1000,
            1000,
        )
    }

    #[test]
    fn a_text_box_no_region_covers_is_adopted() {
        let regions = text_region(0, 0, 60, 60);
        let balloons = [balloon(400, 400, 120, 80, BalloonClass::TextFree)];
        let adopted = adopt_uncovered_text(&regions, &balloons, 1000, 1000, 3_600);
        assert_eq!(adopted.len(), 1);
        // Grown exactly as a detector box is: +2 all sides and +3 right for the
        // tight tier, +5 more all round for the extended tier, which is the
        // masking box, and +20 for the reference.
        let region = &adopted[0];
        assert_eq!(region.members[0].tight, Rect::new(398, 398, 125, 84));
        assert_eq!(region.masking, Rect::new(393, 393, 135, 94));
        assert_eq!(region.reference, region.masking.grown(20, 1000, 1000));
        assert_eq!(region.members[0].confidence, 0.9);
        assert!(!region.flagged_large);
    }

    #[test]
    fn a_text_box_whose_centre_a_region_holds_is_not_adopted() {
        let regions = text_region(400, 400, 120, 80);
        let balloons = [balloon(440, 420, 30, 30, BalloonClass::TextFree)];
        assert!(adopt_uncovered_text(&regions, &balloons, 1000, 1000, 3_600).is_empty());
    }

    /// The centre alone is not enough: a box reaching well into a region from
    /// outside is the same text seen twice, and the merge's own threshold is
    /// what says so.
    #[test]
    fn a_text_box_overlapping_a_region_is_not_adopted() {
        let regions = text_region(400, 400, 200, 200);
        // Centre at (700, 500), outside the region, but half of this box lies
        // inside it.
        let balloons = [balloon(500, 450, 400, 100, BalloonClass::TextFree)];
        assert!(adopt_uncovered_text(&regions, &balloons, 1000, 1000, 3_600).is_empty());
    }

    #[test]
    fn a_bubble_shape_is_not_text_and_is_never_adopted() {
        let regions = text_region(0, 0, 60, 60);
        let balloons = [balloon(400, 400, 120, 80, BalloonClass::Bubble)];
        assert!(adopt_uncovered_text(&regions, &balloons, 1000, 1000, 3_600).is_empty());
    }

    #[test]
    fn a_text_box_the_detector_is_unsure_of_is_not_adopted() {
        let regions = text_region(0, 0, 60, 60);
        let weak = BalloonBox {
            rect: Rect::new(400, 400, 120, 80),
            class: BalloonClass::TextFree,
            score: SURE_SCORE - 0.01,
        };
        assert!(adopt_uncovered_text(&regions, &[weak], 1000, 1000, 3_600).is_empty());
    }

    /// A `text_bubble` box is adopted on the same terms. It almost never
    /// happens - over the 28 real scans the text detector emitted a box for
    /// every one of them - but a balloon whose lettering the text detector
    /// missed is the same miss as a narration box's.
    #[test]
    fn a_text_in_bubble_box_no_region_covers_is_adopted_too() {
        let regions = text_region(0, 0, 60, 60);
        let balloons = [balloon(400, 400, 120, 80, BalloonClass::TextInBubble)];
        assert_eq!(adopt_uncovered_text(&regions, &balloons, 1000, 1000, 3_600).len(), 1);
    }

    /// 01.png's chapter title strip: 78×1146 down the side of a 1536-row page,
    /// which is taller than a quarter of it. Flagged for review, not dropped
    /// and not silently cleaned.
    #[test]
    fn an_enormous_adopted_box_is_flagged_large() {
        let regions = text_region(0, 0, 60, 60);
        let balloons = [balloon(1000, 70, 78, 1146, BalloonClass::TextFree)];
        let adopted = adopt_uncovered_text(&regions, &balloons, 1080, 1536, 3_600);
        assert!(adopted[0].flagged_large);
    }

    /// And the other half of the size rules stays off. A small adopted box
    /// exists *because* the text detector emitted nothing there, so measuring
    /// it against the median of what the text detector did emit and dropping
    /// it would throw away the only evidence the page has.
    #[test]
    fn a_small_adopted_box_is_never_dropped() {
        let regions = text_region(0, 0, 200, 200);
        let balloons = [BalloonBox {
            rect: Rect::new(600, 600, 20, 20),
            class: BalloonClass::TextFree,
            score: 0.55,
        }];
        // 400 px² against a 40 000 px² median is far under the 0.15 floor, and
        // 0.55 is under `SIZE_DROP_MAX_CONFIDENCE`, so the detector's own rule
        // would drop this box.
        assert_eq!(adopt_uncovered_text(&regions, &balloons, 1000, 1000, 40_000).len(), 1);
    }

    /// The balloon detector emits more than one box for one piece of text - on
    /// 01.png nine `text_bubble` boxes stand over six regions - and two of a
    /// duplicate pair are not two regions. The stronger one is taken and the
    /// weaker is then covered by it, on exactly the tests a text-detector
    /// region would have covered it with.
    #[test]
    fn two_boxes_over_one_piece_of_text_become_one_region() {
        let regions = text_region(0, 0, 60, 60);
        let weaker =
            BalloonBox { rect: Rect::new(400, 400, 120, 80), class: BalloonClass::TextFree, score: 0.6 };
        let stronger =
            BalloonBox { rect: Rect::new(410, 405, 120, 80), class: BalloonClass::TextFree, score: 0.9 };
        let adopted = adopt_uncovered_text(&regions, &[weaker.clone(), stronger.clone()], 1000, 1000, 3_600);
        assert_eq!(adopted.len(), 1, "a duplicate pair became two regions");
        assert_eq!(adopted[0].members[0].confidence, 0.9, "the model's own best answer is the one kept");
        assert_eq!(adopted[0].members[0].tight, Rect::new(408, 403, 125, 84));

        // And the order the detector returned them in does not decide it.
        let reversed = adopt_uncovered_text(&regions, &[stronger, weaker], 1000, 1000, 3_600);
        assert_eq!(reversed, adopted);
    }

    /// Two boxes far enough apart are two pieces of text, and the chain must
    /// not swallow the second.
    #[test]
    fn two_boxes_over_different_text_stay_two_regions() {
        let regions = text_region(0, 0, 60, 60);
        let balloons = [
            balloon(400, 400, 120, 80, BalloonClass::TextFree),
            balloon(700, 700, 120, 80, BalloonClass::TextFree),
        ];
        assert_eq!(adopt_uncovered_text(&regions, &balloons, 1000, 1000, 3_600).len(), 2);
    }

    /// The ids a run hands out are list indices, so the two sources have to end
    /// up in one reading order rather than appended.
    #[test]
    fn the_adopted_regions_sort_into_reading_order_with_the_rest() {
        let mut regions = text_region(400, 400, 60, 60);
        let balloons = [balloon(100, 100, 80, 60, BalloonClass::TextFree)];
        let adopted = adopt_uncovered_text(&regions, &balloons, 1000, 1000, 3_600);
        regions.extend(adopted);
        crate::detect::sort_regions(&mut regions);
        assert_eq!(regions[0].masking.y, 93, "the adopted box is higher up the page");
        assert_eq!(regions[1].masking.y, 393);
    }

    #[test]
    fn a_region_inside_a_bubble_is_in_a_balloon() {
        let balloons = [balloon(0, 0, 200, 200, BalloonClass::Bubble)];
        assert!(in_balloon(Rect::new(50, 50, 40, 40), &balloons));
    }

    #[test]
    fn a_region_with_no_balloon_over_it_is_not() {
        let balloons = [balloon(500, 500, 200, 200, BalloonClass::Bubble)];
        assert!(!in_balloon(Rect::new(50, 50, 40, 40), &balloons));
        assert!(!in_balloon(Rect::new(50, 50, 40, 40), &[]));
    }

    #[test]
    fn text_free_overrides_a_bubble_that_happens_to_overlap() {
        // A sound effect over art, with a large balloon box reaching across it.
        let balloons = [
            balloon(0, 0, 1000, 1000, BalloonClass::Bubble),
            balloon(400, 400, 200, 200, BalloonClass::TextFree),
        ];
        assert!(!in_balloon(Rect::new(450, 450, 100, 100), &balloons));
    }

    #[test]
    fn containment_is_of_the_centre_rather_than_of_any_corner() {
        // A tall sound effect whose top corner clips a balloon.
        let balloons = [balloon(0, 0, 200, 200, BalloonClass::Bubble)];
        assert!(!in_balloon(Rect::new(150, 150, 400, 400), &balloons));
    }

    /// The jagged balloon the paper walk reads as picture. A sure `bubble` box
    /// drawn around the whole of the text is the model answering the balloon
    /// question, and it grades as strongly as a `text_bubble` would.
    #[test]
    fn a_sure_bubble_enclosing_the_whole_region_grades_as_the_model_s_own_answer() {
        let balloons = [balloon(400, 100, 220, 480, BalloonClass::Bubble)];
        let region = Rect::new(414, 115, 177, 430);
        assert_eq!(detected(region, &balloons), Detected::TextInBubble);
        assert!(
            Interior::Textured.settles(detected(region, &balloons)),
            "a spiky outline read as picture must not veto an enclosing balloon"
        );
    }

    /// 02.png's own geometry: one two-lobed balloon, two `bubble` boxes that
    /// meet at a single row, and a region under both. No box covers even half of
    /// it, and their union covers nearly all of it.
    #[test]
    fn two_lobes_of_one_balloon_enclose_a_region_neither_of_them_does() {
        let balloons = [
            BalloonBox {
                rect: Rect::new(429, 53, 201, 266),
                class: BalloonClass::Bubble,
                score: 0.92,
            },
            BalloonBox {
                rect: Rect::new(389, 319, 190, 264),
                class: BalloonClass::Bubble,
                score: 0.92,
            },
        ];
        let region = Rect::new(414, 115, 177, 430);
        for one in &balloons {
            assert!(
                covered_percent(region, &[one.rect]) < ENCLOSED_SHARE_PERCENT,
                "one lobe alone must not carry this"
            );
        }
        assert_eq!(detected(region, &balloons), Detected::TextInBubble);
    }

    /// And the union is a union: two boxes over the same paper do not add up to
    /// a covered region.
    #[test]
    fn overlapping_shapes_are_not_counted_twice() {
        let region = Rect::new(0, 0, 100, 100);
        let half = Rect::new(0, 0, 50, 100);
        assert_eq!(covered_percent(region, &[half, half]), 50);
        assert_eq!(covered_percent(region, &[half, Rect::new(40, 0, 60, 100)]), 100);
        assert_eq!(covered_percent(region, &[Rect::new(500, 500, 10, 10)]), 0);
    }

    /// And the case enclosure is narrow enough to exclude: a large `bubble` box
    /// reaching across a sound effect holds its centre but not its extent.
    #[test]
    fn a_sure_bubble_holding_only_the_centre_is_still_only_a_shape() {
        let balloons = [balloon(0, 0, 400, 400, BalloonClass::Bubble)];
        let region = Rect::new(100, 100, 500, 500);
        assert_eq!(detected(region, &balloons), Detected::Bubble);
        assert!(!Interior::Textured.settles(detected(region, &balloons)));
    }

    /// A `bubble` box that only just cleared emission is not the model being
    /// sure of anything, however much of the region it covers.
    #[test]
    fn an_unsure_bubble_does_not_grade_up_however_much_it_encloses() {
        let weak = BalloonBox {
            rect: Rect::new(0, 0, 400, 400),
            class: BalloonClass::Bubble,
            score: 0.4,
        };
        assert_eq!(detected(Rect::new(100, 100, 60, 60), &[weak]), Detected::Bubble);
    }

    /// Text drawn over art with no balloon behind it stays outside, enclosing
    /// `bubble` box or not: `text_free` is a statement about *this text*.
    /// Two sure shapes that meet, but together cover under three quarters of
    /// the region: the model has not said the text is enclosed.
    #[test]
    fn two_touching_shapes_under_the_share_stay_a_shape() {
        let region = Rect::new(0, 0, 100, 100);
        let balloons = [
            BalloonBox { rect: Rect::new(0, 0, 100, 40), class: BalloonClass::Bubble, score: 0.9 },
            BalloonBox { rect: Rect::new(0, 40, 100, 20), class: BalloonClass::Bubble, score: 0.9 },
        ];
        assert_eq!(detected(region, &balloons), Detected::Bubble);
    }

    /// A neighbouring balloon whose box reaches across the region but never
    /// touches the shape holding the centre lends nothing to the share.
    #[test]
    fn a_detached_neighbour_does_not_lend_its_area() {
        let region = Rect::new(0, 0, 100, 100);
        let balloons = [
            BalloonBox { rect: Rect::new(0, 0, 100, 60), class: BalloonClass::Bubble, score: 0.9 },
            BalloonBox { rect: Rect::new(0, 70, 100, 30), class: BalloonClass::Bubble, score: 0.9 },
        ];
        assert_eq!(detected(region, &balloons), Detected::Bubble);
        // Joined, the same two boxes are one balloon and the share is 100.
        let joined = [
            balloons[0].clone(),
            BalloonBox { rect: Rect::new(0, 60, 100, 40), class: BalloonClass::Bubble, score: 0.9 },
        ];
        assert_eq!(detected(region, &joined), Detected::TextInBubble);
    }

    #[test]
    fn text_free_still_wins_over_an_enclosing_bubble() {
        let balloons = [
            balloon(0, 0, 1000, 1000, BalloonClass::Bubble),
            balloon(400, 400, 200, 200, BalloonClass::TextFree),
        ];
        let region = Rect::new(450, 450, 100, 100);
        assert_eq!(detected(region, &balloons), Detected::Outside);
        assert!(!Interior::Textured.settles(detected(region, &balloons)));
    }

    // The paper reading. Every page below is 8-bit grey, because §4 makes the
    // grayscale path canonical and this walk is one of its measurements.

    use crate::detect::{Letterbox, Segmentation};
    use crate::image::{BitDepth, ColorMode};

    fn gray_page(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Raster {
        let mut data = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                data.push(f(x, y));
            }
        }
        Raster {
            width: w,
            height: h,
            mode: ColorMode::Gray,
            depth: BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data,
        }
    }

    /// A segmentation whose text pixels are wherever `f` says.
    fn segmentation(w: u32, h: u32, f: impl Fn(u32, u32) -> bool) -> Segmentation {
        let mut levels = vec![0u8; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                if f(x, y) {
                    levels[(y * w + x) as usize] = 255;
                }
            }
        }
        Segmentation { width: w, height: h, levels, fit: Letterbox::fit(w, h) }
    }

    /// The region every test below reads around: 60×60 at the centre of a
    /// 240×240 page, which asks for six pixels of fill (60 / 10).
    const BOX: Rect = Rect { x: 90, y: 90, w: 60, h: 60 };

    fn glyphs(x: u32, y: u32) -> bool {
        BOX.contains(x as i64, y as i64) && (x % 7 < 4) && (y % 9 < 6)
    }

    /// Diagonal hatching at pitch 7 - the tone that is *not* a balloon.
    fn hatched(x: u32, y: u32) -> bool {
        (x + y) % 7 < 2
    }

    /// A 45° halftone lattice, two families of dots at pitch 8.
    fn halftone(x: u32, y: u32) -> bool {
        let near = |a: u32, b: u32| {
            let (dx, dy) = ((a % 8) as i32 - 2, (b % 8) as i32 - 2);
            dx * dx + dy * dy <= 4
        };
        near(x, y) || near(x + 4, y + 4)
    }

    fn depth_of(interior: Interior) -> u32 {
        match interior {
            Interior::Solid { depth, .. } => depth,
            other => panic!("not solid: {other:?}"),
        }
    }

    #[test]
    fn a_uniform_fill_outside_the_text_is_a_balloon_interior() {
        // Cream paper at 246, not white: the fill is read off the page and
        // never snapped, the same refusal §4 states for a ring median.
        let page = gray_page(240, 240, |x, y| if glyphs(x, y) { 20 } else { 246 });
        let seg = segmentation(240, 240, glyphs);
        let read = interior_of(&page, &seg, BOX);
        assert_eq!(read, Interior::Solid { level: 246 * 257, depth: 6 });
        assert!(read.settles(Detected::Outside), "a detector miss is what this exists to overrule");
    }

    /// The other half of the objection to reading balloons off the page:
    /// *white text over black*. A fill is a fill.
    #[test]
    fn a_black_balloon_under_white_lettering_reads_the_same_as_a_white_one() {
        let page = gray_page(240, 240, |x, y| {
            if glyphs(x, y) {
                250
            } else if BOX.grown(30, 240, 240).contains(x as i64, y as i64) {
                18
            } else {
                200
            }
        });
        let seg = segmentation(240, 240, glyphs);
        assert_eq!(interior_of(&page, &seg, BOX), Interior::Solid { level: 18 * 257, depth: 6 });
    }

    #[test]
    fn hatching_around_the_text_is_not_a_balloon_interior() {
        let page = gray_page(240, 240, |x, y| {
            if glyphs(x, y) {
                20
            } else if hatched(x, y) {
                40
            } else {
                250
            }
        });
        let seg = segmentation(240, 240, glyphs);
        let read = interior_of(&page, &seg, BOX);
        assert_eq!(read, Interior::Textured);
        assert!(!read.settles(Detected::Bubble), "picture outside the text overrules a bubble box");
        assert!(
            read.settles(Detected::TextInBubble),
            "but not the model's own confident answer to the balloon question"
        );
    }

    /// The scan this module was measured against after it shipped: white paper
    /// with a spread of eight levels, one-sided because white saturates. Under
    /// a fixed six-level tolerance a fifth of every ring is off the fill in
    /// dozens of runs, which reads as picture; the band's own spread is what
    /// the tolerance has to follow.
    #[test]
    fn a_noisy_white_fill_is_still_a_balloon_interior() {
        // Deterministic speckle: a hash of the position, one-sided below 255,
        // with a long tail past six levels and nothing past twenty.
        let noise = |x: u32, y: u32| -> u8 {
            let h = x.wrapping_mul(2654435761).wrapping_add(y.wrapping_mul(40503)) ^ (x * y);
            let r = (h >> 7) % 100;
            if r < 60 {
                0
            } else if r < 80 {
                (r - 55) as u8 // 5..=24 levels, mostly under twelve
            } else {
                ((r - 80) % 12 + 8) as u8 // 8..=19
            }
        };
        let page = gray_page(240, 240, |x, y| if glyphs(x, y) { 20 } else { 255 - noise(x, y) });
        let seg = segmentation(240, 240, glyphs);
        let read = interior_of(&page, &seg, BOX);
        assert!(matches!(read, Interior::Solid { .. }), "noisy paper read as {read:?}");
        assert!(read.settles(Detected::Outside), "a caption box the detector called text_free");
    }

    /// The cap: a light grey tone whose dots sit thirty levels under the paper
    /// is not a fill however evenly spread its own deviations are.
    #[test]
    fn a_light_tone_is_not_widened_into_a_fill() {
        let page = gray_page(240, 240, |x, y| {
            if glyphs(x, y) {
                20
            } else if halftone(x, y) {
                220
            } else {
                250
            }
        });
        let seg = segmentation(240, 240, glyphs);
        assert_eq!(interior_of(&page, &seg, BOX), Interior::Textured);
    }

    #[test]
    fn the_detector_grade_follows_the_class_and_the_score() {
        let sure = balloon(0, 0, 200, 200, BalloonClass::TextInBubble);
        let weak = BalloonBox { rect: Rect::new(0, 0, 200, 200), class: BalloonClass::TextInBubble, score: 0.4 };
        // A shape near the text rather than around it: it holds the region's
        // centre and not its whole extent, which is what keeps it a `Bubble`.
        let shape = balloon(60, 60, 200, 200, BalloonClass::Bubble);
        let free = balloon(0, 0, 200, 200, BalloonClass::TextFree);
        let region = Rect::new(50, 50, 40, 40);
        assert_eq!(detected(region, std::slice::from_ref(&sure)), Detected::TextInBubble);
        assert_eq!(detected(region, &[sure.clone(), shape.clone()]), Detected::TextInBubble);
        assert_eq!(detected(region, &[weak]), Detected::Bubble);
        assert_eq!(detected(region, &[shape]), Detected::Bubble);
        assert_eq!(detected(region, &[sure, free]), Detected::Outside);
        assert_eq!(detected(region, &[]), Detected::Outside);
    }

    #[test]
    fn a_halftone_lattice_is_not_a_balloon_interior() {
        let page = gray_page(240, 240, |x, y| {
            if glyphs(x, y) {
                20
            } else if halftone(x, y) {
                60
            } else {
                250
            }
        });
        let seg = segmentation(240, 240, glyphs);
        assert_eq!(interior_of(&page, &seg, BOX), Interior::Textured);
    }

    /// The case the ring walk exists to get right, and the one a plain variance
    /// test gets wrong: four pixels of fill, then the balloon's own outline,
    /// then art that is none of this region's business. The walk stops **at**
    /// the outline, so what is beyond it is never read.
    #[test]
    fn a_thin_outline_ends_the_band_and_is_itself_the_evidence() {
        let page = gray_page(240, 240, |x, y| {
            let (x, y) = (x as i64, y as i64);
            let outline = BOX.grown(7, 240, 240);
            let inside_outline = BOX.grown(4, 240, 240);
            if glyphs(x as u32, y as u32) {
                20
            } else if !outline.contains(x, y) {
                // Art beyond the balloon, and plenty of it.
                if (x * 3 + y * 5) % 11 < 5 { 30 } else { 240 }
            } else if !inside_outline.contains(x, y) {
                25 // the outline: three pixels of ink
            } else {
                246
            }
        });
        let seg = segmentation(240, 240, glyphs);
        let read = interior_of(&page, &seg, BOX);
        let depth = depth_of(read);
        assert!(
            (MIN_FILL_DEPTH..required_depth(BOX)).contains(&depth),
            "the band was {depth} deep: shallower than this box asks for, which is the case \
             the outline has to carry"
        );
        assert!(read.settles(Detected::Outside));
    }

    /// The control for the test above. Same geometry, no outline: art starts
    /// where the fill stops. Four pixels of fill is not six, and there is no
    /// outline behind it to say the fill was a balloon's - so the page declines
    /// to answer rather than inventing one, and the detector keeps its verdict.
    #[test]
    fn a_band_that_runs_into_art_with_no_outline_leaves_the_detector_alone() {
        let page = gray_page(240, 240, |x, y| {
            let (ix, iy) = (x as i64, y as i64);
            if glyphs(x, y) {
                20
            } else if !BOX.grown(4, 240, 240).contains(ix, iy) {
                if (ix * 3 + iy * 5) % 11 < 5 { 30 } else { 240 }
            } else {
                246
            }
        });
        let seg = segmentation(240, 240, glyphs);
        let read = interior_of(&page, &seg, BOX);
        assert_eq!(read, Interior::Unreadable);
        assert!(read.settles(Detected::Bubble), "an unreadable band changes nothing");
        assert!(!read.settles(Detected::Outside));
    }

    /// A glyph that spills past the masking box is a glyph, not a wall. Without
    /// the text mask the first ring reads as ink and the walk stops on it.
    #[test]
    fn the_text_s_own_strokes_are_not_read_as_the_band() {
        let spill = |x: u32, y: u32| glyphs(x, y) || ((x == 89 || x == 150) && (100..140).contains(&y));
        let page = gray_page(240, 240, |x, y| if spill(x, y) { 20 } else { 246 });
        let seg = segmentation(240, 240, spill);

        let blind = interior(&page, &Mask::empty(BOX.grown(30, 240, 240)), BOX);
        assert_ne!(blind, Interior::Solid { level: 246 * 257, depth: 6 }, "the spill was read as ink");

        let seeing = interior_of(&page, &seg, BOX);
        assert_eq!(seeing, Interior::Solid { level: 246 * 257, depth: 6 });
    }

    /* ---- the reading inside the box ---- */

    /// The shape this exists for: a rectangular narration box whose frame sits
    /// tight against the lettering, over art. The ring walk meets the art at
    /// offset 1 - the frame is inside the box, not outside it, because the box
    /// is the detector's and the detector drew it around the whole plate - so
    /// the walk says `Textured` about white paper with black text on it. The
    /// inside says otherwise: the frame is one-sided ink and a tenth of the
    /// box, and everything else between the strokes is one fill.
    #[test]
    fn a_narration_box_with_no_room_around_it_is_read_from_the_inside() {
        // Sparser than `glyphs`, because that pattern is a lattice and this is
        // meant to be lettering: after `TEXT_HALO` it leaves about the 50–62%
        // of paper the real narration boxes leave.
        let plate = Rect::new(BOX.x + 1, BOX.y + 1, BOX.w - 2, BOX.h - 2);
        let letters = |x: u32, y: u32| {
            plate.contains(x as i64, y as i64) && (x % 12 < 3) && (y % 14 < 5)
        };
        let page = gray_page(240, 240, |x, y| {
            let (ix, iy) = (x as i64, y as i64);
            if letters(x, y) {
                20
            } else if plate.contains(ix, iy) {
                246
            } else if BOX.contains(ix, iy) {
                20 // the frame, one pixel wide, inside the box
            } else if hatched(x, y) {
                40 // and art everywhere outside it
            } else {
                250
            }
        });
        let seg = segmentation(240, 240, letters);
        let read = interior_of(&page, &seg, BOX);
        assert_eq!(read, Interior::Solid { level: 246 * 257, depth: 0 }, "read inside, not walked");
        assert!(read.settles(Detected::Outside), "which is what puts the narration back in the run");
    }

    /// And the walk keeps the first word wherever it has one. A balloon with
    /// room around it answers with the depth it actually walked, never `0`.
    #[test]
    fn a_walk_that_finds_a_band_is_not_second_guessed_by_the_inside() {
        let page = gray_page(240, 240, |x, y| if glyphs(x, y) { 20 } else { 246 });
        let seg = segmentation(240, 240, glyphs);
        assert_eq!(interior_of(&page, &seg, BOX), Interior::Solid { level: 246 * 257, depth: 6 });
    }

    /// Lettering sparse enough to leave the paper this reading needs. `glyphs`
    /// is a lattice covering well over half the box, which after `TEXT_HALO`
    /// leaves under a tenth of it - less than `MIN_INNER_PAPER_PERCENT`, so a
    /// fixture built on it never reaches the branches below. This leaves about
    /// the 50–62% the real narration boxes leave.
    fn letters(x: u32, y: u32) -> bool {
        BOX.contains(x as i64, y as i64) && (x % 12 < 3) && (y % 14 < 5)
    }

    /// A page whose box is lettering on `fill`, with `inside` painting whatever
    /// else is between the strokes, and art everywhere outside the box so that
    /// the ring walk answers `Textured` and the inside is what decides.
    fn plate(fill: u8, inside: impl Fn(u32, u32) -> Option<u8>) -> Raster {
        gray_page(240, 240, |x, y| {
            if letters(x, y) {
                20
            } else if BOX.contains(x as i64, y as i64) {
                inside(x, y).unwrap_or(fill)
            } else if hatched(x, y) {
                40
            } else {
                250
            }
        })
    }

    /// [`inner_read`] on a plate lettered with [`letters`], as the counts the
    /// two tests below reason about: `(on_page, samples, off, darker, lighter)`.
    fn inner_counts(page: &Raster, bbox: Rect) -> (usize, usize, usize, usize, usize) {
        let seg = segmentation(240, 240, letters);
        let reach = scan_depth(bbox) + TEXT_HALO + 1;
        let area = bbox.grown(reach, page.width, page.height);
        let (_, read) = inner_read(page, &text_halo(&seg, area), bbox)
            .expect("the plate leaves enough paper between the strokes to be read");
        (read.on_page, read.samples, read.off, read.darker, read.lighter)
    }

    /// The control the two tests below are read against: the same plate with
    /// nothing between the strokes but paper. The walk cannot answer it - there
    /// is art at offset 1 - so a `Solid` here is the inside's, and every
    /// difference from it is caused by what the test adds inside the box.
    #[test]
    fn a_plate_of_lettering_on_paper_is_read_from_the_inside() {
        let read = interior_of(&plate(250, |_, _| None), &segmentation(240, 240, letters), BOX);
        assert_eq!(read, Interior::Solid { level: 250 * 257, depth: 0 });
    }

    /// Screentone between the strokes is not paper, and it is the case
    /// one-sidedness alone would let through: a halftone's dots are all darker
    /// than the paper they sit on, exactly as ink is. What refuses it is
    /// [`INNER_OFF_PERCENT`] - the dots cover far more of the box than a frame
    /// and a few antialiased edges do.
    ///
    /// The tone is confined to the **inside** of the box, and the control above
    /// is the same page without it, so the share is demonstrably what decides:
    /// nothing else about the two pages differs.
    #[test]
    fn screentone_between_the_strokes_is_not_paper() {
        let page = plate(250, |x, y| halftone(x, y).then_some(60));
        let read = interior_of(&page, &segmentation(240, 240, letters), BOX);
        assert!(!matches!(read, Interior::Solid { .. }), "tone between the strokes read as {read:?}");
        // And it is the share that refused it, not the sidedness: every dot is
        // darker than the paper, so `is_one_sided` would have passed it.
        let (_, samples, off, darker, _) = inner_counts(&page, BOX);
        assert!(darker == off, "the fixture's tone is not one-sided");
        assert!(
            off * 100 > samples * INNER_OFF_PERCENT,
            "the tone covers {}% of the samples, which INNER_OFF_PERCENT would have allowed",
            off * 100 / samples
        );
    }

    /// Art between the strokes is refused one step earlier, by one-sidedness: a
    /// picture strays both lighter and darker than its own median, and ink only
    /// ever strays one way.
    ///
    /// The pair is what isolates that. Both pages put the **same** speckle
    /// pattern over the same share of the box; the first paints half of it
    /// lighter than the paper and the second paints all of it darker. The share
    /// is under [`INNER_OFF_PERCENT`] on both, so sidedness is the only thing
    /// that can be deciding.
    #[test]
    fn art_between_the_strokes_is_refused_by_its_two_sidedness() {
        let dark = |x: u32, y: u32| (x as i64 * 3 + y as i64 * 5) % 37 < 2;
        let light = |x: u32, y: u32| (x as i64 * 7 + y as i64 * 2) % 41 < 2;
        let seg = segmentation(240, 240, letters);

        let two_sided = plate(200, |x, y| {
            if dark(x, y) {
                Some(40)
            } else {
                light(x, y).then_some(255)
            }
        });
        let read = interior_of(&two_sided, &seg, BOX);
        assert!(!matches!(read, Interior::Solid { .. }), "art between the strokes read as {read:?}");

        // The same speckle, all of it on one side of the paper: ink, and the
        // reading admits it.
        let one_sided = plate(200, |x, y| (dark(x, y) || light(x, y)).then_some(40));
        assert_eq!(interior_of(&one_sided, &seg, BOX), Interior::Solid { level: 200 * 257, depth: 0 });

        // And both are inside the share, so sidedness is what told them apart.
        for (name, page) in [("two-sided", &two_sided), ("one-sided", &one_sided)] {
            let (_, samples, off, _, _) = inner_counts(page, BOX);
            assert!(
                off * 100 <= samples * INNER_OFF_PERCENT,
                "{name}: {}% off the fill is over INNER_OFF_PERCENT, so the share decided",
                off * 100 / samples
            );
        }
    }

    /// Too little paper left between the strokes to be a sample of anything.
    /// The box is nearly all lettering, so what survives the halo is the gaps
    /// inside the glyphs, and a reading from those is the glyphs' own
    /// antialiasing. The walk's answer stands.
    #[test]
    fn a_box_that_is_almost_all_lettering_is_not_read_from_the_inside() {
        // 96% of the box is text, well under `MIN_INNER_PAPER_PERCENT` of paper.
        let dense =
            |x: u32, y: u32| BOX.contains(x as i64, y as i64) && !(x.is_multiple_of(5) && y.is_multiple_of(5));
        let page = gray_page(240, 240, |x, y| {
            if dense(x, y) {
                20
            } else if BOX.contains(x as i64, y as i64) {
                246
            } else if hatched(x, y) {
                40
            } else {
                250
            }
        });
        let seg = segmentation(240, 240, dense);
        assert_eq!(interior_of(&page, &seg, BOX), Interior::Textured);
    }

    /// And too few pixels outright. A box smaller than
    /// [`MIN_RING_SAMPLES`] worth of paper has no reading either way.
    #[test]
    fn a_box_with_almost_no_pixels_in_it_is_not_read_from_the_inside() {
        let tiny = Rect::new(100, 100, 5, 5);
        let inked = |x: u32, y: u32| tiny.contains(x as i64, y as i64) && x.is_multiple_of(2);
        let page = gray_page(240, 240, |x, y| {
            if inked(x, y) {
                20
            } else if tiny.contains(x as i64, y as i64) {
                246
            } else if hatched(x, y) {
                40
            } else {
                250
            }
        });
        let seg = segmentation(240, 240, inked);
        // 25 pixels, of which the halo leaves nothing like 16 of paper.
        assert!(!matches!(interior_of(&page, &seg, tiny), Interior::Solid { .. }));
    }

    #[test]
    fn a_box_with_no_readable_paper_around_it_is_not_a_verdict() {
        let page = gray_page(8, 8, |_, _| 200);
        let seg = segmentation(8, 8, |_, _| false);
        let read = interior_of(&page, &seg, Rect::new(0, 0, 4, 4));
        assert_eq!(read, Interior::Unreadable);
        assert!(read.settles(Detected::Bubble));
        assert!(!read.settles(Detected::Outside));
        // And a box with no area at all is not a question.
        assert_eq!(interior_of(&page, &seg, Rect::new(0, 0, 0, 0)), Interior::Unreadable);
    }

    /// The depth asked for scales with the lettering, and is clamped at both
    /// ends: a small box must not need a balloon's whole width of fill, and a
    /// large one must not be able to ask for more than any balloon has.
    #[test]
    fn the_band_asked_for_scales_with_the_text_and_stops_at_both_ends() {
        assert_eq!(required_depth(Rect::new(0, 0, 12, 12)), MIN_FILL_DEPTH);
        assert_eq!(required_depth(Rect::new(0, 0, 60, 200)), 6);
        assert_eq!(required_depth(Rect::new(0, 0, 400, 400)), MAX_FILL_DEPTH);
        // Every walk reaches past what it asks for, so an interrupted band has
        // room to have been deep enough first.
        for side in [8u32, 60, 400] {
            let bbox = Rect::new(0, 0, side, side);
            assert!(scan_depth(bbox) > required_depth(bbox));
        }
    }

    /// Two columns of one balloon have that balloon's fill between them.
    #[test]
    fn two_columns_of_one_balloon_may_be_merged() {
        let cols = |x: u32, y: u32| {
            (100..140).contains(&x) && (100..300).contains(&y)
                || (180..220).contains(&x) && (100..300).contains(&y)
        };
        let page = gray_page(400, 400, |x, y| if cols(x, y) { 20 } else { 250 });
        let seg = segmentation(400, 400, cols);
        assert!(!merge_crosses_a_balloon(
            &page,
            &seg,
            Rect::new(95, 95, 50, 210),
            Rect::new(175, 95, 50, 210)
        ));
    }

    /// Anti-aliased text overhang bordering a strip is covered by the text halo.
    #[test]
    fn text_halo_scans_outside_strip_to_cover_text_overhang() {
        let cols = |x: u32, y: u32| {
            (50..100).contains(&x) && (100..300).contains(&y)
                || (104..154).contains(&x) && (100..300).contains(&y)
        };
        let page = gray_page(400, 400, |x, y| {
            if cols(x, y) {
                20
            } else if (x == 100 || x == 101) && (100..300).contains(&y) {
                100
            } else {
                250
            }
        });
        let seg = segmentation(400, 400, cols);
        assert!(!merge_crosses_a_balloon(
            &page,
            &seg,
            Rect::new(50, 100, 50, 200),
            Rect::new(104, 100, 50, 200)
        ));
    }


    /// Two balloons have a rim, art, and a second rim between them. This is the
    /// merge that produces one region spanning two bubbles.
    #[test]
    fn a_merge_across_two_balloons_is_refused() {
        let cols = |x: u32, y: u32| {
            (100..140).contains(&x) && (100..300).contains(&y)
                || (260..300).contains(&x) && (100..300).contains(&y)
        };
        let tone = |x: u32, y: u32| (x + y) % 7 < 3;
        let page = gray_page(400, 400, |x, y| {
            if cols(x, y) {
                20
            } else if !(160..240).contains(&x) {
                250 // the two balloons' interiors
            } else if !(166..234).contains(&x) {
                25 // their rims
            } else if tone(x, y) {
                60
            } else {
                246
            }
        });
        let seg = segmentation(400, 400, cols);
        assert!(merge_crosses_a_balloon(
            &page,
            &seg,
            Rect::new(95, 95, 50, 210),
            Rect::new(255, 95, 50, 210)
        ));
    }

    /// Boxes that already meet have no gap, and boxes sharing no extent at all
    /// are joined by a diagonal rather than by a strip of paper.
    #[test]
    fn overlapping_boxes_merge_and_diagonal_ones_do_not() {
        let page = gray_page(400, 400, |_, _| 250);
        let seg = segmentation(400, 400, |_, _| false);
        assert!(!merge_crosses_a_balloon(
            &page,
            &seg,
            Rect::new(100, 100, 80, 80),
            Rect::new(150, 120, 80, 80)
        ));
        assert!(merge_crosses_a_balloon(
            &page,
            &seg,
            Rect::new(100, 100, 60, 60),
            Rect::new(220, 220, 60, 60)
        ));
    }

    /// One ring is four segments, each walked in order, which is the whole
    /// reason an outline and a halftone can be told apart: both put off-tone
    /// pixels on the ring, and only one of them puts them next to each other.
    #[test]
    fn a_ring_is_four_segments_walked_in_order_and_never_reaches_a_corner() {
        let sides = ring_sides(Rect::new(10, 10, 4, 4), 1);
        let flat: Vec<(i64, i64)> = sides.iter().flatten().copied().collect();
        assert_eq!(flat.len(), 4 * 2, "four segments, each the middle half of its side");
        assert_eq!(sides[0], vec![(11, 9), (12, 9)], "the paper above");
        assert_eq!(sides[3], vec![(14, 11), (14, 12)], "the paper to the right");
        // Nothing within a quarter-side of a corner is ever read, and no corner
        // of the enclosing 6×6 square is.
        for near_corner in [(9, 9), (14, 9), (9, 14), (14, 14), (10, 9), (13, 9), (14, 10)] {
            assert!(!flat.contains(&near_corner), "a corner was walked: {near_corner:?}");
        }
        let mut sorted = flat.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), flat.len(), "a pixel was walked twice");
    }

    /// The case the corner-free walk exists for, and the one a closed
    /// rectangular ring gets wrong: an elliptical balloon with lettering that
    /// nearly fills it, over screentone. The box's corners are inside the rim
    /// and its sides have room; a ring that turns the corner leaves the balloon
    /// at once, reads the tone beyond it, and calls picture.
    #[test]
    fn an_elliptical_balloon_full_of_lettering_is_still_a_balloon_interior() {
        let (cx, cy, rx, ry) = (400.0f64, 400.0f64, 180.0f64, 140.0f64);
        // Corners at 0.96 of the way to the rim: a letterer's own margin.
        let text = Rect::new(400 - 122, 400 - 95, 244, 190);
        let t = |x: u32, y: u32| {
            let (dx, dy) = ((x as f64 - cx) / rx, (y as f64 - cy) / ry);
            (dx * dx + dy * dy).sqrt()
        };
        // A 45° screentone lattice outside the balloon, at pitch 9.
        let tone = |x: u32, y: u32| {
            let near = |a: u32, b: u32| {
                let (dx, dy) = ((a % 9) as i32 - 3, (b % 9) as i32 - 3);
                dx * dx + dy * dy <= 5
            };
            near(x, y) || near(x + 4, y + 4)
        };
        let letters = |x: u32, y: u32| {
            text.contains(x as i64, y as i64) && (x % 11 < 7) && (y % 13 < 9)
        };
        let page = gray_page(800, 800, |x, y| {
            if letters(x, y) {
                20
            } else if t(x, y) <= 1.0 {
                250
            } else if t(x, y) <= 1.0 + 3.0 / rx {
                25 // the rim
            } else if tone(x, y) {
                60
            } else {
                246
            }
        });
        let seg = segmentation(800, 800, letters);
        // The box the gate actually asks about: `detect::boxes` grows the
        // detector's box by +2 all sides and +1 more on the right, then +5.
        let masking = Rect::new(text.x - 7, text.y - 7, text.w + 15, text.h + 14);
        let read = interior_of(&page, &seg, masking);
        assert!(
            matches!(read, Interior::Solid { .. }),
            "an ordinary balloon read as {read:?}, which sends its dialogue to review",
        );
        assert!(read.settles(Detected::Bubble));
    }
}

