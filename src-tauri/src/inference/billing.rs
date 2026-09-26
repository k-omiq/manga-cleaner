//! Optional read-only Modal workspace billing, with credentials confined to native code.
use cleaner_core::engines::render::CloudProvider;
use serde_json::{json, Value};
use super::secrets::{decode_modal_runtime_secret, SecretKey, SecretManager, SecretRole};

#[tauri::command]
pub async fn get_cloud_billing(app: tauri::AppHandle, profile_id: String, cycle: String) -> Result<Value, String> {
    super::config::validate_profile_id(&profile_id).map_err(|_| "invalid billing profile".to_string())?;
    if cycle.len() != 7 || cycle.as_bytes()[4] != b'-'
        || !cycle.bytes().enumerate().all(|(i, c)| i == 4 || c.is_ascii_digit()) {
        return Err("invalid billing cycle".into());
    }
    crate::library::blocking(move || {
        let config = super::config::read_inference_config(&app)?;
        let profile = config.modal_profiles.get(&profile_id).ok_or("billing profile missing")?;
        let key = SecretKey::new(CloudProvider::Modal, &profile_id, &profile.canonical_origin_fingerprint, SecretRole::Setup);
        let secret = SecretManager::global().get_secret(&key)
            .map_err(|_| "billing credential unavailable")?.ok_or("billing credential missing")?;
        let (token_id, token_secret) = decode_modal_runtime_secret(&secret)
            .map_err(|_| "billing credential needs a Modal API token ID and secret")?;
        let response = crate::provision::run_provisioner(&app, "billing", "modal", Some(json!({
            "cycle": cycle,
            "credentials": { "token_id": token_id, "token_secret": token_secret.expose_str().map_err(|_| "invalid billing credential")? }
        })));
        if response.get("success").and_then(Value::as_bool) != Some(true) {
            return Err("Modal billing unavailable; check API token permissions and connectivity".into());
        }
        let data = response.get("data").ok_or("billing response missing")?;
        // Return only the documented public billing fields, never arbitrary helper data.
        let mut answer = serde_json::Map::new();
        for key in ["metered_cost", "billed_cost"] {
            let value = data.get(key).and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v >= 0.0).ok_or("invalid billing amount")?;
            answer.insert(key.into(), json!(value));
        }
        answer.insert("workspace".into(), json!(data.get("workspace").and_then(Value::as_str).ok_or("billing workspace missing")?));
        answer.insert("cycle".into(), json!(cycle));
        Ok(Value::Object(answer))
    }).await
}
