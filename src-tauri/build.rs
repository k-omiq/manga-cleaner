fn main() {
    // A release app without this sidecar shows Cloud but cannot connect to
    // either provider. Fail the build instead of shipping that broken state.
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
        let config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(manifest.join("tauri.conf.json")).expect("read Tauri config"),
        )
        .expect("parse Tauri config");
        let bundled = config["bundle"]["externalBin"]
            .as_array()
            .is_some_and(|bins| {
                bins.iter()
                    .any(|bin| bin == "binaries/manga-cleaner-provisioner")
            });
        assert!(
            bundled,
            "release build is missing bundle.externalBin for the cloud helper; run .github/scripts/build-cloud-provisioner.py first"
        );
        let suffix = if target.contains("windows") {
            ".exe"
        } else {
            ""
        };
        let helper = manifest
            .join("binaries")
            .join(format!("manga-cleaner-provisioner-{target}{suffix}"));
        assert!(
            helper.is_file(),
            "release build is missing {}. Freeze the cloud helper for this target first",
            helper.display()
        );
        let uv_configured = config["bundle"]["externalBin"]
            .as_array()
            .is_some_and(|bins| bins.iter().any(|bin| bin == "binaries/manga-cleaner-uv"));
        assert!(
            uv_configured,
            "release build is missing bundle.externalBin for managed Python; run .github/scripts/stage-flux-python.py first"
        );
        let uv = manifest
            .join("binaries")
            .join(format!("manga-cleaner-uv-{target}{suffix}"));
        assert!(
            uv.is_file(),
            "release build is missing {}. Stage uv for this target first",
            uv.display()
        );
        println!("cargo:rerun-if-changed={}", helper.display());
        println!("cargo:rerun-if-changed={}", uv.display());
        println!("cargo:rerun-if-changed=tauri.conf.json");
    }
    // `crate::provision` looks for the development sidecar under the name the
    // release script gives it, which ends in the target triple.
    println!(
        "cargo:rustc-env=MC_TARGET_TRIPLE={}",
        std::env::var("TARGET").expect("cargo sets TARGET for build scripts")
    );
    let attributes =
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "diagnostics",
            "about",
            "read_settings",
            "write_settings",
            "read_inference_config",
            "write_inference_config",
            "store_cloud_secret",
            "delete_cloud_secret",
            "get_cloud_secret_summary",
            "check_cloud_connection",
            "get_cloud_model_info",
            "prepare_cloud_consent",
            "confirm_cloud_consent",
            "submit_cloud_attempt",
            "get_cloud_attempt_status",
            "get_cloud_attempt_result",
            "cancel_cloud_attempt",
            "reconcile_cloud_recovery",
            "run_cloud_provisioner",
            "provision_inspect",
            "provision_plan",
            "provision_apply",
            "provision_resume",
            "provision_cleanup",
            "provision_probe",
            "cancel_cloud_provisioner",
            "list_projects",
            "create_project",
            "create_chapter",
            "open_chapter",
            "load_pages",
            "history_load",
            "history_push",
            "history_move",
            "rename_project",
            "delete_project",
            "delete_chapter",
            "delete_mask",
            "restore_region",
            "apply_tool",
            "create_region",
            "rerun_mask",
            "clean_anyway",
            "sidecar_available",
            "list_sidecar_models",
            "install_flux_helper",
            "export_chapter",
            "subscribe_events",
            "unsubscribe_events",
            "list_loaded_models",
            "unload_model",
            "list_accelerators",
            "list_models",
            "download_model",
            "cancel_download",
            "delete_model",
            "discard_partial",
            "verify_model",
            "download_runtime",
            "delete_runtime",
            "run_clean",
            "cancel_run",
            "resume_job",
        ]));
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
