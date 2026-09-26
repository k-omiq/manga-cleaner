//! The Tauri adapter.
//!
//! One process: the webview talks to
//! this over commands and a channel, and everything it asks for is answered by
//! `cleaner-core`. Nothing here holds pixels - pixels go to the browser over the
//! `tile://` protocol, and compositing and export happen in Rust, which is what
//! makes color fidelity enforceable rather than aspirational.

mod about;
mod diagnostics;
mod events;
mod exporting;
mod history;
pub mod inference;
mod library;
mod models;
mod model_workflows;
pub mod provision;
mod region;
pub mod run;
mod settings;
mod tile;
mod underlay;
mod weights;

/// The tray icon's id, so close-to-tray can check the icon exists.
const TRAY_ID: &str = "main";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;
    tauri::Builder::default()
        // First, so a second launch while the window is hidden in the tray
        // shows this one instead of starting another copy.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .setup(|app| {
            use tauri::{
                menu::{Menu, MenuItem},
                tray::TrayIconBuilder,
            };
            let show = MenuItem::with_id(app, "show", "Show Manga Cleaner", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Manga Cleaner", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let tray = TrayIconBuilder::with_id(TRAY_ID)
                .menu(&menu)
                .tooltip("Manga Cleaner")
                // A menu-bar icon opens its menu on click on macOS. Elsewhere a
                // left click brings the window back and the menu is on the
                // right button. Linux tray hosts do not report clicks, so there
                // the menu is the only route, which it always offers.
                .show_menu_on_left_click(cfg!(target_os = "macos"))
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if !cfg!(target_os = "macos") {
                            show_main(tray.app_handle());
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                });
            // The macOS menu bar wants a template image, black plus alpha,
            // which the system tints to match a light or dark menu bar; the
            // full-colour app icon would not follow it. The asset and its
            // generator live in `assets/tray/`. Other platforms keep the app
            // icon.
            #[cfg(target_os = "macos")]
            let tray = tray
                .icon(tauri::include_image!("assets/tray/tray-template@2x.png"))
                .icon_as_template(true);
            #[cfg(not(target_os = "macos"))]
            let tray = match app.default_window_icon() {
                Some(icon) => tray.icon(icon.clone()),
                None => tray,
            };
            // Not fatal: a Linux desktop without an AppIndicator host has no
            // tray, and the editor works without one. Close-to-tray checks
            // that the icon exists before it hides the window.
            if let Err(error) = tray.build(app) {
                eprintln!("tray icon unavailable: {error}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Hiding the only window with no tray icon to bring it back
                // would leave the app running with no way to reach it.
                let keep_running = window.app_handle().tray_by_id(TRAY_ID).is_some()
                    && settings::read(window.app_handle())
                        .ok()
                        .and_then(|value| value.get("closeToTray").and_then(|v| v.as_bool()))
                        .unwrap_or(false);
                if keep_running {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        // The one capability the window has beyond `core:default`: a folder
        // chooser. Import cannot be real without it - a project and a chapter
        // each need a source folder, and until this plugin was registered the
        // only way to give one was to type an absolute path from memory. The
        // permission granted in `capabilities/default.json` is `dialog:allow-open`
        // and nothing else: no save dialog, no message boxes, and no filesystem
        // plugin, so the chooser hands back a path and every read of it still
        // goes through this crate's own commands.
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Opens the project page from onboarding in the user's browser. The
        // capability allows that one URL and nothing else.
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            diagnostics::diagnostics,
            about::about,
            settings::read_settings,
            settings::write_settings,
            inference::commands::read_inference_config,
            inference::analysis::list_remote_analysis_capabilities,
            inference::analysis::propose_remote_analysis,
            inference::analysis::confirm_remote_analysis,
            inference::analysis::cancel_remote_analysis,
            inference::analysis::get_remote_analysis_status,
            inference::commands::write_inference_config,
            inference::commands::store_cloud_secret,
            inference::commands::delete_cloud_secret,
            inference::commands::get_cloud_secret_summary,
            inference::commands::check_cloud_connection,
            inference::commands::get_cloud_model_info,
            inference::commands::prepare_cloud_consent,
            inference::commands::confirm_cloud_consent,
            inference::commands::submit_cloud_attempt,
            inference::commands::get_cloud_attempt_status,
            inference::commands::get_cloud_attempt_result,
            inference::commands::cancel_cloud_attempt,
            inference::commands::reconcile_cloud_recovery,
            provision::run_cloud_provisioner,
            provision::provision_inspect,
            provision::provision_plan,
            provision::provision_apply,
            provision::provision_resume,
            provision::provision_cleanup,
            provision::provision_probe,
            provision::cancel_cloud_provisioner,
            library::list_projects,
            library::create_project,
            library::create_chapter,
            library::open_chapter,
            library::load_pages,
            history::history_load,
            history::history_push,
            history::history_move,
            library::rename_project,
            library::delete_project,
            library::delete_chapter,
            library::delete_mask,
            library::restore_region,
            library::keep_dependency_result,
            region::apply_tool,
            region::create_region,
            region::rerun_mask,
            region::clean_anyway,
            region::sidecar_available,
            region::list_sidecar_models,
            exporting::export_chapter,
            events::subscribe_events,
            events::unsubscribe_events,
            models::list_loaded_models,
            models::unload_model,
            models::list_accelerators,
            model_workflows::list_workflow_capabilities,
            model_workflows::import_full_rt,
            model_workflows::remove_full_rt,
            model_workflows::import_sam_ts,
            model_workflows::remove_sam_ts,
            model_workflows::verify_sam_ts,
            model_workflows::analyze_capabilities,
            model_workflows::analyze_chapter_page,
            model_workflows::cancel_capability_analysis,
            model_workflows::load_component_correction,
            model_workflows::prepare_component_write,
            model_workflows::apply_component_write,
            weights::list_models,
            weights::download_model,
            weights::download_model_group,
            weights::cancel_download,
            weights::delete_model,
            weights::delete_model_group,
            weights::discard_partial,
            weights::verify_model,
            weights::verify_model_group,
            weights::download_runtime,
            weights::delete_runtime,
            run::run_clean,
            run::cancel_run,
            run::resume_job,
        ])
        // Pixels do not cross the command boundary. `tile.rs` resolves a chapter
        // to its job through the library and answers with whole PNG responses -
        // never a range, because WebView2 supports neither ranges nor streaming.
        .register_uri_scheme_protocol(
            tile::SCHEME,
            tile::protocol(|app, chapter_id| {
                library::resolve_chapter(app, chapter_id).map_err(|error| error.to_string())
            }),
        )
        .build(tauri::generate_context!())
        .expect("error while building the application")
        .run(|app, event| {
            // A Dock click on macOS with the window hidden in the tray.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = app;
            // The provisioner helper leads its own process group, so nothing
            // ends it when this process ends. A quit during cloud setup stops
            // it here rather than leaving it deploying with no one to read the
            // result; its journal makes a later resume safe.
            if let tauri::RunEvent::Exit = event {
                provision::stop_helpers_for_exit();
            }
        });
}

/// Bring the main window back from the tray, the Dock, or a second launch,
/// including when it was minimised before it was hidden.
fn show_main<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
