//! Finding and loading ONNX Runtime.
//!
//! The runtime is not in the bundle: a small signed application downloads it
//! on first launch and `dlopen`s it. Phase 0 spike 1 measured what that
//! costs and what breaks it, and both findings live here rather than in a
//! comment on the download.
//!
//! Two of them shape this module:
//!
//! - The application must carry `com.apple.security.cs.disable-library-validation`,
//!   because every official ONNX Runtime release is ad-hoc signed with no Team
//!   Identifier and a hardened-runtime process refuses to map it otherwise.
//! - Which archive a platform needs, and which execution providers it turns out
//!   to carry, is [`package`] - they are not the same across platforms and the
//!   difference decides whether the inpainter can leave the CPU.
//! - A runtime carrying `com.apple.quarantine` is refused by policy, and the
//!   operating system's message for that is nothing like the message for a
//!   missing file. [`LoadError`] separates the two so the interface can say
//!   which one happened.
//!
//! ## The WebGPU plugin
//!
//! From 1.28 ONNX Runtime ships its WebGPU provider as a **plugin library**
//! registered at run time - `Microsoft.ML.OnnxRuntime.EP.WebGpu`, one
//! `onnxruntime_providers_webgpu` per platform - rather than compiling it into
//! every general release. The macOS archive still has it built in; the
//! Windows and Linux ones do not, and the plugin is what gives them the one
//! cross-vendor provider with a `DFT` kernel ([`crate::accel`]). [`package`]
//! puts it beside the runtime on those platforms, and [`load`] registers it
//! when it is there: [`WEBGPU_PLUGIN_NAME`] in the runtime's own directory,
//! registered once, and reported through [`webgpu_plugin`] rather than
//! failing the load - a runtime whose plugin will not register is still a
//! runtime with a CPU in it.

pub mod package;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The file name the runtime must keep. On macOS the dylib's own
/// `LC_LOAD_DYLIB` names `@rpath/libonnxruntime.1.dylib`, so a copy saved under
/// any other name cannot resolve its self-reference - and fails in a way that
/// reads exactly like a signing failure.
pub const DYLIB_NAME: &str = if cfg!(target_os = "windows") {
    "onnxruntime.dll"
} else if cfg!(target_os = "macos") {
    "libonnxruntime.dylib"
} else {
    "libonnxruntime.so"
};

/// The WebGPU plugin provider's library, as the package unpacks it beside
/// [`DYLIB_NAME`]. Present on Windows and Linux x64, where it is downloaded;
/// absent on macOS, where the provider is built into the runtime.
pub const WEBGPU_PLUGIN_NAME: &str = if cfg!(target_os = "windows") {
    "onnxruntime_providers_webgpu.dll"
} else if cfg!(target_os = "macos") {
    "libonnxruntime_providers_webgpu.dylib"
} else {
    "libonnxruntime_providers_webgpu.so"
};

/// The name the plugin is registered under. ONNX Runtime wants one per
/// library and does not read it; the provider's own name is what
/// [`crate::accel`] selects on.
const WEBGPU_PLUGIN_REGISTRATION: &str = "webgpu";

/// The ONNX Runtime release the application ships against. `ort` accepts
/// anything from its compile-time floor (the `api-NN` feature) upwards, so a
/// user with a newer runtime already installed is not forced to re-download.
pub const PINNED_VERSION: &str = "1.28.0";

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("no ONNX Runtime found; looked in: {}", .searched.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))]
    NotFound { searched: Vec<PathBuf> },

    /// The file is there and the operating system refused to map it because it
    /// arrived quarantined. Recoverable without a re-download: clear the
    /// attribute. Kept distinct because the remedy is distinct.
    #[error("the ONNX Runtime at {path} is quarantined and cannot be loaded")]
    Quarantined { path: PathBuf },

    /// The file is there and was refused for a signing reason that clearing
    /// quarantine will not fix - on a hardened-runtime build, most likely the
    /// missing `disable-library-validation` entitlement.
    #[error("the ONNX Runtime at {path} was refused by the system: {detail}")]
    Refused { path: PathBuf, detail: String },

    #[error("the ONNX Runtime at {path} could not be loaded: {detail}")]
    Failed { path: PathBuf, detail: String },
}

/// Where to look, in order. `app_data` is the per-user directory a first-launch
/// download writes to; the caller supplies it because only the shell knows it.
///
/// `ORT_DYLIB_PATH` comes first so a developer can point at a build without
/// touching the installed copy - the same variable `ort` itself honours.
pub fn search_paths(app_data: Option<&Path>) -> Vec<PathBuf> {
    let explicit = std::env::var("ORT_DYLIB_PATH").ok();
    search_paths_from(explicit.as_deref(), app_data)
}

/// The same, with the override passed in rather than read.
///
/// Split out so the order can be tested without mutating the process
/// environment: two tests doing that concurrently is a race, and it produced
/// one - `cargo test` is parallel by default and `set_var` is global.
pub fn search_paths_from(explicit: Option<&str>, app_data: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(explicit) = explicit {
        if !explicit.is_empty() {
            paths.push(PathBuf::from(explicit));
        }
    }

    if let Some(dir) = app_data {
        paths.push(dir.join("runtimes").join(DYLIB_NAME));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Beside the executable, and - on macOS - the bundle's Resources,
            // which is `Contents/MacOS/../Resources`.
            paths.push(dir.join("runtimes").join(DYLIB_NAME));
            paths.push(dir.join("../Resources/runtimes").join(DYLIB_NAME));
        }
    }

    paths
}

/// The first search path that exists.
pub fn find(app_data: Option<&Path>) -> Result<PathBuf, LoadError> {
    let searched = search_paths(app_data);
    searched
        .iter()
        .find(|p| p.exists())
        .cloned()
        .ok_or(LoadError::NotFound { searched })
}

/// Load the runtime and commit the `ort` environment. Idempotent: `ort` holds
/// the library in a `OnceLock`, so later calls return the already-loaded one.
///
/// The WebGPU plugin beside it, where there is one, is registered on the
/// first successful load and never again - see [`webgpu_plugin`] for what
/// became of it.
pub fn load(path: &Path) -> Result<(), LoadError> {
    if let Some(dir) = path.parent() {
        add_library_directory(dir);
    }
    ort::init_from(path)
        .map_err(|e| {
            // `ort` and `libloading` both flatten `dlerror` to "dlopen failed",
            // so the text that distinguishes a quarantined runtime from an
            // unsigned one is only available by asking `dlerror` again.
            let detail = dlerror_for(path).unwrap_or_else(|| e.to_string());
            classify(path, &detail)
        })?
        .with_name("manga-cleaner")
        .commit();
    WEBGPU_PLUGIN.get_or_init(|| register_webgpu_plugin(path));
    Ok(())
}

/// What became of the WebGPU plugin beside the loaded runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plugin {
    /// No plugin library beside the runtime. macOS, whose archive has WebGPU
    /// built in, and Linux aarch64, for which none is published.
    Absent,
    /// The runtime already carries a `WebGpuExecutionProvider`, so the library
    /// was left unregistered: two providers of one name is a collision, and
    /// the built-in one is the measured one.
    BuiltIn,
    /// Registered; [`crate::accel`] can select it.
    Registered,
    /// Present and refused, with ONNX Runtime's reason - an ABI older than
    /// the plugin wants, or a library that would not open.
    Failed(String),
}

static WEBGPU_PLUGIN: OnceLock<Plugin> = OnceLock::new();

/// The plugin's state after the first [`load`]. [`Plugin::Absent`] before any
/// load, which is also what a machine with no plugin reads.
pub fn webgpu_plugin() -> Plugin {
    WEBGPU_PLUGIN.get().cloned().unwrap_or(Plugin::Absent)
}

fn register_webgpu_plugin(runtime: &Path) -> Plugin {
    use ort::ep::ExecutionProvider;

    let Some(dir) = runtime.parent() else { return Plugin::Absent };
    let plugin = dir.join(WEBGPU_PLUGIN_NAME);
    if !plugin.exists() {
        return Plugin::Absent;
    }
    if ort::ep::webgpu::WebGPU::default().is_available().unwrap_or(false) {
        return Plugin::BuiltIn;
    }
    let registered = ort::environment::Environment::current()
        .and_then(|env| env.register_ep_library(WEBGPU_PLUGIN_REGISTRATION, &plugin));
    match registered {
        // The handle only exists to unregister with, and nothing ever does:
        // the plugin lives as long as the process, like the runtime it is in.
        Ok(_handle) => Plugin::Registered,
        Err(e) => Plugin::Failed(e.to_string()),
    }
}

/// Put the runtime's directory on the loader's search path.
///
/// Windows only, and for the libraries the runtime loads *by name* rather
/// than the one this module opens by path: `libloading` finds `DirectML.dll`
/// beside `onnxruntime.dll` because it opens that file with
/// `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR`, but the WebGPU plugin is opened by
/// ONNX Runtime itself, and `dxcompiler.dll` and `dxil.dll` by the plugin, and
/// neither of those searches the directory it came from. `SetDllDirectoryW`
/// adds one directory to every subsequent `LoadLibrary` in the process, which
/// is the one the download unpacked into. Unix loaders resolve a library's
/// own dependencies through `ldconfig` and `LD_LIBRARY_PATH`, which is where
/// `libvulkan.so.1` lives, so nothing is needed there.
#[cfg(windows)]
fn add_library_directory(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // A failure here is a directory that will be searched the ordinary way,
    // which is the state every earlier build was in; nothing to report.
    unsafe {
        windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW(wide.as_ptr());
    }
}

#[cfg(not(windows))]
fn add_library_directory(_dir: &Path) {}

/// Attach a remedy to the loader's message.
fn classify(path: &Path, detail: &str) -> LoadError {
    let detail = detail.to_owned();
    if detail.contains("disallowed by system policy") {
        LoadError::Quarantined { path: path.to_path_buf() }
    } else if detail.contains("Team ID") || detail.contains("code signature") {
        LoadError::Refused { path: path.to_path_buf(), detail }
    } else {
        LoadError::Failed { path: path.to_path_buf(), detail }
    }
}

#[cfg(unix)]
fn dlerror_for(path: &Path) -> Option<String> {
    use std::ffi::{CStr, CString};

    const RTLD_LAZY: std::os::raw::c_int = 0x1;
    const RTLD_LOCAL: std::os::raw::c_int = 0x4;
    unsafe extern "C" {
        fn dlopen(
            filename: *const std::os::raw::c_char,
            flag: std::os::raw::c_int,
        ) -> *mut std::os::raw::c_void;
        fn dlerror() -> *const std::os::raw::c_char;
    }

    let c_path = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    unsafe {
        dlerror();
        if !dlopen(c_path.as_ptr(), RTLD_LAZY | RTLD_LOCAL).is_null() {
            return None;
        }
        let err = dlerror();
        if err.is_null() {
            None
        } else {
            Some(CStr::from_ptr(err).to_string_lossy().into_owned())
        }
    }
}

#[cfg(not(unix))]
fn dlerror_for(_path: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_override_is_searched_first() {
        let paths = search_paths_from(
            Some("/nowhere/libonnxruntime.dylib"),
            Some(Path::new("/app-data")),
        );
        assert_eq!(paths[0], PathBuf::from("/nowhere/libonnxruntime.dylib"));
        assert_eq!(paths[1], PathBuf::from("/app-data/runtimes").join(DYLIB_NAME));

        // An empty override is not an override.
        let without = search_paths_from(Some(""), Some(Path::new("/app-data")));
        assert_eq!(without[0], PathBuf::from("/app-data/runtimes").join(DYLIB_NAME));
    }

    /// Through `search_paths_from` rather than `find`, for exactly the reason
    /// that function was split out: `find` reads `ORT_DYLIB_PATH`, and a
    /// developer who has exported it - which the run instructions tell them
    /// to - has a runtime that is *not* absent. The test was then asserting on
    /// the shell it happened to be run from, and failed in one.
    #[test]
    fn an_absent_runtime_names_everywhere_it_looked() {
        let searched = search_paths_from(None, Some(Path::new("/definitely/not/here")));
        let message = LoadError::NotFound { searched }.to_string();
        assert!(message.contains("/definitely/not/here"), "{message}");
    }

    /// The one test that touches a real runtime. It is skipped, loudly, when
    /// `scripts/fetch-runtime.sh` has not been run - a developer without the
    /// download should not see a red test, and a developer with one should not
    /// have the load go unexercised.
    #[test]
    fn an_installed_runtime_loads_and_reports_itself() {
        let dev_copy = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../runtimes/onnxruntime-osx-arm64-1.28.0/lib")
            .join(DYLIB_NAME);
        if !dev_copy.exists() {
            eprintln!("skipped: no runtime at {} - run scripts/fetch-runtime.sh", dev_copy.display());
            return;
        }
        load(&dev_copy).expect("the installed runtime loads");
        assert!(ort::info().contains("ORT Build Info"), "{}", ort::info());
    }

    #[test]
    fn the_quarantine_refusal_is_told_apart_from_a_signing_refusal() {
        let path = Path::new("/nonexistent/libonnxruntime.dylib");
        assert!(matches!(
            classify(path, "library load disallowed by system policy"),
            LoadError::Quarantined { .. }
        ));
        assert!(matches!(
            classify(path, "have different Team IDs"),
            LoadError::Refused { .. }
        ));
        assert!(matches!(classify(path, "no such file"), LoadError::Failed { .. }));
    }
}
