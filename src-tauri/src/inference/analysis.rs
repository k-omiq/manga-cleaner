//! Explicit, review-only cloud page analysis. Each proposal covers one page and
//! one sequential batch. A failed or interrupted batch never resubmits itself.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cleaner_core::balloon::{BalloonBox, BalloonClass};
use cleaner_core::cloud_analysis_wire::{
    AnalysisCapabilities, AnalysisRequest, AnalysisResult, TileRect, RT, SAM, VERSION,
};
use cleaner_core::cloud_decode::decode_analysis_mask;
use cleaner_core::cloud_tiles;
use cleaner_core::engines::render::{CloudProvider, RenderRecipe};
use cleaner_core::image::Raster;
use cleaner_core::mask::Rect;
use cleaner_core::project::{Job, StripMode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::inference::http::{CloudHttpClient, HttpTransportError};
use crate::inference::journal::{
    AnalysisAttemptRecord, AnalysisJournal, AnalysisPhase, ANALYSIS_JOURNAL_SCHEMA_VERSION,
};
use crate::inference::policy::{GrantScope, GrantService};
use crate::library::Library;
use crate::run;

#[derive(Debug, Error, PartialEq, Eq)]
enum AnalysisFlowError {
    #[error("rights_attestation_required")]
    RightsAttestationRequired,
    #[error("retention_acknowledgement_required")]
    RetentionAcknowledgementRequired,
    #[error("analysis_stale: {0}")]
    Stale(&'static str),
    #[error("analysis_cancelled")]
    Cancelled,
    #[error("capability_unavailable: {0}")]
    CapabilityUnavailable(&'static str),
    #[error("{0}")]
    Other(String),
}

impl From<String> for AnalysisFlowError {
    fn from(value: String) -> Self { Self::Other(value) }
}

impl From<&'static str> for AnalysisFlowError {
    fn from(value: &'static str) -> Self { Self::Other(value.into()) }
}

fn require_consent(rights_attested: bool, retention_acknowledged: bool)
    -> Result<(), AnalysisFlowError> {
    if !rights_attested { return Err(AnalysisFlowError::RightsAttestationRequired); }
    if !retention_acknowledged { return Err(AnalysisFlowError::RetentionAcknowledgementRequired); }
    Ok(())
}

fn require_active(cancel: &AtomicBool) -> Result<(), AnalysisFlowError> {
    if cancel.load(Ordering::SeqCst) { return Err(AnalysisFlowError::Cancelled); }
    Ok(())
}

fn require_unchanged(unchanged: bool, context: &'static str) -> Result<(), AnalysisFlowError> {
    if !unchanged { return Err(AnalysisFlowError::Stale(context)); }
    Ok(())
}

pub(crate) trait AnalysisGateway {
    fn capabilities(&self) -> Result<AnalysisCapabilities, String>;
    fn analyze(&self, request: &AnalysisRequest, png: &[u8]) -> Result<AnalysisResult, String>;
}

impl AnalysisGateway for CloudHttpClient {
    fn capabilities(&self) -> Result<AnalysisCapabilities, String> {
        self.get_analysis_capabilities().map_err(|e| match e {
            HttpTransportError::WireValidation | HttpTransportError::JsonDeserialization =>
                "capability_unavailable: gateway advertisement is invalid".into(),
            HttpTransportError::UnexpectedStatus { status: 503 } =>
                "capability_unavailable: gateway analysis is not configured".into(),
            other => other.to_string(),
        })
    }

    fn analyze(&self, request: &AnalysisRequest, png: &[u8]) -> Result<AnalysisResult, String> {
        self.submit_analysis_tile(request, png).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TileDisclosure {
    pub rect: TileRect,
    pub png_sha256: String,
    pub encoded_bytes: usize,
    pub input_sha256: String,
    pub predecessors_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisProposal {
    pub proposal_id: String,
    pub chapter_id: String,
    pub page_index: usize,
    pub provider: CloudProvider,
    pub profile_id: String,
    pub profile_name: String,
    pub capability: String,
    pub graph_sha256s: Vec<String>,
    pub model_revision: String,
    pub source_page_sha256: String,
    pub underlay_sha256: String,
    pub predecessors_sha256: String,
    pub project_revision_sha256: String,
    pub page_width: u32,
    pub page_height: u32,
    pub pages: u32,
    pub total_tile_pixels: u64,
    pub total_encoded_bytes: u64,
    pub includes_surrounding_art: bool,
    pub cost_estimate_usd: Option<f64>,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub tiles: Vec<TileDisclosure>,
}

struct StoredProposal {
    proposal: AnalysisProposal,
    job_path: PathBuf,
    endpoint_fingerprint: String,
    profile_epoch: u64,
    confirmed: bool,
    finished: bool,
    cancel: Arc<AtomicBool>,
    #[cfg(test)]
    before_submit: Option<Arc<dyn Fn() + Send + Sync>>,
}

fn proposals() -> &'static Mutex<HashMap<String, StoredProposal>> {
    static PROPOSALS: OnceLock<Mutex<HashMap<String, StoredProposal>>> = OnceLock::new();
    PROPOSALS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn hex(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn random_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "Random source unavailable")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn source_page(job: &Job, page_index: usize) -> Result<(usize, Vec<u8>, Raster), String> {
    if job.project.strip.mode != StripMode::Single {
        return Err("Remote analysis currently requires a paginated chapter".into());
    }
    let source_idx = Library::resolve_page(&job.project, page_index)
        .ok_or("Page is no longer in chapter")?;
    let declared = job.project.sources.get(source_idx).ok_or("Chapter source is missing")?;
    if u64::from(declared.w) * u64::from(declared.h) > 24_000_000 {
        return Err("Analysis page exceeds the 24 megapixel review limit".into());
    }
    let path = job.source_path(source_idx).ok_or("Chapter source is missing")?;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 20_000_000 { return Err("Source page exceeds the review-preview limit".into()); }
    crate::model_workflows::source_mime(&bytes)?;
    let page = cleaner_core::image::decode(&bytes)
        .or_else(|_| cleaner_core::image::foreign::decode(&bytes))
        .map_err(|e| e.to_string())?;
    if page.width != declared.w || page.height != declared.h
        || u64::from(page.width) * u64::from(page.height) > 24_000_000 {
        return Err("Analysis source geometry changed or exceeds the review limit".into());
    }
    Ok((source_idx, bytes, page))
}

type PreparedTiles = (Vec<u8>, u32, u32, Vec<(TileDisclosure, Vec<u8>)>);

fn prepared_tiles(
    job_path: &Path,
    page_index: usize,
    regions: &[TileRect],
) -> Result<PreparedTiles, String> {
    let _lock = run::lock_job(job_path);
    let job = Job::open(job_path).map_err(|e| e.to_string())?;
    let (source_idx, source, page) = source_page(&job, page_index)?;
    let extent = cloud_tiles::plan(page.width, page.height, regions)?;
    let mut tiles = Vec::with_capacity(extent.tiles.len());
    for rect in extent.tiles {
        let input = crate::underlay::read_exact(&job, source_idx, &page,
            Rect::new(rect.x as i64, rect.y as i64, rect.width, rect.height),
            u32::MAX, None)?;
        let mut rgb = Vec::with_capacity((rect.width * rect.height * 3) as usize);
        for y in 0..rect.height {
            for x in 0..rect.width {
                rgb.extend_from_slice(&input.image.rgb8_pixel(x, y));
            }
        }
        let png = crate::inference::consent::encode_rgb8_png(rect.width, rect.height, &rgb)
            .map_err(|e| e.to_string())?;
        if png.len() > cleaner_core::cloud_analysis_wire::MAX_PNG_BYTES {
            return Err("Analysis tile PNG exceeds transfer limit".into());
        }
        tiles.push((TileDisclosure {
            rect, png_sha256: hex(&png), encoded_bytes: png.len(),
            input_sha256: input.digest, predecessors_sha256: input.predecessors,
        }, png));
    }
    Ok((source, page.width, page.height, tiles))
}

fn joined_digest<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut hash = Sha256::new();
    for value in values { hash.update(value.as_bytes()); hash.update(b"\0"); }
    format!("{:x}", hash.finalize())
}

fn project_revision(job_path: &Path, page_index: usize) -> Result<String, String> {
    let _lock = run::lock_job(job_path);
    project_revision_locked(job_path, page_index)
}

fn project_revision_locked(job_path: &Path, page_index: usize) -> Result<String, String> {
    let job = Job::open(job_path).map_err(|e| e.to_string())?;
    let source_idx = Library::resolve_page(&job.project, page_index)
        .ok_or("Page is no longer in chapter")?;
    let source = job.source_path(source_idx).ok_or("Chapter source is missing")?;
    let mut hash = Sha256::new();
    hash.update(b"analysis_project_revision_v1\0");
    hash.update(std::fs::read(source).map_err(|e| e.to_string())?);
    for record in &job.project.patches {
        if record.source_idx != source_idx { continue; }
        hash.update(serde_json::to_vec(record).map_err(|e| e.to_string())?);
        let patch = job.load_patch(record).map_err(|e| e.to_string())?;
        hash.update(&patch.mask.bits);
        hash.update(&patch.ink.bits);
        hash.update(&patch.pixels.data);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn capability<'a>(available: &'a AnalysisCapabilities, id: &str)
    -> Result<&'a cleaner_core::cloud_analysis_wire::AnalysisCapability, AnalysisFlowError> {
    available.validate().map_err(|_| AnalysisFlowError::CapabilityUnavailable("gateway advertisement is invalid"))?;
    available.capabilities.iter().find(|entry| entry.capability == id)
        .ok_or(AnalysisFlowError::CapabilityUnavailable("gateway does not advertise this analysis model"))
}

fn check_gateway_limits(available: &AnalysisCapabilities, tiles: &[TileDisclosure]) -> Result<(), AnalysisFlowError> {
    if tiles.iter().any(|tile| tile.rect.width > available.limits.max_tile_side
        || tile.rect.height > available.limits.max_tile_side
        || u64::from(tile.rect.width) * u64::from(tile.rect.height) > available.limits.max_tile_pixels
        || tile.encoded_bytes > available.limits.max_png_bytes) {
        return Err(AnalysisFlowError::CapabilityUnavailable("gateway tile limits are smaller than this upload"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn propose<G: AnalysisGateway>(
    job_path: PathBuf,
    chapter_id: String,
    page_index: usize,
    provider: CloudProvider,
    profile_id: String,
    profile_name: String,
    endpoint_fingerprint: String,
    capability_id: String,
    regions: Vec<TileRect>,
    client: &G,
    journal: &AnalysisJournal,
) -> Result<AnalysisProposal, String> {
    if !matches!(capability_id.as_str(), SAM | RT) {
        return Err("capability_unavailable: unsupported analysis capability".into());
    }
    let available = client.capabilities()?;
    let advertised = capability(&available, &capability_id).map_err(|e| e.to_string())?;
    let (source, page_width, page_height, tiles) = prepared_tiles(&job_path, page_index, &regions)?;
    let disclosures: Vec<_> = tiles.into_iter().map(|(tile, _)| tile).collect();
    check_gateway_limits(&available, &disclosures).map_err(|e| e.to_string())?;
    let total_tile_pixels = disclosures.iter().map(|t| u64::from(t.rect.width) * u64::from(t.rect.height)).sum();
    let total_encoded_bytes = disclosures.iter().map(|t| t.encoded_bytes as u64).sum();
    let underlay_sha256 = joined_digest(disclosures.iter().map(|t| t.input_sha256.as_str()));
    let predecessors_sha256 = joined_digest(disclosures.iter().map(|t| t.predecessors_sha256.as_str()));
    let issued_at_ms = now_ms();
    let proposal = AnalysisProposal {
        proposal_id: random_id()?, chapter_id, page_index, provider,
        profile_id, profile_name, capability: capability_id,
        graph_sha256s: advertised.graph_sha256s.clone(), model_revision: advertised.model_revision.clone(),
        source_page_sha256: hex(&source), underlay_sha256, predecessors_sha256,
        project_revision_sha256: project_revision(&job_path, page_index)?,
        page_width, page_height, pages: 1, total_tile_pixels, total_encoded_bytes,
        includes_surrounding_art: true, cost_estimate_usd: None, tiles: disclosures,
        issued_at_ms, expires_at_ms: issued_at_ms.saturating_add(
            crate::inference::consent::DEFAULT_PROPOSAL_TTL.as_millis() as u64),
    };
    let record = AnalysisAttemptRecord {
        created_at_ms: issued_at_ms,
        schema_version: ANALYSIS_JOURNAL_SCHEMA_VERSION,
        proposal_id: proposal.proposal_id.clone(), provider, profile_id: proposal.profile_id.clone(),
        capability: proposal.capability.clone(), source_sha256: proposal.source_page_sha256.clone(),
        underlay_sha256: proposal.underlay_sha256.clone(), total_tiles: proposal.tiles.len(),
        completed_tiles: 0, reported_cost_usd: None,
        tile_submission_started: Some(false), cancel_requested: false,
        phase: AnalysisPhase::Proposed,
    };
    let epoch = GrantService::global().get_profile_epoch(provider, &proposal.profile_id);
    let mut cache = proposals().lock().map_err(|e| e.to_string())?;
    cache.retain(|_, stored| stored.confirmed || stored.proposal.expires_at_ms > issued_at_ms);
    if cache.len() >= crate::inference::consent::MAX_CACHED_PROPOSALS {
        let oldest = cache.iter().filter(|(_, stored)| stored.finished)
            .min_by_key(|(_, stored)| stored.proposal.issued_at_ms)
            .map(|(id, _)| id.clone());
        if let Some(id) = oldest { cache.remove(&id); }
        else { return Err("analysis_proposal_limit".into()); }
    }
    journal.write(&record).map_err(|e| e.to_string())?;
    cache.insert(proposal.proposal_id.clone(), StoredProposal {
        proposal: proposal.clone(), job_path, endpoint_fingerprint, profile_epoch: epoch,
        confirmed: false, finished: false, cancel: Arc::new(AtomicBool::new(false)),
        #[cfg(test)]
        before_submit: None,
    });
    Ok(proposal)
}

fn scope(proposal: &AnalysisProposal, endpoint_fingerprint: &str) -> GrantScope {
    GrantScope {
        capability: proposal.capability.clone(), provider: proposal.provider,
        profile_id: proposal.profile_id.clone(),
        canonical_endpoint_fingerprint: endpoint_fingerprint.to_string(),
        source_hash: proposal.source_page_sha256.clone(),
        crop_sha256: proposal.tiles[0].png_sha256.clone(),
        hint_sha256: proposal.underlay_sha256.clone(),
        operation_digest: hex(proposal.proposal_id.as_bytes()),
        crop_bounds: Rect::new(0, 0, proposal.page_width, proposal.page_height),
        mask_hash: proposal.underlay_sha256.clone(), revision: 0,
        input_sha256: proposal.underlay_sha256.clone(),
        predecessors_sha256: proposal.predecessors_sha256.clone(),
        recipe: RenderRecipe::new(&proposal.capability, VERSION, &proposal.capability,
            &proposal.model_revision, false),
        region_ids: vec![format!("page_{}", proposal.page_index)],
    }
}

fn same_tiles(proposal: &AnalysisProposal, source: &[u8], width: u32, height: u32,
    tiles: &[(TileDisclosure, Vec<u8>)]) -> bool {
    width == proposal.page_width && height == proposal.page_height
        && hex(source) == proposal.source_page_sha256
        && tiles.len() == proposal.tiles.len()
        && tiles.iter().zip(&proposal.tiles).all(|((actual, _), approved)|
            actual.rect == approved.rect && actual.png_sha256 == approved.png_sha256
            && actual.input_sha256 == approved.input_sha256
            && actual.predecessors_sha256 == approved.predecessors_sha256)
}

fn tile_request(proposal: &AnalysisProposal, index: usize) -> Result<AnalysisRequest, String> {
    let tile = &proposal.tiles[index];
    let mut request = AnalysisRequest {
        protocol_version: VERSION.into(), capability: proposal.capability.clone(),
        graph_sha256s: proposal.graph_sha256s.clone(), model_revision: proposal.model_revision.clone(),
        tile_id: format!("tile_{index}"), tile_rect: tile.rect,
        tile_png_sha256: tile.png_sha256.clone(),
        source_page_sha256: proposal.source_page_sha256.clone(), request_digest: String::new(),
    };
    request.request_digest = request.digest()?;
    Ok(request)
}

pub fn cancel(proposal_id: &str) -> Result<bool, String> {
    let proposals = proposals().lock().map_err(|e| e.to_string())?;
    let Some(stored) = proposals.get(proposal_id) else { return Ok(false) };
    if stored.finished { return Ok(false); }
    stored.cancel.store(true, Ordering::SeqCst);
    Ok(true)
}

pub(crate) fn confirm_and_run<G: AnalysisGateway>(
    proposal_id: &str,
    rights_attested: bool,
    retention_acknowledged: bool,
    client: &G,
    journal: &AnalysisJournal,
    progress: impl Fn(&AnalysisAttemptRecord),
) -> Result<crate::model_workflows::Analysis, String> {
    require_consent(rights_attested, retention_acknowledged).map_err(|e| e.to_string())?;
    let (proposal, path, endpoint_fingerprint, epoch, cancel) = {
        let mut map = proposals().lock().map_err(|e| e.to_string())?;
        let stored = map.get_mut(proposal_id).ok_or("analysis_proposal_missing")?;
        if stored.confirmed { return Err("analysis_proposal_consumed".into()); }
        if now_ms() >= stored.proposal.expires_at_ms { return Err("analysis_proposal_expired".into()); }
        stored.confirmed = true;
        (stored.proposal.clone(), stored.job_path.clone(), stored.endpoint_fingerprint.clone(),
            stored.profile_epoch, Arc::clone(&stored.cancel))
    };
    let mut record = journal.read(proposal_id).map_err(|e| e.to_string())?;
    let mut run = || -> Result<crate::model_workflows::Analysis, AnalysisFlowError> {
        require_active(&cancel)?;
        let advertised = client.capabilities()?;
        let current = capability(&advertised, &proposal.capability)?;
        if current.graph_sha256s != proposal.graph_sha256s || current.model_revision != proposal.model_revision {
            return Err(AnalysisFlowError::CapabilityUnavailable("model identity changed"));
        }
        check_gateway_limits(&advertised, &proposal.tiles)?;
        let rects: Vec<_> = proposal.tiles.iter().map(|tile| tile.rect).collect();
        let (source, width, height, tiles) = prepared_tiles(&path, proposal.page_index, &rects)?;
        require_unchanged(same_tiles(&proposal, &source, width, height, &tiles)
            && project_revision(&path, proposal.page_index)? == proposal.project_revision_sha256,
            "source or underlay changed before dispatch")?;
        let grant_scope = scope(&proposal, &endpoint_fingerprint);
        let grants = GrantService::global();
        let grant = grants.issue_grant_with_epoch(grant_scope.clone(), epoch, Duration::from_secs(300), 1)
            .map_err(|e| e.to_string())?;
        grants.validate_and_consume(&grant.nonce, &grant_scope).map_err(|e| e.to_string())?;
        {
            let cache = proposals().lock().map_err(|e| e.to_string())?;
            require_active(&cancel)?;
            record.phase = AnalysisPhase::Confirmed;
            journal.write(&record).map_err(|e| e.to_string())?;
            drop(cache);
        }
        progress(&record);
        let page_pixels = usize::try_from(u64::from(width) * u64::from(height))
            .map_err(|_| "analysis page too large")?;
        let mut mask = (proposal.capability == SAM).then(|| vec![0u8; page_pixels]);
        let mut boxes = Vec::new();
        let mut all_costs_known = true;
        let mut cost = 0.0;
        for (index, (_, png)) in tiles.iter().enumerate() {
            require_active(&cancel)?;
            require_unchanged(project_revision(&path, proposal.page_index)? == proposal.project_revision_sha256,
                "source or underlay changed during batch")?;
            let request = tile_request(&proposal, index)?;
            request.validate(png)?;
            #[cfg(test)]
            {
                let before_submit = proposals().lock().map_err(|e| e.to_string())?
                    .get(proposal_id).and_then(|stored| stored.before_submit.clone());
                if let Some(hook) = before_submit { hook(); }
            }
            {
                let cache = proposals().lock().map_err(|e| e.to_string())?;
                require_active(&cancel)?;
                record.phase = AnalysisPhase::SubmittedTile { index };
                record.tile_submission_started = Some(true);
                journal.write(&record).map_err(|e| e.to_string())?;
                drop(cache);
            }
            progress(&record);
            let result = client.analyze(&request, png)?;
            result.validate(&request)?;
            stitch(&result, &mut mask, &mut boxes, width, height)?;
            journal.cache_tile_result(proposal_id, index, &result).map_err(|e| e.to_string())?;
            if let Some(value) = result.reported_cost_usd { cost += value; } else { all_costs_known = false; }
            record.completed_tiles = index + 1;
            record.reported_cost_usd = all_costs_known.then_some(cost);
            {
                let cache = proposals().lock().map_err(|e| e.to_string())?;
                record.cancel_requested = cancel.load(Ordering::SeqCst);
                record.phase = AnalysisPhase::ResultCachedTile { index };
                journal.write(&record).map_err(|e| e.to_string())?;
                drop(cache);
            }
            progress(&record);
            // The gateway has no cancel endpoint. A request cancelled during analyze
            // becomes cancelled only after its response is validated and cached.
            require_active(&cancel)?;
        }
        let (now_source, now_width, now_height, now_tiles) = prepared_tiles(&path, proposal.page_index, &rects)?;
        require_unchanged(same_tiles(&proposal, &now_source, now_width, now_height, &now_tiles),
            "source or underlay changed before evidence attachment")?;
        let _lock = run::lock_job(&path);
        require_unchanged(project_revision_locked(&path, proposal.page_index)? == proposal.project_revision_sha256,
            "source or underlay changed before evidence attachment")?;
        require_active(&cancel)?;
        let identity = format!("{}:{}", proposal.model_revision, proposal.graph_sha256s.join(":"));
        let evidence = crate::model_workflows::store_remote_analysis(
            proposal.provider.as_str(), &proposal.capability, &identity,
            &source, width, height, mask, boxes,
        )?;
        record.phase = AnalysisPhase::AttachedEvidence;
        journal.write(&record).map_err(|e| e.to_string())?;
        progress(&record);
        Ok(evidence)
    };
    let result = run();
    if let Err(error) = &result {
        record.cancel_requested = cancel.load(Ordering::SeqCst);
        record.phase = if *error == AnalysisFlowError::Cancelled { AnalysisPhase::Cancelled }
            else if matches!(record.phase, AnalysisPhase::SubmittedTile { .. }) {
                AnalysisPhase::UnknownRemoteState { index: record.completed_tiles }
            } else { AnalysisPhase::Failed { code: error.to_string().split(':').next().unwrap_or("analysis_failed").to_string() } };
        let _ = journal.write(&record);
        progress(&record);
    }
    if let Ok(mut cache) = proposals().lock() {
        if let Some(stored) = cache.get_mut(proposal_id) { stored.finished = true; }
    }
    result.map_err(|e| e.to_string())
}

fn stitch(result: &AnalysisResult, mask: &mut Option<Vec<u8>>, boxes: &mut Vec<BalloonBox>,
    page_width: u32, page_height: u32) -> Result<(), String> {
    let tile = result.tile_rect;
    if tile.x.checked_add(tile.width).is_none_or(|right| right > page_width)
        || tile.y.checked_add(tile.height).is_none_or(|bottom| bottom > page_height)
    { return Err("analysis result lies outside page".into()); }
    if let Some(pixels) = mask {
        let encoded = result.mask_bytes()?.ok_or("analysis mask missing")?;
        let decoded = decode_analysis_mask(&encoded, tile).map_err(|e| e.to_string())?;
        for y in 0..tile.height as usize {
            let source = y * tile.width as usize;
            let target = (tile.y as usize + y) * page_width as usize + tile.x as usize;
            pixels[target..target + tile.width as usize]
                .copy_from_slice(&decoded[source..source + tile.width as usize]);
        }
    }
    for item in &result.boxes {
        let class = match item.class {
            0 => BalloonClass::Bubble, 1 => BalloonClass::TextInBubble,
            2 => BalloonClass::TextFree, _ => return Err("invalid analysis box class".into()),
        };
        boxes.push(BalloonBox { rect: Rect::new(
            i64::from(tile.x + item.rect.x), i64::from(tile.y + item.rect.y),
            item.rect.width, item.rect.height,
        ), class, score: item.score });
    }
    Ok(())
}

fn journal_for(app: &tauri::AppHandle) -> Result<AnalysisJournal, String> {
    Ok(AnalysisJournal::new(crate::inference::commands::get_app_journal_dir(app)?))
}

fn profile(
    app: &tauri::AppHandle, provider: CloudProvider, profile_id: &str,
) -> Result<(crate::inference::config::CloudProfile, CloudHttpClient), String> {
    let config = crate::inference::commands::read_inference_config(app.clone())?;
    if config.selected_target.provider() != Some(provider)
        || config.selected_target.profile_id() != Some(profile_id) {
        return Err("analysis_profile_not_active".into());
    }
    let profile = match provider {
        CloudProvider::Beam => config.beam_profiles.get(profile_id),
        CloudProvider::Modal => config.modal_profiles.get(profile_id),
    }.ok_or("analysis_profile_missing")?.clone();
    let client = crate::inference::commands::build_client_for_profile(&config, provider, profile_id)
        .map_err(|e| e.to_string())?;
    Ok((profile, client))
}

#[tauri::command]
pub async fn list_remote_analysis_capabilities(
    app: tauri::AppHandle, provider: CloudProvider, profile_id: String,
) -> Result<AnalysisCapabilities, String> {
    if !crate::inference::cloud_allowed(&app) { return Err("cloud_disabled".into()); }
    tauri::async_runtime::spawn_blocking(move || {
        let (_, client) = profile(&app, provider, &profile_id)?;
        client.capabilities()
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn propose_remote_analysis(
    app: tauri::AppHandle, chapter_id: String, page_index: usize,
    provider: CloudProvider, profile_id: String, capability: String,
    regions: Vec<TileRect>,
) -> Result<AnalysisProposal, String> {
    if !crate::inference::cloud_allowed(&app) { return Err("cloud_disabled".into()); }
    tauri::async_runtime::spawn_blocking(move || {
        let (selected, client) = profile(&app, provider, &profile_id)?;
        let path = Library::for_app(&app).map_err(|e| e.to_string())?
            .resolve_chapter(&chapter_id).map_err(|e| e.to_string())?;
        let endpoint = crate::inference::config::compute_canonical_endpoint_fingerprint(&selected.endpoint_url)?;
        propose(path, chapter_id, page_index, provider, profile_id, selected.name,
            endpoint, capability, regions, &client, &journal_for(&app)?)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn confirm_remote_analysis(
    app: tauri::AppHandle, proposal_id: String,
    rights_attested: bool, retention_acknowledged: bool,
) -> Result<crate::model_workflows::Analysis, String> {
    if !crate::inference::cloud_allowed(&app) { return Err("cloud_disabled".into()); }
    require_consent(rights_attested, retention_acknowledged).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let (provider, profile_id) = {
            let cache = proposals().lock().map_err(|e| e.to_string())?;
            let stored = cache.get(&proposal_id).ok_or("analysis_proposal_missing")?;
            (stored.proposal.provider, stored.proposal.profile_id.clone())
        };
        let (_, client) = profile(&app, provider, &profile_id)?;
        let journal = journal_for(&app)?;
        use tauri::Emitter;
        confirm_and_run(&proposal_id, rights_attested, retention_acknowledged,
            &client, &journal, |record| { let _ = app.emit("cloud://analysis", record); })
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn cancel_remote_analysis(app: tauri::AppHandle, proposal_id: String) -> Result<bool, String> {
    let journal = journal_for(&app)?;
    cancel_with_journal(&proposal_id, &journal)
}

fn cancel_with_journal(proposal_id: &str, journal: &AnalysisJournal) -> Result<bool, String> {
    let cache = proposals().lock().map_err(|e| e.to_string())?;
    let mut record = journal.read(proposal_id).map_err(|e| e.to_string())?;
    if matches!(record.phase, AnalysisPhase::AttachedEvidence | AnalysisPhase::Cancelled | AnalysisPhase::Failed { .. }) {
        return Ok(false);
    }
    let active = cache.get(proposal_id).filter(|stored| !stored.finished);
    if active.is_none() {
        record.phase = if matches!(record.phase, AnalysisPhase::Proposed) {
            AnalysisPhase::Cancelled
        } else {
            journal.recover(proposal_id).map_err(|e| e.to_string())?.phase
        };
    } else if matches!(record.phase, AnalysisPhase::Proposed) {
        record.phase = AnalysisPhase::Cancelled;
    }
    record.cancel_requested = true;
    journal.write(&record).map_err(|e| e.to_string())?;
    if let Some(stored) = active { stored.cancel.store(true, Ordering::SeqCst); }
    Ok(true)
}

#[tauri::command]
pub fn get_remote_analysis_status(
    app: tauri::AppHandle, proposal_id: String,
) -> Result<AnalysisAttemptRecord, String> {
    let journal = journal_for(&app)?;
    let active = proposals().lock().map_err(|e| e.to_string())?.contains_key(&proposal_id);
    if active { journal.read(&proposal_id) } else { journal.recover(&proposal_id) }
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use cleaner_core::cloud_analysis_wire::{AnalysisCapability, AnalysisLimits, AnalysisTimings};
    use cleaner_core::image::{fixtures, Format};
    use cleaner_core::mask::Mask;
    use cleaner_core::patch::{Engine, Patch, Provenance};
    use cleaner_core::project::Project;
    use base64::Engine as _;

    struct FakeGateway {
        submissions: AtomicUsize,
        change_predecessor: Option<PathBuf>,
        cancel_after_first: Mutex<Option<String>>,
    }

    impl AnalysisGateway for FakeGateway {
        fn capabilities(&self) -> Result<AnalysisCapabilities, String> {
            Ok(AnalysisCapabilities {
                protocol_version: VERSION.into(),
                capabilities: vec![AnalysisCapability {
                    capability: SAM.into(), graph_sha256s: vec!["a".repeat(64), "b".repeat(64)],
                    model_revision: "c".repeat(40),
                }],
                limits: AnalysisLimits {
                    max_tile_side: 1024, max_tile_pixels: 1_048_576,
                    max_png_bytes: 4_194_304, max_components: 4096, max_boxes: 4096,
                },
            })
        }

        fn analyze(&self, request: &AnalysisRequest, png: &[u8]) -> Result<AnalysisResult, String> {
            request.validate(png)?;
            let count = self.submissions.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                if let Some(id) = self.cancel_after_first.lock().unwrap().as_ref() {
                    cancel(id)?;
                }
            }
            if let Some(path) = &self.change_predecessor {
                let mut job = Job::open(path).map_err(|e| e.to_string())?;
                job.project.patches[0].visible = false;
                job.flush().map_err(|e| e.to_string())?;
            }
            let mut bytes = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut bytes, request.tile_rect.width, request.tile_rect.height);
                encoder.set_color(png::ColorType::Grayscale);
                encoder.set_depth(png::BitDepth::Eight);
                encoder.write_header().map_err(|e| e.to_string())?
                    .write_image_data(&vec![255; (request.tile_rect.width * request.tile_rect.height) as usize])
                    .map_err(|e| e.to_string())?;
            }
            let result = AnalysisResult {
                protocol_version: VERSION.into(), capability: request.capability.clone(),
                request_digest: request.request_digest.clone(), tile_id: request.tile_id.clone(),
                tile_rect: request.tile_rect, graph_sha256s: request.graph_sha256s.clone(),
                model_revision: request.model_revision.clone(),
                mask_png_b64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
                components: vec![], boxes: vec![],
                timings: AnalysisTimings { load_ms: 0, preprocess_ms: 0, inference_ms: 0, postprocess_ms: 0 },
                reported_cost_usd: None,
            };
            result.validate(request)?;
            Ok(result)
        }
    }

    fn fixture(name: &str) -> (PathBuf, AnalysisJournal) {
        fixture_with_width(name, 256)
    }

    fn fixture_with_width(name: &str, width: u32) -> (PathBuf, AnalysisJournal) {
        let root = std::env::temp_dir().join(format!("mc-analysis-{name}-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&root).unwrap();
        let mut page = fixtures::by_name("l8").raster;
        page.width = width;
        page.height = 256;
        page.data = vec![200; width as usize * 256];
        let source = root.join("page.png");
        let bytes = cleaner_core::image::encode(&page, Format::Png).unwrap();
        std::fs::write(&source, &bytes).unwrap();
        let reference = cleaner_core::ingest::source_ref(&source, &bytes).unwrap();
        let manifest = root.join("job/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project = Project::new(manifest.parent().unwrap(), "test-app", StripMode::Single, &[reference]);
        let mut job = Job::create(&manifest, project).unwrap();
        let bounds = Rect::new(20, 20, 16, 16);
        let mut pixels = page;
        pixels.width = 16;
        pixels.height = 16;
        pixels.data = vec![100; 16 * 16];
        let patch = Patch {
            id: "earlier".into(), mask: Mask::filled(bounds), ink: Mask::filled(bounds),
            pixels, order: 1, visible: true,
            provenance: Provenance {
                engine: Engine::Fill, engine_version: "1.0".into(), model_sha256: None,
                execution_provider: "cpu".into(), params_snapshot: serde_json::json!({}),
                mask_sha256: hex(&vec![255; 16 * 16]), source_sha256: hex(&bytes),
                cloud: None, created: 1_760_000_000,
            },
        };
        job.complete_region(0, &patch, None).unwrap();
        let journal = AnalysisJournal::new(root.join("journal"));
        (manifest, journal)
    }

    fn proposal(path: PathBuf, journal: &AnalysisJournal, gateway: &FakeGateway) -> AnalysisProposal {
        propose(path, "chapter".into(), 0, CloudProvider::Modal, "modal-prof-1".into(),
            "Modal".into(), "a".repeat(64), SAM.into(), vec![], gateway, journal).unwrap()
    }

    #[test]
    fn analysis_consent_submit_and_review_evidence_stay_review_only() {
        let (path, journal) = fixture("happy");
        let gateway = FakeGateway { submissions: AtomicUsize::new(0), change_predecessor: None,
            cancel_after_first: Mutex::new(None) };
        let proposal = proposal(path, &journal, &gateway);
        assert_eq!(proposal.pages, 1);
        assert!(proposal.includes_surrounding_art);
        assert_eq!(proposal.cost_estimate_usd, None);
        assert_eq!(require_consent(false, true).unwrap_err(), AnalysisFlowError::RightsAttestationRequired);
        assert_eq!(require_consent(true, false).unwrap_err(), AnalysisFlowError::RetentionAcknowledgementRequired);
        assert_eq!(confirm_and_run(&proposal.proposal_id, false, true, &gateway, &journal, |_| {}).err().unwrap(),
            AnalysisFlowError::RightsAttestationRequired.to_string());
        assert_eq!(confirm_and_run(&proposal.proposal_id, true, false, &gateway, &journal, |_| {}).err().unwrap(),
            AnalysisFlowError::RetentionAcknowledgementRequired.to_string());
        assert_eq!(gateway.submissions.load(Ordering::SeqCst), 0);
        let evidence = confirm_and_run(&proposal.proposal_id, true, true, &gateway, &journal, |_| {}).unwrap();
        let value = serde_json::to_value(evidence).unwrap();
        assert_eq!(value["samWriteEligible"], false);
        assert_eq!(value["remoteSource"], "remote:modal:text_mask_sam_ts@1");
        assert_eq!(gateway.submissions.load(Ordering::SeqCst), 1);
        assert!(matches!(journal.read(&proposal.proposal_id).unwrap().phase, AnalysisPhase::AttachedEvidence));
    }

    #[test]
    fn predecessor_change_after_submission_stales_without_second_job() {
        let (path, journal) = fixture_with_width("stale", 1300);
        let gateway = FakeGateway { submissions: AtomicUsize::new(0), change_predecessor: Some(path.clone()),
            cancel_after_first: Mutex::new(None) };
        let proposal = proposal(path, &journal, &gateway);
        assert_eq!(proposal.tiles.len(), 2);
        assert_eq!(require_unchanged(false, "source or underlay changed during batch").unwrap_err(),
            AnalysisFlowError::Stale("source or underlay changed during batch"));
        let error = confirm_and_run(&proposal.proposal_id, true, true, &gateway, &journal, |_| {}).err().unwrap();
        assert_eq!(error, AnalysisFlowError::Stale("source or underlay changed during batch").to_string());
        assert_eq!(gateway.submissions.load(Ordering::SeqCst), 1);
        assert_eq!(journal.read(&proposal.proposal_id).unwrap().completed_tiles, 1);
        assert!(!matches!(journal.read(&proposal.proposal_id).unwrap().phase, AnalysisPhase::AttachedEvidence));
    }

    #[test]
    fn cloud_unconfigured_does_not_affect_local_analysis_contract() {
        let config = crate::inference::config::InferenceConfig::default();
        assert!(config.beam_profiles.is_empty() && config.modal_profiles.is_empty());
        let page = fixtures::by_name("l8").raster;
        let mask = vec![0; (page.width * page.height) as usize];
        assert!(cleaner_core::fusion::fuse(page.width, page.height, Some(&mask), &[], &[]).is_ok());
    }

    #[test]
    fn missing_capability_and_cancel_never_submit_a_tile() {
        let (path, journal) = fixture("refusal");
        let gateway = FakeGateway { submissions: AtomicUsize::new(0), change_predecessor: None,
            cancel_after_first: Mutex::new(None) };
        let missing = propose(path.clone(), "chapter".into(), 0, CloudProvider::Modal,
            "modal-prof-1".into(), "Modal".into(), "a".repeat(64), RT.into(),
            vec![], &gateway, &journal);
        assert_eq!(capability(&gateway.capabilities().unwrap(), RT).err().unwrap(),
            AnalysisFlowError::CapabilityUnavailable("gateway does not advertise this analysis model"));
        assert_eq!(missing.unwrap_err(), AnalysisFlowError::CapabilityUnavailable(
            "gateway does not advertise this analysis model").to_string());
        let approved = proposal(path, &journal, &gateway);
        assert!(cancel(&approved.proposal_id).unwrap());
        assert_eq!(require_active(&proposals().lock().unwrap().get(&approved.proposal_id).unwrap().cancel)
            .unwrap_err(), AnalysisFlowError::Cancelled);
        assert_eq!(confirm_and_run(&approved.proposal_id, true, true, &gateway, &journal, |_| {}).err().unwrap(),
            AnalysisFlowError::Cancelled.to_string());
        assert_eq!(gateway.submissions.load(Ordering::SeqCst), 0);
        let record = journal.read(&approved.proposal_id).unwrap();
        assert!(matches!(record.phase, AnalysisPhase::Cancelled));
        assert_eq!(record.tile_submission_started, Some(false));
    }

    #[test]
    fn cancellation_after_first_tile_stops_remaining_batch() {
        let (path, journal) = fixture_with_width("cancel-batch", 1300);
        let gateway = FakeGateway { submissions: AtomicUsize::new(0), change_predecessor: None,
            cancel_after_first: Mutex::new(None) };
        let approved = proposal(path, &journal, &gateway);
        assert_eq!(approved.tiles.len(), 2);
        *gateway.cancel_after_first.lock().unwrap() = Some(approved.proposal_id.clone());
        let error = confirm_and_run(&approved.proposal_id, true, true, &gateway, &journal, |_| {})
            .err().unwrap();
        assert_eq!(error, "analysis_cancelled");
        assert_eq!(gateway.submissions.load(Ordering::SeqCst), 1);
        let record = journal.read(&approved.proposal_id).unwrap();
        assert!(matches!(record.phase, AnalysisPhase::Cancelled));
        assert_eq!(record.completed_tiles, 1);
        assert_eq!(record.tile_submission_started, Some(true));
    }

    #[test]
    fn live_cancel_between_tiles_never_sends_next_tile() {
        let (path, journal) = fixture_with_width("cancel-between-tiles", 1300);
        let gateway = FakeGateway { submissions: AtomicUsize::new(0), change_predecessor: None,
            cancel_after_first: Mutex::new(None) };
        let approved = proposal(path, &journal, &gateway);
        let id = approved.proposal_id.clone();
        let error = confirm_and_run(&id, true, true, &gateway, &journal, |record| {
            if matches!(record.phase, AnalysisPhase::ResultCachedTile { index: 0 }) {
                cancel(&id).unwrap();
            }
        }).err().unwrap();
        assert_eq!(error, AnalysisFlowError::Cancelled.to_string());
        assert_eq!(gateway.submissions.load(Ordering::SeqCst), 1);
        let record = journal.read(&id).unwrap();
        assert!(matches!(record.phase, AnalysisPhase::Cancelled));
        assert!(record.cancel_requested);
        assert_eq!(record.completed_tiles, 1);
    }

    #[test]
    fn cancel_before_submitted_mark_prevents_dispatch() {
        let (path, journal) = fixture_with_width("cancel-before-mark", 1300);
        let gateway = FakeGateway { submissions: AtomicUsize::new(0), change_predecessor: None,
            cancel_after_first: Mutex::new(None) };
        let approved = proposal(path, &journal, &gateway);
        let id = approved.proposal_id.clone();
        let reached = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        proposals().lock().unwrap().get_mut(&id).unwrap().before_submit = Some(Arc::new({
            let reached = Arc::clone(&reached);
            let resume = Arc::clone(&resume);
            move || { reached.wait(); resume.wait(); }
        }));
        std::thread::scope(|scope| {
            let run = scope.spawn(|| confirm_and_run(&id, true, true, &gateway, &journal, |_| {}));
            reached.wait();
            assert!(cancel_with_journal(&id, &journal).unwrap());
            let record = journal.read(&id).unwrap();
            assert!(record.cancel_requested);
            assert!(matches!(record.phase, AnalysisPhase::Confirmed));
            resume.wait();
            assert_eq!(run.join().unwrap().err().unwrap(), "analysis_cancelled");
        });
        assert_eq!(gateway.submissions.load(Ordering::SeqCst), 0);
        let record = journal.read(&id).unwrap();
        assert!(matches!(record.phase, AnalysisPhase::Cancelled));
        assert_eq!(record.tile_submission_started, Some(false));
    }

    #[test]
    fn cancel_after_submitted_mark_reports_one_in_flight_tile() {
        struct BlockingGateway<'a> {
            inner: &'a FakeGateway,
            entered: Arc<std::sync::Barrier>,
            resume: Arc<std::sync::Barrier>,
        }
        impl AnalysisGateway for BlockingGateway<'_> {
            fn capabilities(&self) -> Result<AnalysisCapabilities, String> { self.inner.capabilities() }
            fn analyze(&self, request: &AnalysisRequest, png: &[u8]) -> Result<AnalysisResult, String> {
                self.entered.wait();
                self.resume.wait();
                self.inner.analyze(request, png)
            }
        }
        let (path, journal) = fixture_with_width("cancel-after-mark", 1300);
        let inner = FakeGateway { submissions: AtomicUsize::new(0), change_predecessor: None,
            cancel_after_first: Mutex::new(None) };
        let approved = proposal(path, &journal, &inner);
        assert_eq!(approved.tiles.len(), 2);
        let id = approved.proposal_id.clone();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        let gateway = BlockingGateway { inner: &inner, entered: Arc::clone(&entered), resume: Arc::clone(&resume) };
        std::thread::scope(|scope| {
            let run = scope.spawn(|| confirm_and_run(&id, true, true, &gateway, &journal, |_| {}));
            entered.wait();
            assert!(cancel_with_journal(&id, &journal).unwrap());
            let record = journal.read(&id).unwrap();
            assert!(record.cancel_requested);
            assert!(matches!(record.phase, AnalysisPhase::SubmittedTile { index: 0 }));
            assert_eq!(record.completed_tiles, 0);
            resume.wait();
            assert_eq!(run.join().unwrap().err().unwrap(), "analysis_cancelled");
        });
        assert_eq!(inner.submissions.load(Ordering::SeqCst), 1);
        let record = journal.read(&id).unwrap();
        assert!(record.cancel_requested);
        assert!(matches!(record.phase, AnalysisPhase::Cancelled));
        assert_eq!(record.completed_tiles, 1);
        assert_eq!(record.tile_submission_started, Some(true));
    }

    #[test]
    fn cancelling_active_proposed_proposal_persists_cancelled() {
        let (path, journal) = fixture("cancel-proposed");
        let gateway = FakeGateway { submissions: AtomicUsize::new(0), change_predecessor: None,
            cancel_after_first: Mutex::new(None) };
        let approved = proposal(path, &journal, &gateway);
        assert!(cancel_with_journal(&approved.proposal_id, &journal).unwrap());
        let record = journal.read(&approved.proposal_id).unwrap();
        assert!(record.cancel_requested);
        assert!(matches!(record.phase, AnalysisPhase::Cancelled));
        assert_eq!(record.tile_submission_started, Some(false));
        assert_eq!(gateway.submissions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn recovered_submitted_tile_discard_keeps_remote_outcome_unknown() {
        let (path, journal) = fixture("recovered-cancel");
        let gateway = FakeGateway { submissions: AtomicUsize::new(0), change_predecessor: None,
            cancel_after_first: Mutex::new(None) };
        let approved = proposal(path, &journal, &gateway);
        let mut record = journal.read(&approved.proposal_id).unwrap();
        record.phase = AnalysisPhase::SubmittedTile { index: 0 };
        journal.write(&record).unwrap();
        proposals().lock().unwrap().remove(&approved.proposal_id);

        assert!(cancel_with_journal(&approved.proposal_id, &journal).unwrap());
        let persisted = journal.read(&approved.proposal_id).unwrap();
        assert!(persisted.cancel_requested);
        assert!(matches!(persisted.phase, AnalysisPhase::UnknownRemoteState { index: 0 }));
        assert!(matches!(journal.recover(&approved.proposal_id).unwrap().phase,
            AnalysisPhase::UnknownRemoteState { index: 0 }));
        assert_eq!(gateway.submissions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stitch_places_distinct_masks_on_partial_edge_tile() {
        fn tile_result(rect: TileRect, set: &[(u32, u32)]) -> AnalysisResult {
            let mut pixels = vec![0u8; (rect.width * rect.height) as usize];
            for &(x, y) in set { pixels[(y * rect.width + x) as usize] = 255; }
            let mut png_bytes = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut png_bytes, rect.width, rect.height);
                encoder.set_color(png::ColorType::Grayscale);
                encoder.set_depth(png::BitDepth::Eight);
                encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
            }
            AnalysisResult {
                protocol_version: VERSION.into(), capability: SAM.into(),
                request_digest: "a".repeat(64), tile_id: "tile_0".into(), tile_rect: rect,
                graph_sha256s: vec!["b".repeat(64), "c".repeat(64)],
                model_revision: "d".repeat(40),
                mask_png_b64: Some(base64::engine::general_purpose::STANDARD.encode(png_bytes)),
                components: vec![], boxes: vec![],
                timings: AnalysisTimings { load_ms: 0, preprocess_ms: 0, inference_ms: 0, postprocess_ms: 0 },
                reported_cost_usd: None,
            }
        }

        let mut mask = Some(vec![0u8; 5 * 3]);
        let mut boxes = Vec::new();
        stitch(&tile_result(TileRect { x: 0, y: 0, width: 3, height: 3 },
            &[(0, 0), (2, 2)]), &mut mask, &mut boxes, 5, 3).unwrap();
        stitch(&tile_result(TileRect { x: 3, y: 0, width: 2, height: 3 },
            &[(0, 1), (1, 2)]), &mut mask, &mut boxes, 5, 3).unwrap();
        assert_eq!(mask.as_ref().unwrap().as_slice(), &[
            255, 0, 0, 0, 0,
            0, 0, 0, 255, 0,
            0, 0, 255, 0, 255,
        ]);
        assert_eq!(mask.as_ref().unwrap().len(), 15);
        assert!(stitch(&tile_result(TileRect { x: 4, y: 0, width: 2, height: 3 },
            &[(1, 0)]), &mut mask, &mut boxes, 5, 3).is_err());
        assert!(boxes.is_empty());
    }

    #[test]
    fn expired_proposal_cannot_mint_an_analysis_grant() {
        let (path, journal) = fixture("expired");
        let gateway = FakeGateway { submissions: AtomicUsize::new(0), change_predecessor: None,
            cancel_after_first: Mutex::new(None) };
        let approved = proposal(path, &journal, &gateway);
        proposals().lock().unwrap().get_mut(&approved.proposal_id).unwrap()
            .proposal.expires_at_ms = 0;
        let error = confirm_and_run(&approved.proposal_id, true, true, &gateway, &journal, |_| {}).err().unwrap();
        assert!(matches!(error.as_str(), "analysis_proposal_expired" | "analysis_proposal_missing"));
        assert_eq!(gateway.submissions.load(Ordering::SeqCst), 0);
    }
}
