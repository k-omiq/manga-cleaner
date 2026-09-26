//! Read-only usage totals from durable attempts, never from transient UI events.
//! Missing provider prices remain unknown; this is not a provider invoice.
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use serde::Serialize;
use super::journal::{AnalysisAttemptRecord, AnalysisPhase, AttemptPhase};

static SESSION_START: OnceLock<u64> = OnceLock::new();
fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}
pub fn start_session() { SESSION_START.get_or_init(now_ms); }

#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePeriod {
    pub attempts: u64,
    pub reported_usd: f64,
    pub unpriced_attempts: u64,
}
impl UsagePeriod {
    fn add(&mut self, cost: Option<f64>) {
        self.attempts += 1;
        match cost.filter(|value| value.is_finite() && *value >= 0.0) {
            Some(value) => self.reported_usd += value,
            None => self.unpriced_attempts += 1,
        }
    }
}
#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudUsage {
    pub month: UsagePeriod,
    pub session: UsagePeriod,
    pub unreadable_attempts: u64,
}
impl CloudUsage {
    fn add(&mut self, at: u64, cost: Option<f64>, month: u64, session: u64, now: u64) {
        if at > now { return; }
        if at >= month { self.month.add(cost); }
        if at >= session { self.session.add(cost); }
    }
}

#[tauri::command]
pub async fn get_cloud_usage(app: tauri::AppHandle, month_start_ms: u64) -> Result<CloudUsage, String> {
    let root = super::commands::get_app_journal_dir(&app)?;
    let session = *SESSION_START.get_or_init(now_ms);
    tauri::async_runtime::spawn_blocking(move || summarize(&root, month_start_ms, session, now_ms()))
        .await.map_err(|e| e.to_string())?
}

fn summarize(root: &std::path::Path, month: u64, session: u64, now: u64) -> Result<CloudUsage, String> {
    if month > now || now.saturating_sub(month) > 32 * 24 * 60 * 60 * 1000 {
        return Err("invalid usage month".into());
    }
    let journal = super::journal::AttemptJournal::new(root, cleaner_core::cloud_wire::provisional_fixture_limits());
    let mut usage = CloudUsage::default();
    for id in journal.list_attempt_ids().map_err(|e| e.to_string())? {
        let record = match journal.get_record(&id) {
            Ok(record) => record,
            Err(_) => { usage.unreadable_attempts += 1; continue; }
        };
        let cost = match record.phase {
            AttemptPhase::Intent => continue,
            AttemptPhase::Cancelled { handle: None, .. } | AttemptPhase::Failed { handle: None, .. } => continue,
            AttemptPhase::ResultCached { reported_cost_usd, .. }
            | AttemptPhase::AttachmentPending { reported_cost_usd, .. }
            | AttemptPhase::Committed { reported_cost_usd, .. } => reported_cost_usd,
            _ => None,
        };
        usage.add(record.created_at_ms, cost, month, session, now);
    }
    let analysis_dir = root.join("analysis");
    if analysis_dir.exists() {
        for entry in std::fs::read_dir(analysis_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.ends_with(".json") || name.ends_with(".result.json") { continue; }
            use std::io::Read;
            let read = std::fs::File::open(entry.path()).and_then(|file| {
                let mut data = Vec::new();
                file.take(1024 * 1024).read_to_end(&mut data)?;
                Ok(data)
            });
            let record = read.ok().and_then(|data| serde_json::from_slice::<AnalysisAttemptRecord>(&data).ok());
            let Some(record) = record else { usage.unreadable_attempts += 1; continue; };
            if matches!(record.phase, AnalysisPhase::Proposed | AnalysisPhase::Confirmed) { continue; }
            // A new record says explicitly whether dispatch was reached. Older
            // records lack that bit, so terminal ambiguity stays unpriced
            // rather than being presented as free.
            if record.tile_submission_started == Some(false) { continue; }
            if record.created_at_ms == 0 { usage.unreadable_attempts += 1; continue; }
            usage.add(record.created_at_ms, record.reported_cost_usd, month, session, now);
        }
    }
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::journal::{AnalysisJournal, ANALYSIS_JOURNAL_SCHEMA_VERSION};
    use cleaner_core::engines::render::CloudProvider;
    #[test]
    fn periods_keep_unknown_prices_and_do_not_count_old_sessions() {
        let mut usage = CloudUsage::default();
        usage.add(110, Some(0.2), 100, 150, 200);
        usage.add(160, None, 100, 150, 200);
        usage.add(170, Some(0.0), 100, 150, 200);
        usage.add(99, Some(10.0), 100, 150, 200);
        usage.add(201, Some(10.0), 100, 150, 200);
        assert_eq!(usage.month.attempts, 3);
        assert_eq!(usage.month.reported_usd, 0.2);
        assert_eq!(usage.month.unpriced_attempts, 1);
        assert_eq!(usage.session.attempts, 2);
        assert_eq!(usage.session.reported_usd, 0.0);
        assert_eq!(usage.session.unpriced_attempts, 1);
    }
    #[test]
    fn invalid_prices_are_unknown_not_free() {
        let mut period = UsagePeriod::default();
        for cost in [None, Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] { period.add(cost); }
        assert_eq!(period.unpriced_attempts, 4);
        assert_eq!(period.reported_usd, 0.0);
    }

    #[test]
    fn cancelled_analysis_counts_only_possible_provider_dispatches() {
        let now = now_ms();
        let root = std::env::temp_dir().join(format!("mc-usage-analysis-{}-{now}", std::process::id()));
        let journal = AnalysisJournal::new(root.clone());
        let base = AnalysisAttemptRecord {
            created_at_ms: now - 100,
            schema_version: ANALYSIS_JOURNAL_SCHEMA_VERSION,
            proposal_id: "before".into(), provider: CloudProvider::Modal,
            profile_id: "modal".into(), capability: cleaner_core::cloud_analysis_wire::SAM.into(),
            source_sha256: "a".repeat(64), underlay_sha256: "b".repeat(64),
            total_tiles: 1, completed_tiles: 0, reported_cost_usd: None,
            tile_submission_started: Some(false), cancel_requested: true,
            phase: AnalysisPhase::Cancelled,
        };
        journal.write(&base).unwrap();
        let mut before_failed = base.clone();
        before_failed.proposal_id = "before_failed".into();
        before_failed.phase = AnalysisPhase::Failed { code: "cancelled_before_dispatch".into() };
        journal.write(&before_failed).unwrap();
        let mut unknown_without_dispatch = base.clone();
        unknown_without_dispatch.proposal_id = "unknown_without_dispatch".into();
        unknown_without_dispatch.phase = AnalysisPhase::UnknownRemoteState { index: 0 };
        journal.write(&unknown_without_dispatch).unwrap();
        let mut after = base.clone();
        after.proposal_id = "after".into();
        after.tile_submission_started = Some(true);
        journal.write(&after).unwrap();
        let mut legacy = base.clone();
        legacy.proposal_id = "legacy".into();
        legacy.tile_submission_started = None;
        journal.write(&legacy).unwrap();
        let totals = summarize(&root, now - 1000, now - 1000, now).unwrap();
        assert_eq!(totals.month.attempts, 2);
        assert_eq!(totals.month.unpriced_attempts, 2);
        assert_eq!(totals.session.attempts, 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
