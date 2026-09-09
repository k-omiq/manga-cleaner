//! The script gate: clean Japanese, leave English alone.
//!
//! The asymmetry is the whole design:
//!
//! > Low confidence → drop the box and flag it, because leaving Japanese is
//! > recoverable and painting over English is not.
//!
//! So the gate's default is **do not clean**, and a region reaches the pipeline
//! only on a confident CJK verdict. Everything the gate refuses appears in
//! review with a *clean anyway* action, because a page that reports "cleaned"
//! with text still on it and no way to find it is worse than one that reports
//! what it skipped.

use std::path::Path;

use crate::balloon::Detected;
use crate::detect::{Region, Segmentation};
use crate::image::Raster;

pub mod lines;
pub mod ocr;
mod osd;

pub use lines::{Orientation, TextLine};
pub use ocr::{Ocr, OcrError, Reading, cjk_share};
pub use osd::{LineScript, Osd, OsdError};

/// What a run does with text the balloon question puts outside a balloon.
///
/// The rule is *"Gate does not decide. Region is cleaned only if the user opts
/// in, and always enters review."* The default is the first half and
/// [`OutsideText::Clean`] is the opt-in - a property of the run, chosen in the
/// tool window beside the engine that kind of text starts on, and off unless
/// somebody turned it on. Sound effects
/// and lettering over art are still what §3 conceded rather than classified,
/// so the opt-in is not a script verdict: nothing is read, everything the
/// balloon question left outside goes to the ladder, and the person who
/// turned it on is the one who decided that was right for this chapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutsideText {
    /// Hold it back for review under *"text outside a speech bubble"*.
    #[default]
    Review,
    /// Clean it, on the run's own say-so.
    Clean,
}

impl OutsideText {
    /// The seam's name for it: `outsideBubbles` on `runClean`, `"review"` or
    /// `"clean"`. Anything else - absent, or a value this build does not know -
    /// is the default, because the default is the direction that leaves text
    /// rather than the one that paints over it.
    pub fn from_arg(name: Option<&str>) -> OutsideText {
        match name {
            Some("clean") => OutsideText::Clean,
            _ => OutsideText::Review,
        }
    }
}

/// What the gate decided, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Confidently CJK. Clean it.
    Clean { script: String },
    /// Confidently something else - Latin, Cyrillic, Hangul. Leave it, and say
    /// so: this is the case that must never be silent.
    NotJapanese { script: String },
    /// The model said too little to act on. Held back and flagged, the same as
    /// a refusal, because acting on a guess here is the unrecoverable
    /// direction.
    Uncertain,
    /// Out of a balloon, where §3 says the gate does not decide at all.
    OutOfBalloon,
    /// Out of a balloon, and the run opted in. §3's table has always read
    /// *"cleaned only if the user opts in"* for this row; [`OutsideText::Clean`]
    /// is that opt-in. No script was read - the gate does not decide out of a
    /// balloon whichever way the run leans - so the verdict carries none.
    OptedIn,
}

impl Verdict {
    pub fn cleans(&self) -> bool {
        matches!(self, Verdict::Clean { .. } | Verdict::OptedIn)
    }

    /// The i18n key review lists this under. `review.reason.*` in
    /// `src/lib/i18n/en.js`, which is the fixed vocabulary - the first two names
    /// here were invented alongside catalogue entries that already said the same
    /// thing, and the catalogue's names win.
    pub fn reason_key(&self) -> Option<&'static str> {
        match self {
            Verdict::Clean { .. } | Verdict::OptedIn => None,
            Verdict::NotJapanese { .. } => Some("review.reason.gateSkippedNotJapanese"),
            Verdict::Uncertain => Some("review.reason.gateSkippedLowConfidence"),
            Verdict::OutOfBalloon => Some("review.reason.gateSkippedOutsideBubble"),
        }
    }

    /// The `gateSkipCause` the seam carries, which the interface turns back into
    /// [`Verdict::reason_key`]'s key. `None` for a verdict that cleans.
    ///
    /// Three causes, not two. `src/lib/model/types.js` had `'low-confidence'`
    /// and `'outside-bubble'`, and folding `NotJapanese` into the first of them
    /// would state the opposite of what happened: the gate was *confident*, and
    /// being confident is why it left the region alone.
    pub fn skip_cause(&self) -> Option<&'static str> {
        match self {
            Verdict::Clean { .. } | Verdict::OptedIn => None,
            Verdict::NotJapanese { .. } => Some("not-japanese"),
            Verdict::Uncertain => Some("low-confidence"),
            Verdict::OutOfBalloon => Some("outside-bubble"),
        }
    }
}

/// The labels this gate treats as "clean it".
///
/// **Wider than "Japanese", deliberately.** The model's own confusion mass is
/// Chinese↔Japanese, citing SIW-13, and Phase 0 spike 8 measured a hiragana
/// column coming back `HanS_vert`. Separating Han from kana is a problem this model cannot solve
/// and the product does not need solved: the question the gate is asked is
/// *"is this the Latin typesetting a localiser added?"*, and against that
/// question every CJK label is the same answer.
///
/// Hangul is **not** here. It is a different language with its own product,
/// reached instead through per-script selection.
const CJK_LABELS: [&str; 6] =
    ["Japanese", "Japanese_vert", "HanS", "HanS_vert", "HanT", "HanT_vert"];

fn is_cjk(label: &str) -> bool {
    let base = label.strip_suffix("-dn").unwrap_or(label);
    CJK_LABELS.contains(&base)
}

/// A label that means "I read something, but not a script": `Common` is
/// punctuation and digits, `Joined` and `NULL` are structural. A line that
/// returns one of these has not identified a script and must not be counted as
/// a vote against one.
fn is_abstention(label: &str) -> bool {
    let base = label.strip_suffix("-dn").unwrap_or(label);
    matches!(base, "Common" | "Joined" | "NULL" | "Broken")
}

/// How much evidence a line needs before its verdict counts. One collapsed
/// timestep is a single glyph's worth of opinion; the short columns that
/// came back `Common` and `HanS_vert` in Phase 0 spike 8 were all at this
/// level.
const MIN_LINE_STRENGTH: usize = 2;

/// The scripts a `NotJapanese` verdict may name and be **believed without a
/// second opinion**.
///
/// The script identifier is a 3.7 MB LSTM over a 48-pixel strip, and its
/// failures are not evenly spread. On the alphabets it was trained heavily on -
/// the Latin a localiser types, the Cyrillic and Greek and Hangul and Hebrew
/// and Arabic and Thai that a page might genuinely be in, and Tesseract's own
/// `Fraktur` - a confident answer is an answer. On the long tail it is not: a
/// tall thin column of kana comes back `Tibetan` or `Syriac`, confidently,
/// because those scripts are *also* tall and thin and the model has barely
/// seen them. Measured on the user's own scans: 15.png at 512,1011 read
/// `Tibetan`, 04.png at 786,923 read `Syriac`, and both are ordinary dialogue.
///
/// So the list is an allow-list rather than a deny-list, and it is short on
/// purpose. A script that is not on it is not a refusal - it is a verdict this
/// model is not entitled to reach alone, and [`ScriptGate::judge`] puts it to
/// the reader instead. The `-dn` suffix is the model's own "dotted normalised"
/// variant of a label and means the same script.
const TRUSTED_SCRIPTS: [&str; 9] = [
    "Latin",
    "Cyrillic",
    "Greek",
    "Hangul",
    "Hangul_vert",
    "Fraktur",
    "Arabic",
    "Hebrew",
    "Thai",
];

fn is_trusted(label: &str) -> bool {
    let base = label.strip_suffix("-dn").unwrap_or(label);
    TRUSTED_SCRIPTS.contains(&base)
}

/// How much of a reading has to be written in a Japanese block before it
/// overturns the identifier ([`ocr::cjk_share`]).
///
/// **0.6, and it is a floor rather than a threshold anyone tuned.** The reader
/// invents nothing outside its vocabulary, which is Japanese, so a genuine
/// Japanese balloon comes back at or very near 1.0 and there is a wide empty
/// band underneath. What the share is really guarding against is the reader
/// hallucinating a few characters over Latin lettering or over screentone: that
/// produces short, mixed, low-share readings, and 0.6 sits well above them
/// without sitting so high that one stray digit in a balloon refuses it.
/// Nobody has swept it.
const MIN_CJK_SHARE: f32 = 0.6;

/// How many **Japanese** characters a reading needs before it is evidence at
/// all. A single character is what the reader emits when it has found nothing
/// and is guessing at the paper - and a single kana would pass the share test
/// at 1.0. Counted over the Japanese blocks alone, because a kana and a full
/// stop is still one character of evidence.
const MIN_RESCUE_CHARS: usize = 2;

/// The `script` a rescued verdict carries.
///
/// **Not `"Japanese"`.** A patch's provenance and the probe both read this
/// string, and a rescue is a different claim from an identification: it says a
/// reader whose whole vocabulary is Japanese produced Japanese text here, not
/// that a script identifier recognised the script. Anyone auditing a page that
/// was cleaned when it should not have been needs to be able to tell the two
/// apart without re-running anything.
pub const RESCUED_SCRIPT: &str = "Japanese_ocr";

pub struct ScriptGate {
    osd: Osd,
    /// The rescue reader, when the run has one. `None` is the shipped default
    /// and the behaviour that predates it: the weights are optional, and a run
    /// without them must reach exactly the verdicts it reached before this
    /// existed.
    ocr: Option<Ocr>,
}

impl ScriptGate {
    pub fn open(
        model: &Path,
        labels_json: &Path,
        preference: crate::accel::Preference,
    ) -> Result<ScriptGate, OsdError> {
        Ok(ScriptGate { osd: Osd::open(model, labels_json, preference)?, ocr: None })
    }

    /// Attach the rescue reader.
    ///
    /// A separate step rather than an argument to [`ScriptGate::open`] for the
    /// same reason [`crate::detect::Pipeline::with_picks`] is one: every caller
    /// that does not ask for it - the spikes, the tests, a machine where the
    /// weights were never downloaded - goes on gating exactly as before.
    pub fn with_ocr(mut self, ocr: Ocr) -> ScriptGate {
        self.ocr = Some(ocr);
        self
    }

    /// Whether a reader is attached. What `open_gate` in the adapter is asked
    /// after opening one beside a reader that would not open.
    pub fn has_reader(&self) -> bool {
        self.ocr.is_some()
    }

    /// Where the session landed and why.
    ///
    /// The same accessor [`crate::detect::Detector`] and
    /// [`crate::balloon::BalloonDetector`] carry, and for the same reason: a
    /// caller reporting which provider each of a run's models chose should not
    /// have to reach past this type to the session inside it. The gate's answer
    /// is not written into a patch's provenance - the gate produces no pixels -
    /// but it is a model that opened, and diagnostics name all of them.
    pub fn selection(&self) -> &crate::accel::Selection {
        self.osd.selection()
    }

    /// Whether the owner should give this session back at its next safe point.
    /// [`crate::detect::Detector::spent`] states the trade.
    pub fn spent(&self) -> bool {
        // The reader lives inside this gate and is released with it, so a
        // spent reader lease is this gate's own signal to give the session back.
        self.osd.spent() || self.ocr.as_ref().is_some_and(|ocr| ocr.spent())
    }

    /// Give this gate's sessions up for good, because one of them has failed in
    /// a way another run cannot survive - a reset adapter reported as
    /// device-removed. [`crate::registry::Lease::poison`] states what that
    /// costs and why it is not the same signal as an unload the user asked for.
    ///
    /// Only the identifier is poisoned, and the rescue reader needs no poison of
    /// its own: [`ScriptGate::spent`] is an `or` over both, so a poisoned
    /// identifier is already this gate answering `true`, and the reader is
    /// opened inside the gate and released with it rather than parked
    /// separately. A device that took one of them down took both, and the gate
    /// is what the caller gives back.
    pub fn poison(&self) {
        self.osd.poison();
    }

    /// Decide one region.
    ///
    /// `detected` comes from outside: §3 scopes the gate by whether the text
    /// sits in a balloon, and `11-phase0-spikes.md` §6 found that the text
    /// detector does not answer that question - so whatever does answer it
    /// ([`crate::balloon::detected`]) hands the answer in here rather than this
    /// module guessing.
    ///
    /// **It is still not this module's guess when the page disagrees with it.**
    /// The balloon detector's misses are one-sided: a bubble it emitted no box
    /// for arrives here as *outside* and leaves as `OutOfBalloon`, which puts
    /// ordinary dialogue in review under *"text outside a speech bubble"*. So
    /// the handed-in answer is put to [`crate::balloon::interior`] - the same
    /// module, measuring the paper around the text rather than asking the model
    /// again - and it is that function, not this one, that decides what a
    /// uniform fill and a bubble stroke mean, and how far a confident detector
    /// may be overruled. Everything this method needs for it is already in its
    /// hands: the crop, the segmentation that says which pixels are strokes,
    /// and the region's own box.
    ///
    /// `outside` is the run's answer to §3's *"cleaned only if the user opts
    /// in"*. It changes nothing for a region the balloon question puts inside
    /// a balloon: those are gated on script exactly as before.
    pub fn judge(
        &mut self,
        page: &Raster,
        seg: &Segmentation,
        region: &Region,
        detected: Detected,
        outside: OutsideText,
    ) -> Result<Verdict, OsdError> {
        // The lettering's own box, not the masking box: see
        // [`Region::text_bounds`]. The five pixels of edit margin between them
        // are five pixels of a tight balloon's fill, and spending them here is
        // spending the evidence.
        let inside =
            crate::balloon::interior_of(page, seg, region.text_bounds()).settles(detected);
        if !inside {
            return Ok(match outside {
                OutsideText::Review => Verdict::OutOfBalloon,
                OutsideText::Clean => Verdict::OptedIn,
            });
        }

        let verdict = self.identify(page, seg, region)?;
        if !rescue_applies(&verdict, detected) {
            return Ok(verdict);
        }
        // A reader that fails is a reader that said nothing, and a rescue that
        // says nothing leaves the identifier's verdict standing - which is the
        // recoverable direction, and the direction this module defaults to
        // everywhere else. Nothing here can turn a missing or broken optional
        // model into a failed page.
        let Some(ocr) = self.ocr.as_mut() else { return Ok(verdict) };
        let reading = ocr.read(page, region.text_bounds()).ok();
        Ok(rescued(verdict, reading.as_ref()))
    }

    /// The script identifier's own verdict, line by line: everything `judge`
    /// did before the reader existed, unchanged.
    fn identify(
        &mut self,
        page: &Raster,
        seg: &Segmentation,
        region: &Region,
    ) -> Result<Verdict, OsdError> {
        let orientation = lines::orientation(seg, region.masking);
        let split = lines::split(seg, region.masking, orientation);
        if split.is_empty() {
            return Ok(Verdict::Uncertain);
        }

        let mut cjk = 0usize;
        let mut other: Vec<(String, usize)> = Vec::new();
        let mut best_cjk_label = String::new();

        for line in &split {
            let tight = lines::tighten(seg, line, orientation);
            let Some(script) = self.osd.identify(page, tight, orientation)? else {
                continue;
            };
            if script.strength < MIN_LINE_STRENGTH || is_abstention(&script.label) {
                continue;
            }
            if is_cjk(&script.label) {
                cjk += script.strength;
                if best_cjk_label.is_empty() {
                    best_cjk_label = script.label;
                }
            } else {
                match other.iter_mut().find(|(label, _)| *label == script.label) {
                    Some((_, n)) => *n += script.strength,
                    None => other.push((script.label, script.strength)),
                }
            }
        }

        let non_cjk: usize = other.iter().map(|(_, n)| n).sum();
        if cjk == 0 && non_cjk == 0 {
            return Ok(Verdict::Uncertain);
        }
        if cjk > non_cjk {
            return Ok(Verdict::Clean { script: best_cjk_label });
        }
        if non_cjk > cjk {
            // Ties break on the lower label so two runs agree.
            other.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let label = other.first().map(|(l, _)| l.clone()).unwrap_or_default();
            return Ok(Verdict::NotJapanese { script: label });
        }
        // Equal evidence both ways is not a verdict.
        Ok(Verdict::Uncertain)
    }
}

/// Whether the reader is allowed near this region at all.
///
/// Three conditions, and each one is a separate refusal to widen:
///
/// - **The balloon detector must be confident**, not the paper reading and not
///   a weak box. [`Detected::TextInBubble`] is the model saying *this text is
///   in a speech balloon* from either of the two directions
///   [`crate::balloon::Detected`] documents. A reader whose vocabulary is
///   Japanese, pointed at a sound effect or a caption over art, will read
///   something - so the answer to "may I read this" cannot come from the reader.
/// - **The identifier must have failed**, not disagreed. A confident `Latin`
///   is the case §3 exists for and is never second-guessed here.
/// - **A named script must be one the identifier is not good at.** See
///   [`TRUSTED_SCRIPTS`].
///
/// `Clean`, `OutOfBalloon` and `OptedIn` are not rescuable: the first needs no
/// help and the last two are not script verdicts.
fn rescue_applies(verdict: &Verdict, detected: Detected) -> bool {
    if detected != Detected::TextInBubble {
        return false;
    }
    match verdict {
        Verdict::Uncertain => true,
        Verdict::NotJapanese { script } => !is_trusted(script),
        Verdict::Clean { .. } | Verdict::OutOfBalloon | Verdict::OptedIn => false,
    }
}

/// What a reading does to a verdict the reader was allowed to look at.
///
/// Taken as an `Option<&Reading>` rather than read from a session, so the whole
/// decision - including "the reader was not there" and "the reader failed"  - 
/// is one pure function with tests that need no weights.
fn rescued(verdict: Verdict, reading: Option<&Reading>) -> Verdict {
    let Some(reading) = reading else { return verdict };
    // Counted over Japanese characters only: a reading of one kana and a
    // full stop is one character of evidence, not two.
    let long_enough =
        reading.text.chars().filter(|c| ocr::is_japanese_block(*c)).count() >= MIN_RESCUE_CHARS;
    if long_enough && ocr::cjk_share(&reading.text) >= MIN_CJK_SHARE {
        Verdict::Clean { script: RESCUED_SCRIPT.to_owned() }
    } else {
        verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cjk_label_the_model_can_emit_is_recognised() {
        for label in ["Japanese", "Japanese_vert", "HanS_vert", "HanT", "Japanese-dn"] {
            assert!(is_cjk(label), "{label}");
        }
        for label in ["Latin", "Cyrillic", "Hangul", "Hangul_vert", "Fraktur"] {
            assert!(!is_cjk(label), "{label}");
        }
    }

    #[test]
    fn an_abstention_is_not_a_vote_against_japanese() {
        // A punctuation-only column returns `Common`. A
        // punctuation-only box is classified Japanese on CJK punctuation alone,
        // so `Common` must not count as evidence of Latin.
        for label in ["Common", "Joined", "NULL", "Broken"] {
            assert!(is_abstention(label), "{label}");
        }
        assert!(!is_abstention("Latin"));
        assert!(!is_abstention("Japanese_vert"));
    }

    #[test]
    fn a_verdict_carries_the_key_review_lists_it_under() {
        assert_eq!(Verdict::Clean { script: "Japanese_vert".into() }.reason_key(), None);
        assert!(Verdict::Uncertain.reason_key().is_some());
        assert!(Verdict::NotJapanese { script: "Latin".into() }.reason_key().is_some());
        assert!(Verdict::OutOfBalloon.reason_key().is_some());
        assert!(!Verdict::Uncertain.cleans());
        assert!(Verdict::Clean { script: "Japanese".into() }.cleans());
        // The opt-in cleans and carries no cause: it is not a gate skip and
        // must not be counted as one.
        assert!(Verdict::OptedIn.cleans());
        assert_eq!(Verdict::OptedIn.reason_key(), None);
        assert_eq!(Verdict::OptedIn.skip_cause(), None);
    }

    #[test]
    fn the_opt_in_is_read_from_the_seam_and_defaults_to_review() {
        assert_eq!(OutsideText::from_arg(Some("clean")), OutsideText::Clean);
        assert_eq!(OutsideText::from_arg(Some("review")), OutsideText::Review);
        assert_eq!(OutsideText::from_arg(Some("yes")), OutsideText::Review);
        assert_eq!(OutsideText::from_arg(None), OutsideText::Review);
        assert_eq!(OutsideText::default(), OutsideText::Review);
    }

    /// Three causes, three keys, and no two verdicts sharing either. A fold of
    /// `NotJapanese` into `low-confidence` - the shape the interface's original
    /// two-value union forced - would report the opposite of what happened.
    #[test]
    fn every_held_back_verdict_has_its_own_cause_and_key() {
        let held = [
            Verdict::NotJapanese { script: "Latin".into() },
            Verdict::Uncertain,
            Verdict::OutOfBalloon,
        ];
        let causes: Vec<_> = held.iter().map(|v| v.skip_cause().unwrap()).collect();
        let keys: Vec<_> = held.iter().map(|v| v.reason_key().unwrap()).collect();
        for list in [&causes, &keys] {
            let mut sorted = list.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), 3, "two verdicts report as one: {list:?}");
        }
        assert_eq!(Verdict::Clean { script: "Japanese".into() }.skip_cause(), None);
    }

    fn reading(text: &str) -> Reading {
        Reading { text: text.to_owned(), score: -0.1 }
    }

    /// The allow-list is the whole of the rescue's scope on a `NotJapanese`,
    /// and the two labels that started this are both outside it.
    #[test]
    fn the_scripts_the_identifier_reads_well_are_believed_and_the_rest_are_not() {
        for label in ["Latin", "Cyrillic", "Greek", "Hangul_vert", "Fraktur", "Thai", "Latin-dn"] {
            assert!(is_trusted(label), "{label}");
        }
        // The two the user's pages actually produced over kana, and the CJK
        // labels, which never reach the rescue because they are `Clean`.
        for label in ["Tibetan", "Syriac", "Japanese", "HanS_vert", "Devanagari", ""] {
            assert!(!is_trusted(label), "{label}");
        }
    }

    /// The reader is only ever asked about a region the balloon detector put
    /// *confidently* in a bubble, and only about a verdict that failed.
    #[test]
    fn only_a_failed_verdict_inside_a_confident_balloon_is_offered_to_the_reader() {
        let uncertain = Verdict::Uncertain;
        let tibetan = Verdict::NotJapanese { script: "Tibetan".into() };
        let latin = Verdict::NotJapanese { script: "Latin".into() };

        assert!(rescue_applies(&uncertain, Detected::TextInBubble));
        assert!(rescue_applies(&tibetan, Detected::TextInBubble));
        // A script §3 is actually about is never second-guessed.
        assert!(!rescue_applies(&latin, Detected::TextInBubble));

        // A weak balloon box or none at all is not enough, however the gate
        // read the script. `Detected::Bubble` is a statement about a shape near
        // the text; the reader must not be pointed at a sound effect.
        for detected in [Detected::Bubble, Detected::Outside] {
            assert!(!rescue_applies(&uncertain, detected), "{detected:?}");
            assert!(!rescue_applies(&tibetan, detected), "{detected:?}");
        }

        // Nothing that already decided is reopened.
        for verdict in [
            Verdict::Clean { script: "Japanese_vert".into() },
            Verdict::OutOfBalloon,
            Verdict::OptedIn,
        ] {
            assert!(!rescue_applies(&verdict, Detected::TextInBubble), "{verdict:?}");
        }
    }

    /// The rescue decision itself, over every shape of reading, with no model
    /// anywhere near it.
    #[test]
    fn a_reading_rescues_only_when_it_is_long_enough_and_japanese_enough() {
        let held = Verdict::Uncertain;
        let clean = Verdict::Clean { script: RESCUED_SCRIPT.into() };

        assert_eq!(rescued(held.clone(), Some(&reading("こんにちは"))), clean);
        assert_eq!(rescued(held.clone(), Some(&reading("漢字だ"))), clean);
        // Two characters is the floor, and it is inclusive.
        assert_eq!(rescued(held.clone(), Some(&reading("あい"))), clean);

        // One character is a guess at the paper, not a reading.
        assert_eq!(rescued(held.clone(), Some(&reading("あ"))), held);
        assert_eq!(rescued(held.clone(), Some(&reading("あ."))), held, "punctuation is not evidence");
        assert_eq!(rescued(held.clone(), Some(&reading("あ 1"))), held);
        assert_eq!(rescued(held.clone(), Some(&reading(""))), held);
        // Latin the reader hallucinated over lettering it could not read.
        assert_eq!(rescued(held.clone(), Some(&reading("HELLO"))), held);
        // A mixture under the share: two of five is 0.4.
        assert_eq!(rescued(held.clone(), Some(&reading("あいABC"))), held);
        // Three of four is 0.75 and clears it.
        assert_eq!(rescued(held.clone(), Some(&reading("あいうA"))), clean);

        // No reader, or a reader that failed, leaves the verdict alone - which
        // is the behaviour of every machine that never downloaded the weights.
        assert_eq!(rescued(held.clone(), None), held);
        let not_japanese = Verdict::NotJapanese { script: "Syriac".into() };
        assert_eq!(rescued(not_japanese.clone(), None), not_japanese);
        assert_eq!(rescued(not_japanese, Some(&reading("セリフだ"))), clean);
    }

    /// A rescued region says in its own verdict that a reader put it there.
    /// The string reaches provenance and the probe, and a rescue that called
    /// itself `Japanese` would be indistinguishable from an identification.
    #[test]
    fn a_rescued_verdict_names_the_reader_and_still_cleans() {
        let verdict = rescued(Verdict::Uncertain, Some(&reading("やめろ")));
        assert_eq!(verdict, Verdict::Clean { script: RESCUED_SCRIPT.into() });
        assert!(verdict.cleans());
        assert_eq!(verdict.skip_cause(), None);
        assert_eq!(verdict.reason_key(), None);
        assert_ne!(RESCUED_SCRIPT, "Japanese");
        // And it is not a label the identifier can emit, so nothing that reads
        // the identifier's vocabulary can confuse the two.
        assert!(!CJK_LABELS.contains(&RESCUED_SCRIPT));
    }

    /// The constants the decision rests on, pinned so a sweep of either is a
    /// visible change rather than a silent one.
    #[test]
    fn the_rescue_thresholds_are_the_ones_the_decision_log_records() {
        assert_eq!(MIN_CJK_SHARE, 0.6);
        assert_eq!(MIN_RESCUE_CHARS, 2);
        assert_eq!(ocr::MAX_TOKENS, 64);
    }
}
