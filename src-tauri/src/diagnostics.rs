//! What the application can actually do on this machine, right now.
//!
//! The runtime and the weights are downloaded after install, so "is the
//! inpainter available" is a question with a real answer that changes over a session's
//! life. Phase 0 spike 1 also found two failure modes whose messages are
//! nothing alike - a quarantined runtime and an unsigned one - and the interface
//! can only tell the user which happened if this layer keeps them apart.

use serde::Serialize;

/// Whether a component is usable, and if not, why not. `reason_key` is an i18n
/// key: no English crosses the seam.
#[derive(Debug, Serialize)]
pub struct ComponentStatus {
    pub name: &'static str,
    pub available: bool,
    pub detail: Option<String>,
    pub reason_key: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct Diagnostics {
    pub app_version: &'static str,
    pub components: Vec<ComponentStatus>,
}

#[tauri::command]
pub fn diagnostics(app: tauri::AppHandle) -> Diagnostics {
    Diagnostics {
        app_version: env!("CARGO_PKG_VERSION"),
        components: vec![onnx_runtime(&app)],
    }
}

pub fn onnx_runtime(app: &tauri::AppHandle) -> ComponentStatus {
    use cleaner_core::runtime::{self, LoadError};
    use tauri::Manager;

    let app_data = app.path().app_data_dir().ok();
    let found = match runtime::find(app_data.as_deref()) {
        Ok(path) => path,
        Err(err) => {
            return ComponentStatus {
                name: "onnxruntime",
                available: false,
                detail: Some(err.to_string()),
                reason_key: Some("diagnostics.runtime.missing"),
            };
        }
    };

    match runtime::load(&found) {
        Ok(()) => ComponentStatus {
            name: "onnxruntime",
            available: true,
            detail: Some(ort::info().to_owned()),
            reason_key: None,
        },
        Err(err) => ComponentStatus {
            name: "onnxruntime",
            available: false,
            detail: Some(err.to_string()),
            // The three cases are told apart because their remedies are: clear
            // an extended attribute, re-sign the application, or download the
            // runtime again.
            reason_key: Some(match err {
                LoadError::Quarantined { .. } => "diagnostics.runtime.quarantined",
                LoadError::Refused { .. } => "diagnostics.runtime.refused",
                _ => "diagnostics.runtime.unloadable",
            }),
        },
    }
}
