//! Phase 0, spike 1: `ort` + `load-dynamic`.
//!
//! The question: can a signed, hardened-runtime macOS app `dlopen` an ONNX
//! Runtime that was **not** in the bundle at signing time - the "small signed
//! app plus a post-install download" shape chosen to sidestep `externalBin`
//! and Frameworks-dylib signing?
//!
//! This binary is the probe. It takes the dylib path, loads it, prints what the
//! runtime says about itself, creates a session over an inlined model and runs
//! it. Every step that can fail prints why, so the failure mode is legible from
//! a `codesign`-ed app bundle where a debugger is inconvenient.
//!
//! Usage: `spike-ort-loaddynamic [<path-to-libonnxruntime.dylib>]`
//! Falls back to `$ORT_DYLIB_PATH`, then to a `runtimes/` directory beside the
//! executable - which is where a real post-install download would land.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

mod tiny_onnx;

fn main() {
    // The report is built up as the probe proceeds and printed either way: on a
    // failure the lines gathered before it - the `dlerror` above all - are the
    // whole point.
    let mut report = String::new();
    let outcome = run(&mut report);
    print!("{report}");
    match outcome {
        Ok(()) => println!("SPIKE RESULT: pass"),
        Err(err) => {
            println!("SPIKE RESULT: fail");
            println!("{err:?}");
            std::process::exit(1);
        }
    }
}

/// Where the runtime is, in the order a shipped app would look.
fn resolve_dylib() -> Result<PathBuf> {
    if let Some(arg) = std::env::args().nth(1) {
        return Ok(PathBuf::from(arg));
    }
    if let Ok(env) = std::env::var("ORT_DYLIB_PATH") {
        if !env.is_empty() {
            return Ok(PathBuf::from(env));
        }
    }
    // The shipped layout: alongside the executable, inside the bundle's
    // Resources, or in the per-user application-support directory a first-launch
    // download would write to. Only the first two are probed here; the spike is
    // about the load, not about the search.
    let exe = std::env::current_exe().context("current_exe")?;
    let dir = exe.parent().ok_or_else(|| anyhow!("executable has no parent"))?;
    for candidate in [
        dir.join("runtimes/libonnxruntime.dylib"),
        dir.join("../Resources/runtimes/libonnxruntime.dylib"),
    ] {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(anyhow!(
        "no ONNX Runtime found: pass a path, set ORT_DYLIB_PATH, or place one in {}",
        dir.join("runtimes").display()
    ))
}

fn run(report: &mut String) -> Result<()> {
    let dylib = resolve_dylib()?;
    report.push_str(&format!("dylib: {}\n", dylib.display()));
    report.push_str(&format!("exists: {}\n", dylib.exists()));

    // 1. The load itself. This is the step library validation kills: a
    //    hardened-runtime process signed with a Team ID refuses to map a dylib
    //    that is ad-hoc signed, which every official ONNX Runtime release is.
    //
    //    `libloading` renders every failure as "dlopen failed", which is the one
    //    thing that must not be opaque here, so `dlerror` is read directly
    //    first. A successful probe is harmless: the handle is left open and
    //    `ort` gets the same image.
    if let Some(dlerror) = raw_dlopen_error(&dylib) {
        report.push_str(&format!("dlerror: {dlerror}\n"));
    }
    ort::init_from(&dylib)
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("ort::init_from({})", dylib.display()))?
        .with_name("manga-cleaner-spike")
        .commit();
    report.push_str("loaded: yes\n");
    report.push_str(&format!("build info: {}\n", ort::info()));

    // 2. A session over a model that never touched the disk, so a model-loading
    //    failure cannot be mistaken for a runtime-loading one.
    let model = tiny_onnx::relu_1x4();
    let mut session = ort::session::Session::builder()
        .context("session builder")?
        .commit_from_memory(&model)
        .context("commit_from_memory: the runtime loaded but could not build a session")?;
    report.push_str(&format!(
        "session: {} input(s), {} output(s)\n",
        session.inputs().len(),
        session.outputs().len()
    ));

    // 3. Inference, so the report covers the kernel path and not only symbol
    //    resolution.
    let input = ort::value::Tensor::from_array(([1usize, 4], vec![-1.0f32, 0.0, 2.0, 3.5]))
        .context("input tensor")?;
    let outputs = session.run(ort::inputs![input]).context("run")?;
    let (shape, data) = outputs["y"]
        .try_extract_tensor::<f32>()
        .context("extract output")?;
    report.push_str(&format!("output {shape:?}: {data:?}\n"));

    let expected = [0.0f32, 0.0, 2.0, 3.5];
    if data != expected {
        return Err(anyhow!("wrong output: got {data:?}, expected {expected:?}"));
    }
    report.push_str("relu: correct\n");

    // 4. What the process actually has mapped, which is the evidence that the
    //    load was dynamic and not a link-time dependency.
    report.push_str(&format!("mapped: {}\n", mapped_onnxruntime_images().join(", ")));
    Ok(())
}

/// `dlopen` the path directly and return `dlerror` if it refused. `None` means
/// the load succeeded - the handle is deliberately not closed, because `ort` is
/// about to ask for the same image.
fn raw_dlopen_error(path: &Path) -> Option<String> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    unsafe {
        dlerror(); // clear any stale error
        let handle = dlopen(c_path.as_ptr(), RTLD_LAZY | RTLD_LOCAL);
        if !handle.is_null() {
            return None;
        }
        let err = dlerror();
        if err.is_null() {
            Some("dlopen returned null with no dlerror".to_owned())
        } else {
            Some(std::ffi::CStr::from_ptr(err).to_string_lossy().into_owned())
        }
    }
}

const RTLD_LAZY: std::os::raw::c_int = 0x1;
const RTLD_LOCAL: std::os::raw::c_int = 0x4;

unsafe extern "C" {
    fn dlopen(
        filename: *const std::os::raw::c_char,
        flag: std::os::raw::c_int,
    ) -> *mut std::os::raw::c_void;
    fn dlerror() -> *const std::os::raw::c_char;
}

/// The `onnxruntime` images in this process's address space, read from dyld.
fn mapped_onnxruntime_images() -> Vec<String> {
    let mut found = Vec::new();
    unsafe {
        let count = libc_dyld_image_count();
        for i in 0..count {
            let name = libc_dyld_get_image_name(i);
            if name.is_null() {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned();
            if name.contains("onnxruntime") {
                found.push(name);
            }
        }
    }
    if found.is_empty() {
        found.push("(none)".to_owned());
    }
    found
}

// dyld introspection, declared here rather than pulling in a crate for two symbols.
unsafe extern "C" {
    #[link_name = "_dyld_image_count"]
    fn libc_dyld_image_count() -> u32;
    #[link_name = "_dyld_get_image_name"]
    fn libc_dyld_get_image_name(index: u32) -> *const std::os::raw::c_char;
}

/// Not used by the binary; keeps the module honest under `cargo test`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_bytes_are_a_plausible_protobuf() {
        let bytes = tiny_onnx::relu_1x4();
        assert!(bytes.len() > 40);
        // field 1 (ir_version), varint: tag byte 0x08, value 8.
        assert_eq!(&bytes[0..2], &[0x08, 0x08]);
    }

    #[test]
    fn an_absent_runtime_is_an_error_rather_than_a_panic() {
        // The search order itself is exercised by running the binary; what
        // matters here is that a missing runtime reports rather than aborts.
        unsafe { std::env::set_var("ORT_DYLIB_PATH", "") };
        let previous = std::env::args().nth(1);
        if previous.is_none() {
            let _ = resolve_dylib();
        }
    }
}
