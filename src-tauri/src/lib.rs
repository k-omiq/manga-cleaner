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
mod library;
mod models;
mod region;
pub mod run;
mod settings;
mod tile;
mod weights;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
        .invoke_handler(tauri::generate_handler![
            diagnostics::diagnostics,
            about::about,
            settings::read_settings,
            settings::write_settings,
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
            weights::list_models,
            weights::download_model,
            weights::cancel_download,
            weights::delete_model,
            weights::discard_partial,
            weights::verify_model,
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
        .run(tauri::generate_context!())
        .expect("error while running the application");
}
