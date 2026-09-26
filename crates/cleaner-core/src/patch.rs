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
///
/// When deserializing legacy manifests, optional fields are omitted or null
/// without fabricating unmeasured data.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CloudRecord {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe_id: Option<String>,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(default)]
    pub cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
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

    /// Presentation settings survive in the patch's provenance snapshot. Old
    /// manifests have no `layer` field and render exactly as before.
    pub fn layer_style(&self) -> LayerStyle {
        LayerStyle::from_snapshot(&self.provenance.params_snapshot)
    }

    /// Move and rotate the saved pixels and masks together for every consumer
    /// of `Job::load_patch`: tiles, underlays, flattened and layered exports.
    pub fn presented(mut self) -> Self {
        let style = self.layer_style();
        if style.offset_x == 0 && style.offset_y == 0 && style.rotation == 0.0 {
            return self;
        }
        let source = self.mask.bounds;
        if source.w == 0 || source.h == 0 { return self; }
        let cx = source.x as f64 + source.w as f64 / 2.0;
        let cy = source.y as f64 + source.h as f64 / 2.0;
        let angle = style.rotation.to_radians();
        let (sin, cos) = angle.sin_cos();
        let bounds = style.display_bounds(source);
        let mut mask = crate::mask::Mask::empty(bounds);
        let mut ink = crate::mask::Mask::empty(bounds);
        let mut pixels = self.pixels.clone();
        pixels.width = bounds.w;
        pixels.height = bounds.h;
        pixels.data = vec![0; crate::composite::blank_len(bounds.w, bounds.h, &self.pixels)];
        let channels = pixels.mode.samples();
        for y in 0..bounds.h {
            for x in 0..bounds.w {
                let world_x = bounds.x as f64 + x as f64 + 0.5 - style.offset_x as f64 - cx;
                let world_y = bounds.y as f64 + y as f64 + 0.5 - style.offset_y as f64 - cy;
                let sx = (cx + world_x * cos + world_y * sin).floor() as i64;
                let sy = (cy - world_x * sin + world_y * cos).floor() as i64;
                if self.mask.contains(sx, sy) {
                    mask.set(bounds.x + x as i64, bounds.y + y as i64, true);
                    let lx = (sx - source.x) as u32;
                    let ly = (sy - source.y) as u32;
                    for channel in 0..channels {
                        pixels.set_sample(x, y, channel, self.pixels.sample(lx, ly, channel));
                    }
                }
                if self.ink.contains(sx, sy) {
                    ink.set(bounds.x + x as i64, bounds.y + y as i64, true);
                }
            }
        }
        self.mask = mask;
        self.ink = ink;
        self.pixels = pixels;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LayerStyle {
    pub opacity: u8,
    pub offset_x: i32,
    pub offset_y: i32,
    pub rotation: f64,
    pub locked: bool,
}

impl Default for LayerStyle {
    fn default() -> Self {
        Self { opacity: 100, offset_x: 0, offset_y: 0, rotation: 0.0, locked: false }
    }
}

impl LayerStyle {
    pub fn from_snapshot(snapshot: &serde_json::Value) -> Self {
        serde_json::from_value::<Self>(snapshot.get("layer").cloned().unwrap_or_default())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn sanitized(mut self) -> Self {
        self.opacity = self.opacity.min(100);
        self.offset_x = self.offset_x.clamp(-10_000, 10_000);
        self.offset_y = self.offset_y.clamp(-10_000, 10_000);
        self.rotation = if self.rotation.is_finite() { self.rotation.clamp(-180.0, 180.0) } else { 0.0 };
        self
    }

    pub fn display_bounds(self, source: crate::mask::Rect) -> crate::mask::Rect {
        if self.offset_x == 0 && self.offset_y == 0 && self.rotation == 0.0 { return source; }
        let cx = source.x as f64 + source.w as f64 / 2.0;
        let cy = source.y as f64 + source.h as f64 / 2.0;
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let corners = [
            (source.x as f64, source.y as f64),
            (source.right() as f64, source.y as f64),
            (source.x as f64, source.bottom() as f64),
            (source.right() as f64, source.bottom() as f64),
        ];
        let transformed = corners.map(|(x, y)| {
            let (dx, dy) = (x - cx, y - cy);
            (cx + dx * cos - dy * sin + self.offset_x as f64,
             cy + dx * sin + dy * cos + self.offset_y as f64)
        });
        let left = transformed.iter().map(|p| p.0).fold(f64::INFINITY, f64::min).floor() as i64;
        let top = transformed.iter().map(|p| p.1).fold(f64::INFINITY, f64::min).floor() as i64;
        let right = transformed.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max).ceil() as i64;
        let bottom = transformed.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max).ceil() as i64;
        crate::mask::Rect::new(left, top, (right - left) as u32, (bottom - top) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_record_cost_none_serializes_explicit_null() {
        let record = CloudRecord {
            provider: "beam".to_string(),
            profile_id: Some("prof-1".to_string()),
            job_id: None,
            request_id: "req-123".to_string(),
            attempt_id: None,
            recipe_id: Some("flux-sdnq-v1".to_string()),
            model: "flux-schnell".to_string(),
            model_revision: None,
            tier: None,
            cost: None,
            duration_ms: Some(1200),
        };

        let json = serde_json::to_string(&record).unwrap();
        assert!(
            json.contains(r#""cost":null"#),
            "JSON must serialize explicit null cost: {json}"
        );

        let de: CloudRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(de.cost, None);
        assert_eq!(de.provider, "beam");
        assert_eq!(de.profile_id.as_deref(), Some("prof-1"));
        assert_eq!(de.recipe_id.as_deref(), Some("flux-sdnq-v1"));
    }

    #[test]
    fn cloud_record_cost_deserializes_missing_null_and_numeric() {
        // Missing cost field
        let missing_json = r#"{"provider":"modal","request_id":"req-1","model":"flux"}"#;
        let de_missing: CloudRecord = serde_json::from_str(missing_json).unwrap();
        assert_eq!(de_missing.cost, None);

        // Explicit null cost
        let null_json = r#"{"provider":"modal","request_id":"req-1","model":"flux","cost":null}"#;
        let de_null: CloudRecord = serde_json::from_str(null_json).unwrap();
        assert_eq!(de_null.cost, None);

        // Numeric cost
        let num_json = r#"{"provider":"modal","request_id":"req-1","model":"flux","cost":0.035}"#;
        let de_num: CloudRecord = serde_json::from_str(num_json).unwrap();
        assert_eq!(de_num.cost, Some(0.035));
    }
}
