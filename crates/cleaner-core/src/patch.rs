//! A patch: the pixels one region's edit produced, and the record of how.
//!
//! Patch layers, never a flattened page: the displayed page is
//! `raw + ordered visible patches`, and nothing ever writes back into the
//! source raster.

use crate::image::Raster;
use crate::mask::Mask;

/// Which rung produced a patch. `decline` is not here: a declined region has no
/// patch, which is the whole point of declining.
///
/// Serialised as the rung id - the suffix of [`Engine::rung_key`] - because that
/// is the `engine` field in a persisted patch record and the
/// `provenance.engine` the seam restates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Fill,
    Denoise,
    Lama,
    Flux,
    Cloud,
    /// A brush stroke. **Not a rung**, and that is the whole of what separates
    /// it from the six above: there is no ladder position to escalate from or
    /// to, no quality verdict, and nothing to fit - the user said where the
    /// paint goes and what colour it is, and that is authoritative. It is
    /// an [`Engine`] variant only because that is the field a patch records
    /// what made it in.
    Paint,
    /// A clone or heal stroke. A rung in exactly the same sense `Paint` is:
    /// none. The pixels are measured from the page rather than invented, which
    /// is why it is a separate word from `Paint` in the record.
    Clone,
}

impl Engine {
    /// The i18n key the seam carries for this rung. No English crosses the
    /// seam.
    pub fn rung_key(self) -> &'static str {
        match self {
            Engine::Fill => "ladder.rung.fill",
            Engine::Denoise => "ladder.rung.denoise",
            Engine::Lama => "ladder.rung.lama",
            Engine::Flux => "ladder.rung.flux",
            Engine::Cloud => "ladder.rung.cloud",
            Engine::Paint => "ladder.rung.paint",
            Engine::Clone => "ladder.rung.clone",
        }
    }

    /// Whether this engine is a rung of the ladder rather than a hand tool
    /// that merely records itself in the same field.
    ///
    /// **The ladder's arithmetic has no answer for the two hand tools.**
    /// `rung()` is a total order the ceiling compares against, `step()` walks a
    /// Layers row's picker up and down it, and an automatic run escalates along
    /// it - none of which means anything for a stroke the user painted. Written
    /// as one predicate here rather than as an arm in each of those places, so
    /// a seventh rung and an eighth tool cannot drift apart.
    pub fn is_rung(self) -> bool {
        !matches!(self, Engine::Paint | Engine::Clone)
    }
}

/// What a cloud request cost and where it went. Separate from the rest of the
/// record because it is absent for every local rung.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CloudRecord {
    pub provider: String,
    pub model: String,
    pub request_id: String,
    pub tier: String,
    pub cost: f64,
}

/// Enough to reproduce a patch, or to know that it cannot be reproduced.
///
/// Field names are verbatim, because this record is
/// what gets persisted per patch and the seam contract restates the names.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Provenance {
    pub engine: Engine,
    pub engine_version: String,
    pub model_sha256: Option<String>,
    pub execution_provider: String,
    /// The thresholds, dilations and radii actually used - not the defaults.
    ///
    /// A JSON object, not a string holding JSON: it is a snapshot of
    /// values and `src/lib/model/types.js` declares it
    /// `Record<string, unknown>`, so a string here would reach both the
    /// manifest and the seam double-encoded.
    pub params_snapshot: serde_json::Value,
    pub mask_sha256: String,
    pub source_sha256: String,
    pub cloud: Option<CloudRecord>,
    /// Seconds since the Unix epoch. Stored as a number rather than formatted,
    /// for the same reason `RelativeTime` crosses the seam as data. The
    /// interface's own model declares this field an ISO 8601 string, so the
    /// adapter formats it.
    pub created: u64,
}

#[derive(Debug, Clone)]
pub struct Patch {
    pub id: String,
    /// The **applied** mask: post-growth, pre-isolation. The fidelity contract
    /// is stated over exactly this, and the two readings differ by up to
    /// 26 px on the inpaint path.
    pub mask: Mask,
    /// **The lettering this edit removed**: [`crate::fit::Fitted::ink`] for a
    /// rung, the stroke itself for a hand tool, and `mask` when nothing
    /// narrower is known. Usually a subset of `mask`, and on the fill path a
    /// much smaller one - rung 0 paints the whole grown balloon, and the grown
    /// balloon is not where the text was. The mask file an export writes
    /// beside a page is the union of these rather than of `mask`, because a
    /// typesetter asking "where was the text" wants the lettering, not the
    /// fill. Nothing else reads it: the compositor, the fidelity contract and
    /// a PSD's layer mask are all stated over `mask`.
    pub ink: Mask,
    /// Pixels covering `mask.bounds`, in the page's own mode and depth. A patch
    /// is never in a different colour space from the page it belongs to - that
    /// is what makes "no helpful promotion" enforceable.
    pub pixels: Raster,
    pub order: u32,
    pub visible: bool,
    pub provenance: Provenance,
}

impl Patch {
    /// Whether the patch's pixel buffer actually covers its mask.
    pub fn is_well_formed(&self) -> bool {
        self.pixels.width == self.mask.bounds.w && self.pixels.height == self.mask.bounds.h
    }
}
