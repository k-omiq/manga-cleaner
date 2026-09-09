//! `about()` - the GPL-3.0 obligations, and what is actually running.
//!
//! This is an obligation rather than an About box: §6 of the licence requires
//! the written offer wherever the corresponding source is not shipped
//! alongside, and the interface renders it from `about.offer.written` beside
//! these facts.
//!
//! Every value here is **data, not copy** - a licence identifier, a URL, a model
//! name, a provider name - so none of it is translated. The `labelKey` beside
//! it is the part that is.
//!
//! The one fact the mock could not tell the truth about is the runtime: it
//! reported "ONNX Runtime, CPU execution provider" from a fixture. Here it is
//! whatever the process actually loaded, and it says so when nothing has.

use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    pub label_key: &'static str,
    pub value: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct About {
    pub app_version: &'static str,
    pub facts: Vec<Fact>,
}

/// The source address the written offer points at.
const SOURCE_URL: &str = "https://github.com/k-omiq/manga-cleaner";

#[tauri::command]
pub fn about(app: tauri::AppHandle) -> About {
    About {
        app_version: env!("CARGO_PKG_VERSION"),
        facts: vec![
            Fact { label_key: "about.fact.licence", value: "GPL-3.0-or-later".into() },
            Fact { label_key: "about.fact.source", value: SOURCE_URL.into() },
            Fact {
                label_key: "about.fact.detector",
                value: "comic_text_detector (GPL-3.0) · comic-text-and-bubble-detector v4-s int8 \
                        (Apache-2.0) · image-script-identification osd_lstm (Apache-2.0)"
                    .into(),
            },
            Fact {
                label_key: "about.fact.engines",
                value: "lama-manga onnx opset 17 (MIT)".into(),
            },
            Fact {
                label_key: "about.fact.cloud",
                value: "Google, paid tier only, opt-in per request".into(),
            },
            Fact { label_key: "about.fact.runtime", value: runtime_fact(&app) },
        ],
    }
}

/// What is loaded, and what it can do - not what a fixture said.
fn runtime_fact(app: &tauri::AppHandle) -> String {
    use cleaner_core::accel::Accelerator;

    let status = crate::diagnostics::onnx_runtime(app);
    if !status.available {
        // The reason is already an i18n key on the diagnostics side; here the
        // fact is a value, so it says the plain thing and leaves the remedy to
        // the diagnostics row that has a key for it.
        return "ONNX Runtime not loaded".into();
    }

    let providers = cleaner_core::accel::available();
    let names: Vec<&str> = providers.iter().map(|a: &Accelerator| a.ort_name()).collect();
    let version = ort::info();
    if names.is_empty() {
        version.to_owned()
    } else {
        format!("{version} · {}", names.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The interface reads `appVersion` and `labelKey`; the seam is camelCase
    /// everywhere else, and this struct was the one that forgot to say so.
    #[test]
    fn the_about_payload_crosses_the_seam_in_camel_case() {
        let about = About {
            app_version: "0.0.0",
            facts: vec![Fact { label_key: "about.fact.licence", value: "GPL-3.0-only".into() }],
        };
        let json = serde_json::to_string(&about).unwrap();
        assert!(json.contains("\"appVersion\""), "{json}");
        assert!(json.contains("\"labelKey\""), "{json}");
        assert!(!json.contains("app_version") && !json.contains("label_key"), "{json}");
    }
}
