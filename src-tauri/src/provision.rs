//! Desktop bridge from Manga Cleaner to the cloud provisioner helper (`provisioner/`).
//!
//! Protocol guarantees:
//! - Sends exactly one bounded JSON envelope via stdin (< 64 KiB).
//! - Receives exactly one JSON envelope via stdout. Stdout is read to the end,
//!   so a chatty helper never blocks on a full pipe, but only a bounded prefix
//!   is kept.
//! - Secrets (API keys, tokens) travel ONLY via stdin; never via command-line arguments or environment variables.
//! - Safe helper discovery: no shell invocation (`sh`, `bash`, `cmd`), explicit executable paths.
//! - Clear missing-helper error when binary is unavailable, failing closed.
//! - Strict allowlisted operations (`inspect`, `plan`, `apply`, `resume`, `cleanup_plan`, `cleanup_apply`, `forget_credential`, `probe_compatibility`).
//! - Strict allowlisted providers (`modal`, `beam`).
//! - Stderr is read only for progress lines of one exact shape (IC-2), which
//!   become `provision://progress` events built from this file's own strings.
//!   Every other stderr byte is dropped: provider SDKs can print credentials
//!   in tracebacks.
//! - The runtime credential a successful `apply` or `resume` returns (IC-1) is
//!   moved into the OS keyring here. It never reaches the webview or a log.
//! - A running helper can be stopped (`cancel_cloud_provisioner`). The helper's
//!   journal is what makes a later `resume` safe after a stop.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cleaner_core::engines::render::{CloudProvider, ExecutionTarget};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::inference::commands::{
    verify_cloud_profile, write_inference_config_at, CloudConnectionStatus,
};
use crate::inference::config::{self, CloudProfile, InferenceConfig};
use crate::inference::consent::ConsentService;
use crate::inference::secrets::{
    encode_modal_runtime_secret, SecretKey, SecretManager, SecretRole, SecretValue,
};

pub const HELPER_PROTOCOL_VERSION: &str = "1.0.0";
pub const MAX_REQUEST_BYTES: usize = 64 * 1024; // 64 KiB envelope limit
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024; // 2 MiB bounded output limit
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// `apply`, `resume` and `cleanup_apply`. A first `apply` builds the image and
/// pulls about 5.5 GB of weights into the provider's volume.
pub const DEPLOY_TIMEOUT_SECS: u64 = 30 * 60;

// Standardized Protocol Error Codes matching provisioner/protocol.py
pub const ERR_INVALID_VERSION: &str = "ERR_INVALID_PROTOCOL_VERSION";
pub const ERR_UNSUPPORTED_OP: &str = "ERR_UNSUPPORTED_OPERATION";
pub const ERR_UNSUPPORTED_PROVIDER: &str = "ERR_UNSUPPORTED_PROVIDER";
pub const ERR_INVALID_PAYLOAD: &str = "ERR_INVALID_REQUEST_PAYLOAD";
pub const ERR_PAYLOAD_TOO_LARGE: &str = "ERR_PAYLOAD_TOO_LARGE";
pub const ERR_VALIDATION: &str = "ERR_VALIDATION_ERROR";
pub const ERR_SECURITY_VIOLATION: &str = "ERR_SECURITY_VIOLATION";
pub const ERR_PROVIDER_UNAVAILABLE: &str = "ERR_PROVIDER_UNAVAILABLE";
pub const ERR_EXECUTION_FAILED: &str = "ERR_EXECUTION_FAILED";
pub const ERR_EXECUTION_TIMEOUT: &str = "ERR_EXECUTION_TIMEOUT";

// Codes only this bridge produces. The helper never sends them.
/// The runtime credential could not be written to the OS keyring. `resume`
/// issues a fresh one, so the UI offers Resume.
pub const ERR_SECRET_STORE: &str = "ERR_SECRET_STORE";
/// The installed profile could not be saved to or selected in `inference.json`.
pub const ERR_CONFIG_WRITE: &str = "ERR_CONFIG_WRITE";
/// `cancel_cloud_provisioner` stopped the helper. Same code the browser mock uses.
pub const ERR_CANCELLED: &str = "ERR_CANCELLED";

pub const ALLOWLISTED_OPERATIONS: &[&str] = &[
    "inspect",
    "plan",
    "apply",
    "resume",
    "cleanup_plan",
    "cleanup_apply",
    "forget_credential",
    "probe_compatibility",
];

pub const ALLOWLISTED_PROVIDERS: &[&str] = &["modal", "beam"];

/// The only environment variables the helper inherits: what a process needs
/// to start, find its temp dir and reach the network through a proxy. The
/// helper holds a pasted provider key, so nothing else the app was started
/// with gets through: `MODAL_SERVER_URL` or `MC_BEAM_GATEWAY_HOST` would send
/// that key to another server, `PYTHONPATH` would run other code next to it,
/// and a provider token variable would stand in for the key the user pasted.
const HELPER_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    // Windows. Python does not start without SYSTEMROOT.
    "SYSTEMROOT",
    "WINDIR",
    "USERPROFILE",
    "TEMP",
    "TMP",
    "APPDATA",
    "LOCALAPPDATA",
];

/// The Tauri event that carries IC-2 progress to the webview.
pub const PROGRESS_EVENT: &str = "provision://progress";
/// IC-2 step ids. Fixed: the UI has a label for each.
pub const PROGRESS_STEPS: &[&str] = &[
    "inspect", "validate", "volume", "state", "secret", "image", "deploy", "weights", "token",
    "endpoint", "health", "cleanup",
];
/// IC-2 step states.
pub const PROGRESS_STATES: &[&str] = &["start", "done", "fail", "skip"];

/// A progress line is about 80 bytes. Anything far longer is not one, and
/// bounding the line keeps a helper that prints a huge traceback from growing
/// the buffer without limit.
const MAX_PROGRESS_LINE_BYTES: usize = 4 * 1024;
/// How often the runner looks at the child, the stop flag and the clock.
const WAIT_INTERVAL: Duration = Duration::from_millis(40);
/// How long the pipe readers get after the helper exits. A process the helper
/// started can hold its pipes open after it exits; the answer is not worth
/// waiting on forever.
const READER_GRACE: Duration = Duration::from_secs(5);
/// Bound on each `python` probe of the development fallback.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// The sidecar's name. Tauri's `externalBin` ships it next to the main
/// executable with the target-triple suffix stripped.
const SIDECAR_NAME: &str = "manga-cleaner-provisioner";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperCommand {
    Executable(PathBuf),
    PythonModule {
        python_bin: PathBuf,
        workdir: PathBuf,
    },
}

fn binary_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{}.exe", base)
    } else {
        base.to_string()
    }
}

/// The allowlisted spelling of `value`. Returning the list's own `'static`
/// string is what keeps helper-chosen text out of events and envelopes.
fn allowlisted(list: &[&'static str], value: &str) -> Option<&'static str> {
    list.iter().copied().find(|item| *item == value)
}

/// Discover the cloud provisioner helper through safe search paths without shell invocation.
///
/// Order: the `MANGA_CLEANER_PROVISIONER_BIN` override, the bundled sidecar
/// next to the executable, then (development builds only) the sidecar the
/// release script builds into `src-tauri/binaries/` and the Python module.
pub fn discover_helper() -> Option<HelperCommand> {
    if let Ok(env_path) = std::env::var("MANGA_CLEANER_PROVISIONER_BIN") {
        let p = PathBuf::from(env_path.trim());
        if !p.as_os_str().is_empty() && p.is_file() {
            return Some(HelperCommand::Executable(p));
        }
    }

    let bundled = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(binary_name(SIDECAR_NAME))));
    if let Some(bundled) = bundled.filter(|p| p.is_file()) {
        return Some(HelperCommand::Executable(bundled));
    }

    // A release build trusts only its bundled sidecar. The other two paths are
    // a compile-time checkout and whatever Python this machine has, and an
    // installed app has no business running either.
    if cfg!(debug_assertions) {
        discover_dev_helper()
    } else {
        None
    }
}

/// The development fallbacks: the sidecar `.github/scripts/build-cloud-provisioner.py`
/// leaves in `src-tauri/binaries/`, then `python -m provisioner` from the checkout.
fn discover_dev_helper() -> Option<HelperCommand> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let built = manifest_dir.join("binaries").join(binary_name(&format!(
        "{SIDECAR_NAME}-{}",
        env!("MC_TARGET_TRIPLE")
    )));
    if built.is_file() {
        return Some(HelperCommand::Executable(built));
    }

    let root = manifest_dir.parent()?;
    if !root.join("provisioner").join("__main__.py").is_file() {
        return None;
    }
    find_python(root).map(|python_bin| HelperCommand::PythonModule {
        python_bin,
        workdir: root.to_path_buf(),
    })
}

fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python3")
    }
}

/// A Python for the development fallback: the first candidate that can import
/// `modal`, else the first that runs at all. A helper without its SDK still
/// answers with its own missing-SDK error, which says more than "not found".
fn find_python(root: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("VIRTUAL_ENV")
        .map(|venv| venv_python(Path::new(&venv)))
        .into_iter()
        .chain([".venv", "venv"].map(|name| venv_python(&root.join(name))))
        .filter(|python| python.is_file())
        .collect();
    // Bare names resolve through PATH at spawn; no shell is involved.
    candidates.extend(["python3", "python"].map(PathBuf::from));

    let mut runnable = None;
    for python in candidates {
        if python_succeeds(&python, &["-c", "import modal"]) {
            return Some(python);
        }
        if runnable.is_none() && python_succeeds(&python, &["--version"]) {
            runnable = Some(python);
        }
    }
    runnable
}

/// Run a short Python probe, bounded so a wedged interpreter cannot stall discovery.
fn python_succeeds(python: &Path, args: &[&str]) -> bool {
    let Ok(mut child) = Command::new(python)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(WAIT_INTERVAL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Resolves journal root without secret logging.
pub fn resolve_journal_root(app: &tauri::AppHandle) -> PathBuf {
    if let Ok(val) = std::env::var("MANGA_CLEANER_JOURNAL_ROOT") {
        let p = PathBuf::from(val.trim());
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    use tauri::Manager;
    if let Ok(app_data) = app.path().app_data_dir() {
        return app_data.join("cloud").join("journal");
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        PathBuf::from(home)
            .join(".manga-cleaner")
            .join("cloud")
            .join("journal")
    } else {
        std::env::temp_dir().join("manga-cleaner-cloud-journal")
    }
}

/// Construct a standardized error response envelope.
pub fn make_error_response(
    request_id: &str,
    code: &str,
    message: &str,
    actionable_guidance: Option<&str>,
    remedy_steps: Option<&[&str]>,
) -> serde_json::Value {
    serde_json::json!({
        "protocol_version": HELPER_PROTOCOL_VERSION,
        "request_id": request_id,
        "success": false,
        "data": null,
        "error": {
            "code": code,
            "message": message,
            "actionable_guidance": actionable_guidance,
            "remedy_steps": remedy_steps.unwrap_or(&[]),
        }
    })
}

fn cancelled_response(request_id: &str) -> Value {
    make_error_response(
        request_id,
        ERR_CANCELLED,
        "The cloud setup was stopped.",
        Some("Resume continues from the last finished step. Clean up removes what was created."),
        None,
    )
}

// ============================================================================
// IC-2 progress
// ============================================================================

/// One `provision://progress` event. Every string is one of this file's own
/// allowlisted constants; nothing the helper wrote crosses as free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProvisionProgress {
    pub op: &'static str,
    pub provider: &'static str,
    pub step: &'static str,
    pub state: &'static str,
    pub pct: Option<u8>,
}

/// Parse one stderr line as IC-2 progress.
///
/// Accepts exactly `{"mc_progress":1,"op":..,"step":..,"state":..,"pct":..}`:
/// those five keys and no others, `mc_progress` the integer 1, allowlisted
/// strings, and `pct` null or an integer 0..=100. Anything else is `None`.
pub fn parse_progress_line(line: &[u8], provider: &'static str) -> Option<ProvisionProgress> {
    let value: Value = serde_json::from_slice(line).ok()?;
    let fields = value.as_object()?;
    if fields.len() != 5 || fields.get("mc_progress")?.as_u64()? != 1 {
        return None;
    }
    let text = |key: &str, list: &[&'static str]| {
        fields
            .get(key)
            .and_then(Value::as_str)
            .and_then(|v| allowlisted(list, v))
    };
    let pct = match fields.get("pct")? {
        Value::Null => None,
        number => Some(
            u8::try_from(number.as_u64()?)
                .ok()
                .filter(|pct| *pct <= 100)?,
        ),
    };
    Some(ProvisionProgress {
        op: text("op", ALLOWLISTED_OPERATIONS)?,
        provider,
        step: text("step", PROGRESS_STEPS)?,
        state: text("state", PROGRESS_STATES)?,
        pct,
    })
}

/// Read `pipe` to the end, handing each newline-terminated line to `on_line`.
/// A line longer than [`MAX_PROGRESS_LINE_BYTES`] is dropped whole.
fn for_each_line(mut pipe: impl Read, mut on_line: impl FnMut(&[u8])) {
    let mut line = Vec::with_capacity(256);
    let mut overlong = false;
    let mut chunk = [0u8; 4096];
    loop {
        let n = match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for &byte in &chunk[..n] {
            if byte == b'\n' {
                if !overlong {
                    on_line(&line);
                }
                line.clear();
                overlong = false;
            } else if line.len() < MAX_PROGRESS_LINE_BYTES {
                line.push(byte);
            } else {
                overlong = true;
            }
        }
    }
    if !overlong && !line.is_empty() {
        on_line(&line);
    }
}

/// Read `pipe` to the end, keeping at most `cap` bytes. True when more came.
fn read_bounded(mut pipe: impl Read, cap: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut overflowed = false;
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let room = cap - kept.len();
                kept.extend_from_slice(&chunk[..n.min(room)]);
                overflowed |= n > room;
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    (kept, overflowed)
}

// ============================================================================
// Running helpers and cancel
// ============================================================================

/// Stop flags of the helpers running now, by run number.
fn running_helpers() -> MutexGuard<'static, HashMap<u64, Arc<AtomicBool>>> {
    static RUNNING: OnceLock<Mutex<HashMap<u64, Arc<AtomicBool>>>> = OnceLock::new();
    RUNNING
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A helper run's entry in [`running_helpers`], removed however the run ends.
struct RunSlot {
    id: u64,
    stop: Arc<AtomicBool>,
}

impl RunSlot {
    fn register() -> RunSlot {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        running_helpers().insert(id, stop.clone());
        RunSlot { id, stop }
    }
}

impl Drop for RunSlot {
    fn drop(&mut self) {
        running_helpers().remove(&self.id);
    }
}

/// Flag every running helper to stop. True when one was running.
///
/// The runner that owns the child does the killing, on its next look at the
/// flag: only the owner can be sure the process id still names its child.
pub fn stop_running_helpers() -> bool {
    let helpers = running_helpers();
    for stop in helpers.values() {
        stop.store(true, Ordering::SeqCst);
    }
    !helpers.is_empty()
}

/// How long an exiting app waits for running helpers to be killed.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// Stop every running helper before the app exits.
///
/// Flags them as a cancel does, then waits a short while for each runner to
/// kill its child and give up its slot, since the process ending first would
/// leave the helper running.
pub fn stop_helpers_for_exit() {
    if !stop_running_helpers() {
        return;
    }
    let started = Instant::now();
    while started.elapsed() < EXIT_GRACE && !running_helpers().is_empty() {
        std::thread::sleep(WAIT_INTERVAL);
    }
}

/// Answer of `cancel_cloud_provisioner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProvisionCancelResult {
    /// A helper was running and has been told to stop. Its own command then
    /// answers with `ERR_CANCELLED`.
    pub cancelled: bool,
}

/// Stop the running provisioner helper (IC-2). The helper's journal makes a later `resume` safe.
#[tauri::command]
pub fn cancel_cloud_provisioner() -> ProvisionCancelResult {
    ProvisionCancelResult {
        cancelled: stop_running_helpers(),
    }
}

/// Kill the helper and everything it started, then reap it.
fn kill_helper(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(pid) = libc::pid_t::try_from(child.id()) {
            if pid > 0 {
                // SAFETY: `kill` takes no pointers. The group id is still the
                // child's: the child leads its group (see `process_group(0)` at
                // spawn) and has not been reaped, so its id cannot be reused.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

// ============================================================================
// Execution
// ============================================================================

/// One helper invocation.
pub struct HelperRun<'a> {
    pub command: &'a HelperCommand,
    pub journal_root: &'a Path,
    pub request: &'a [u8],
    pub request_id: &'a str,
    /// Allowlisted provider name, copied into every progress event.
    pub provider: &'static str,
    pub timeout: Duration,
    /// Set by [`stop_running_helpers`]; the runner kills the helper when it sees it.
    pub stop: &'a AtomicBool,
}

enum Ended {
    Exited,
    Stopped,
    TimedOut,
    Lost,
}

/// Execute helper subprocess with bounded request/response via stdin/stdout, no shell invocation, and strict timeout.
///
/// `on_progress` receives each valid IC-2 line, in order, before this returns.
pub fn execute_helper_subprocess(
    run: &HelperRun<'_>,
    on_progress: Box<dyn Fn(&ProvisionProgress) + Send>,
) -> Value {
    let request_id = run.request_id;
    let mut command = match run.command {
        HelperCommand::Executable(bin) => {
            let mut c = Command::new(bin);
            c.arg("--journal-root").arg(run.journal_root);
            c
        }
        HelperCommand::PythonModule {
            python_bin,
            workdir,
        } => {
            let mut c = Command::new(python_bin);
            c.current_dir(workdir);
            c.args(["-m", "provisioner", "--journal-root"])
                .arg(run.journal_root);
            c
        }
    };

    command.env_clear();
    for key in HELPER_ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a stop or a timeout reaches everything the
        // helper started: the PyInstaller one-file bootloader runs the real
        // helper as a child, and provider SDKs start processes of their own.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW. The helper is a console program; a windowed app
        // that starts one without this flag opens a console window for it.
        command.creation_flags(0x0800_0000);
    }

    if run.stop.load(Ordering::SeqCst) {
        return cancelled_response(request_id);
    }

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return make_error_response(
                request_id,
                ERR_EXECUTION_FAILED,
                &format!("Failed to spawn cloud provisioner helper process: {}", e),
                Some("Check file permissions and ensure binary is executable."),
                None,
            );
        }
    };

    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        kill_helper(&mut child);
        return make_error_response(
            request_id,
            ERR_EXECUTION_FAILED,
            "Failed to open helper stdio pipes",
            None,
            None,
        );
    };

    // Readers start before the request is written: a helper that prints
    // before it reads would otherwise fill a pipe and wait on us forever.
    let (stdout_tx, stdout_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = stdout_tx.send(read_bounded(stdout, MAX_RESPONSE_BYTES));
    });
    let (stderr_tx, stderr_rx) = mpsc::channel::<()>();
    let provider = run.provider;
    std::thread::spawn(move || {
        for_each_line(stderr, |line| {
            if let Some(progress) = parse_progress_line(line, provider) {
                on_progress(&progress);
            }
        });
        let _ = stderr_tx.send(());
    });
    // Written from its own thread so a helper that never reads stdin cannot
    // stall the stop flag and the timeout below. Closing stdin signals EOF.
    let request = run.request.to_vec();
    let writer = std::thread::spawn(move || {
        stdin.write_all(&request)?;
        stdin.write_all(b"\n")
    });

    let started = Instant::now();
    let ended = loop {
        match child.try_wait() {
            Ok(Some(_status)) => break Ended::Exited,
            Ok(None) => {}
            Err(_) => break Ended::Lost,
        }
        if run.stop.load(Ordering::SeqCst) {
            break Ended::Stopped;
        }
        if started.elapsed() >= run.timeout {
            break Ended::TimedOut;
        }
        std::thread::sleep(WAIT_INTERVAL);
    };

    match ended {
        Ended::Exited => {}
        Ended::Stopped => {
            kill_helper(&mut child);
            return cancelled_response(request_id);
        }
        Ended::TimedOut => {
            kill_helper(&mut child);
            return make_error_response(
                request_id,
                ERR_EXECUTION_TIMEOUT,
                &format!(
                    "Cloud provisioner helper timed out after {} seconds",
                    run.timeout.as_secs()
                ),
                Some("The cloud operation exceeded execution time limits. Check provider status."),
                Some(&[
                    "Verify provider network reachability",
                    "Retry operation with probe_compatibility",
                ]),
            );
        }
        Ended::Lost => {
            kill_helper(&mut child);
            return make_error_response(
                request_id,
                ERR_EXECUTION_FAILED,
                "Lost track of the cloud provisioner helper process",
                None,
                None,
            );
        }
    }

    // Progress the helper wrote before exiting goes out before its answer.
    let _ = stderr_rx.recv_timeout(READER_GRACE);
    let Ok((raw_stdout, overflowed)) = stdout_rx.recv_timeout(READER_GRACE) else {
        return make_error_response(
            request_id,
            ERR_INVALID_PAYLOAD,
            "Provisioner helper exited but its output never closed",
            None,
            None,
        );
    };
    if overflowed {
        return make_error_response(
            request_id,
            ERR_INVALID_PAYLOAD,
            "Provisioner helper response exceeded the size limit",
            None,
            None,
        );
    }

    let stdout_str = String::from_utf8_lossy(&raw_stdout);
    let trimmed = stdout_str.trim();

    if trimmed.is_empty() {
        // The writer has finished: the child exited, so its stdin is closed.
        let wrote = writer.join().map(|result| result.is_ok()).unwrap_or(false);
        return if wrote {
            make_error_response(
                request_id,
                ERR_INVALID_PAYLOAD,
                "Provisioner helper exited without a protocol response",
                Some("Ensure provisioner dependencies are satisfied and real SDKs are configured."),
                None,
            )
        } else {
            make_error_response(
                request_id,
                ERR_INVALID_PAYLOAD,
                "Failed to write request payload to helper stdin",
                Some("Ensure helper process is not immediately terminating."),
                None,
            )
        };
    }

    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(parsed)
            if parsed.get("protocol_version").and_then(|v| v.as_str())
                == Some(HELPER_PROTOCOL_VERSION)
                && parsed.get("request_id").and_then(|v| v.as_str()) == Some(request_id)
                && parsed.get("success").and_then(|v| v.as_bool()).is_some() =>
        {
            parsed
        }
        Ok(_) => make_error_response(
            request_id,
            ERR_INVALID_PAYLOAD,
            "Provisioner helper returned an invalid protocol envelope",
            None,
            None,
        ),
        Err(e) => make_error_response(
            request_id,
            ERR_INVALID_PAYLOAD,
            &format!(
                "Failed to parse JSON response from provisioner helper: {}",
                e
            ),
            Some("Ensure helper emits only valid JSON protocol envelopes on stdout."),
            None,
        ),
    }
}

// ============================================================================
// IC-1 installation result
// ============================================================================

/// Take `data.runtime_credential` out of an envelope. Done to every envelope,
/// whatever the operation or outcome, so the credential cannot reach the
/// webview through a path that forgot to ask.
fn take_runtime_credential(response: &mut Value) -> Option<Value> {
    response
        .get_mut("data")
        .and_then(Value::as_object_mut)
        .and_then(|data| data.remove("runtime_credential"))
}

/// The keyring value for an IC-1 credential, or `None` when it is missing,
/// of the wrong kind for `provider`, or malformed.
fn runtime_secret(provider: CloudProvider, credential: Option<&Value>) -> Option<SecretValue> {
    let fields = credential?.as_object()?;
    let text = |key: &str| fields.get(key).and_then(Value::as_str);
    match (provider, text("kind")?) {
        (CloudProvider::Modal, "modal_proxy") => {
            encode_modal_runtime_secret(text("token_id")?, text("token_secret")?).ok()
        }
        (CloudProvider::Beam, "beam_bearer") => {
            let token = text("token")?.trim();
            // Bounded like the Modal secret; a control character could never
            // form a valid Authorization header anyway.
            (!token.is_empty() && token.len() <= 4096 && !token.chars().any(char::is_control))
                .then(|| SecretValue::new(token))
        }
        _ => None,
    }
}

fn provider_label(provider: CloudProvider) -> &'static str {
    match provider {
        CloudProvider::Modal => "Modal",
        CloudProvider::Beam => "Beam",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Where a finished installation is saved. The fields are the seams tests
/// replace: a temporary `inference.json`, an in-memory keyring, a fake check.
pub(crate) struct Installer<'a> {
    /// `inference.json`. `None` when the app has no config folder.
    pub config_path: Option<&'a Path>,
    pub secrets: &'a SecretManager,
    pub verify: &'a dyn Fn(&InferenceConfig, CloudProvider, &str) -> CloudConnectionStatus,
}

/// Finish a successful `apply` or `resume` (IC-1): keyring, profile, default
/// target, health check. The envelope that comes back never carries the
/// runtime credential; on success its `data` gains `profile`, `health` and
/// `selected`.
pub(crate) fn complete_installation(
    mut response: Value,
    provider: CloudProvider,
    installer: &Installer<'_>,
) -> Value {
    let credential = take_runtime_credential(&mut response);
    if response.get("success").and_then(Value::as_bool) != Some(true) {
        return response;
    }
    match save_installation(&response, provider, credential.as_ref(), installer) {
        Ok(additions) => {
            if let Some(data) = response.get_mut("data").and_then(Value::as_object_mut) {
                data.extend(additions);
            }
            response
        }
        Err((code, message)) => {
            let request_id = response
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            make_error_response(request_id, code, message, None, None)
        }
    }
}

type InstallError = (&'static str, &'static str);

const INSTALL_INVALID: InstallError = (
    ERR_INVALID_PAYLOAD,
    "The helper reported success without a usable installation id, endpoint or runtime credential. Resume the setup to issue them again.",
);
const INSTALL_CONFIG: InstallError = (
    ERR_CONFIG_WRITE,
    "The cloud profile could not be saved to the app settings. Resume the setup once the settings file can be written.",
);
const INSTALL_SECRET: InstallError = (
    ERR_SECRET_STORE,
    "The runtime credential could not be saved to the system credential manager. Resume the setup to issue a new one.",
);

fn save_installation(
    response: &Value,
    provider: CloudProvider,
    credential: Option<&Value>,
    installer: &Installer<'_>,
) -> Result<Map<String, Value>, InstallError> {
    let data = response
        .get("data")
        .and_then(Value::as_object)
        .ok_or(INSTALL_INVALID)?;
    let profile_id = data
        .get("installation_id")
        .and_then(Value::as_str)
        .filter(|id| config::validate_profile_id(id).is_ok())
        .ok_or(INSTALL_INVALID)?;
    if data
        .get("provider")
        .is_some_and(|named| named.as_str() != Some(provider.as_str()))
    {
        return Err(INSTALL_INVALID);
    }
    let endpoint_url = data
        .get("endpoint_url")
        .and_then(Value::as_str)
        .ok_or(INSTALL_INVALID)?;
    let (canonical_origin, fingerprint) =
        config::validate_https_endpoint(endpoint_url).map_err(|_| INSTALL_INVALID)?;
    let secret = runtime_secret(provider, credential).ok_or(INSTALL_INVALID)?;

    // Read before the keyring is touched, so an unreadable settings file
    // leaves no credential behind that no profile points at.
    let path = installer.config_path.ok_or(INSTALL_CONFIG)?;
    let mut inference =
        config::read_inference_config_from_path(path).map_err(|_| INSTALL_CONFIG)?;

    // Same epoch rule as `store_cloud_secret`: invalidate before and after,
    // so no grant minted against the old credential survives the swap.
    let key = SecretKey::new(
        provider,
        profile_id.to_string(),
        fingerprint.clone(),
        SecretRole::Runtime,
    );
    let consent = ConsentService::global();
    consent.invalidate_profile(provider, profile_id);
    let stored = installer.secrets.store_secret(&key, secret, false);
    consent.invalidate_profile(provider, profile_id);
    stored.map_err(|_| INSTALL_SECRET)?;

    let profiles = match provider {
        CloudProvider::Beam => &mut inference.beam_profiles,
        CloudProvider::Modal => &mut inference.modal_profiles,
    };
    let previous = profiles.get(profile_id);
    let is_new = previous.is_none();
    // A `resume` re-issues the credential of an installation the user may
    // have renamed; keep their name and the original creation time.
    let name = previous
        .map(|p| p.name.clone())
        .unwrap_or_else(|| format!("{} ({profile_id})", provider_label(provider)));
    let now = now_ms();
    let created_at_ms = previous.map_or(now, |p| p.created_at_ms);
    profiles.insert(
        profile_id.to_string(),
        CloudProfile {
            id: profile_id.to_string(),
            name: name.clone(),
            endpoint_url: endpoint_url.to_string(),
            canonical_origin,
            canonical_origin_fingerprint: fingerprint,
            created_at_ms,
            updated_at_ms: now,
        },
    );
    inference.selected_target = match provider {
        CloudProvider::Beam => ExecutionTarget::Beam {
            profile_id: profile_id.to_string(),
        },
        CloudProvider::Modal => ExecutionTarget::Modal {
            profile_id: profile_id.to_string(),
        },
    };
    let Ok(written) = write_inference_config_at(path, inference) else {
        // A new installation's credential would be left with no profile
        // pointing at it; the resume the error asks for stores it again. A
        // profile already saved keeps the new one, which has replaced its old
        // credential in the keyring.
        if is_new {
            let _ = installer.secrets.delete_secret(&key);
        }
        return Err(INSTALL_CONFIG);
    };

    let health = (installer.verify)(&written, provider, profile_id);
    let mut additions = Map::new();
    additions.insert(
        "profile".into(),
        json!({
            "provider": provider.as_str(),
            "profile_id": profile_id,
            "name": name,
            "endpoint_url": endpoint_url,
        }),
    );
    additions.insert(
        "health".into(),
        json!({ "ok": health.ok, "status": health.status, "latency_ms": health.latency_ms }),
    );
    additions.insert("selected".into(), Value::Bool(true));
    Ok(additions)
}

// ============================================================================
// Commands
// ============================================================================

/// Core runner for cloud provisioner requests.
#[tauri::command]
pub async fn run_cloud_provisioner(
    app: tauri::AppHandle,
    op: String,
    provider: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    crate::library::blocking(move || Ok(run_provisioner(&app, &op, &provider, params))).await
}

fn run_provisioner(
    app: &tauri::AppHandle,
    op: &str,
    provider: &str,
    params: Option<Value>,
) -> Value {
    let params_val = params.unwrap_or_else(|| serde_json::json!({}));

    // Resolve or generate request_id
    let request_id = params_val
        .get("request_id")
        .and_then(|v| v.as_str())
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("req-mc-{}", now_ms()));

    // 1. Validate operation
    let Some(op) = allowlisted(ALLOWLISTED_OPERATIONS, op) else {
        return make_error_response(
            &request_id,
            ERR_UNSUPPORTED_OP,
            &format!("Unsupported operation '{}'. Allowlisted operations: {:?}", op, ALLOWLISTED_OPERATIONS),
            Some("Operation must be one of inspect, plan, apply, resume, cleanup_plan, cleanup_apply, forget_credential, probe_compatibility."),
            None,
        );
    };

    // 2. Validate provider
    let (provider_name, cloud_provider) = match provider {
        "modal" => ("modal", CloudProvider::Modal),
        "beam" => ("beam", CloudProvider::Beam),
        _ => {
            return make_error_response(
                &request_id,
                ERR_UNSUPPORTED_PROVIDER,
                &format!(
                    "Unsupported provider '{}'. Allowlisted providers: {:?}",
                    provider, ALLOWLISTED_PROVIDERS
                ),
                Some("Provider must be one of 'modal' or 'beam'."),
                None,
            );
        }
    };

    let timeout_secs = match op {
        "apply" | "resume" | "cleanup_apply" => DEPLOY_TIMEOUT_SECS,
        _ => DEFAULT_TIMEOUT_SECS,
    };

    // 3. Assemble JSON envelope
    let envelope = serde_json::json!({
        "protocol_version": HELPER_PROTOCOL_VERSION,
        "request_id": request_id,
        "op": op,
        "provider": provider_name,
        "params": params_val,
    });

    let req_bytes = match serde_json::to_vec(&envelope) {
        Ok(b) => b,
        Err(e) => {
            return make_error_response(
                &request_id,
                ERR_INVALID_PAYLOAD,
                &format!("Failed to serialize request envelope: {}", e),
                None,
                None,
            );
        }
    };

    if req_bytes.len() > MAX_REQUEST_BYTES {
        return make_error_response(
            &request_id,
            ERR_PAYLOAD_TOO_LARGE,
            &format!(
                "Request payload size ({} bytes) exceeds limit of {} bytes",
                req_bytes.len(),
                MAX_REQUEST_BYTES
            ),
            Some("Ensure request parameters are bounded below 64 KiB."),
            None,
        );
    }

    // 4. Discover helper binary
    let Some(helper) = discover_helper() else {
        return make_error_response(
            &request_id,
            ERR_PROVIDER_UNAVAILABLE,
            "Cloud provisioner helper binary not found: no executable discovered in MANGA_CLEANER_PROVISIONER_BIN or next to the application",
            Some("Install or package the cloud provisioner helper binary, or set MANGA_CLEANER_PROVISIONER_BIN to its path."),
            Some(&[
                "Set MANGA_CLEANER_PROVISIONER_BIN to the absolute path of the provisioner executable",
                "In a development build, build the sidecar into src-tauri/binaries or make Python 3 with the provisioner requirements available",
            ]),
        );
    };

    // 5. Resolve journal root
    let journal_root = resolve_journal_root(app);

    // 6. Execute helper with strict timeout, stoppable through `cancel_cloud_provisioner`
    let slot = RunSlot::register();
    let emitter = app.clone();
    let response = execute_helper_subprocess(
        &HelperRun {
            command: &helper,
            journal_root: &journal_root,
            request: &req_bytes,
            request_id: &request_id,
            provider: provider_name,
            timeout: Duration::from_secs(timeout_secs),
            stop: &slot.stop,
        },
        Box::new(move |progress: &ProvisionProgress| {
            use tauri::Emitter;
            let _ = emitter.emit(PROGRESS_EVENT, progress);
        }),
    );
    drop(slot);

    // 7. IC-1: the runtime credential goes to the keyring, never to the webview
    if matches!(op, "apply" | "resume") {
        let config_path = config::config_path(app).ok();
        complete_installation(
            response,
            cloud_provider,
            &Installer {
                config_path: config_path.as_deref(),
                secrets: SecretManager::global(),
                verify: &verify_cloud_profile,
            },
        )
    } else {
        let mut response = response;
        take_runtime_credential(&mut response);
        response
    }
}

#[tauri::command]
pub async fn provision_inspect(
    app: tauri::AppHandle,
    provider: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    run_cloud_provisioner(app, "inspect".into(), provider, params).await
}

#[tauri::command]
pub async fn provision_plan(
    app: tauri::AppHandle,
    provider: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    run_cloud_provisioner(app, "plan".into(), provider, params).await
}

#[tauri::command]
pub async fn provision_apply(
    app: tauri::AppHandle,
    provider: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    run_cloud_provisioner(app, "apply".into(), provider, params).await
}

#[tauri::command]
pub async fn provision_resume(
    app: tauri::AppHandle,
    provider: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    run_cloud_provisioner(app, "resume".into(), provider, params).await
}

#[tauri::command]
pub async fn provision_cleanup(
    app: tauri::AppHandle,
    provider: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let op = params
        .as_ref()
        .and_then(|p| p.get("cleanup_op"))
        .and_then(|v| v.as_str())
        .unwrap_or("cleanup_apply");
    run_cloud_provisioner(app, op.into(), provider, params).await
}

#[tauri::command]
pub async fn provision_probe(
    app: tauri::AppHandle,
    provider: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    run_cloud_provisioner(app, "probe_compatibility".into(), provider, params).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::secrets::{decode_modal_runtime_secret, SecretStore, StorageBackendKind};

    #[test]
    fn test_error_response_structure() {
        let err = make_error_response(
            "req-1",
            ERR_PROVIDER_UNAVAILABLE,
            "Helper missing",
            Some("Install helper"),
            Some(&["Step 1", "Step 2"]),
        );
        assert_eq!(err["protocol_version"], "1.0.0");
        assert_eq!(err["request_id"], "req-1");
        assert_eq!(err["success"], false);
        assert_eq!(err["error"]["code"], ERR_PROVIDER_UNAVAILABLE);
        assert_eq!(err["error"]["message"], "Helper missing");
        assert_eq!(err["error"]["actionable_guidance"], "Install helper");
        assert_eq!(err["error"]["remedy_steps"][0], "Step 1");
    }

    #[test]
    fn test_allowlisted_operations_contains_required() {
        for op in &[
            "inspect",
            "plan",
            "apply",
            "resume",
            "cleanup_plan",
            "cleanup_apply",
            "probe_compatibility",
        ] {
            assert!(ALLOWLISTED_OPERATIONS.contains(op));
        }
    }

    #[test]
    fn test_allowlisted_providers() {
        assert!(ALLOWLISTED_PROVIDERS.contains(&"modal"));
        assert!(ALLOWLISTED_PROVIDERS.contains(&"beam"));
    }

    #[test]
    fn deploy_operations_get_thirty_minutes() {
        assert_eq!(DEPLOY_TIMEOUT_SECS, 30 * 60);
    }

    // ---------------------------------------------------------------- IC-2

    #[test]
    fn the_exact_progress_shape_parses() {
        let line = br#"{"mc_progress":1,"op":"apply","step":"deploy","state":"start","pct":null}"#;
        assert_eq!(
            parse_progress_line(line, "modal"),
            Some(ProvisionProgress {
                op: "apply",
                provider: "modal",
                step: "deploy",
                state: "start",
                pct: None,
            })
        );
        let with_pct =
            br#"{"mc_progress":1,"op":"resume","step":"weights","state":"done","pct":100}"#;
        assert_eq!(
            parse_progress_line(with_pct, "beam").and_then(|p| p.pct),
            Some(100)
        );
        let spaced = b" {\"pct\": 0, \"state\": \"skip\", \"step\": \"cleanup\", \"op\": \"cleanup_apply\", \"mc_progress\": 1}\r";
        assert_eq!(
            parse_progress_line(spaced, "beam").map(|p| p.step),
            Some("cleanup")
        );
    }

    #[test]
    fn every_other_stderr_line_is_dropped() {
        let rejected: &[&[u8]] = &[
            b"",
            b"Traceback (most recent call last):",
            b"token_secret=ws-very-secret",
            b"[1, 2, 3]",
            b"null",
            // missing or extra keys
            br#"{"mc_progress":1,"op":"apply","step":"deploy","state":"start"}"#,
            br#"{"mc_progress":1,"op":"apply","step":"deploy","state":"start","pct":null,"message":"ws-secret"}"#,
            // marker not the integer 1
            br#"{"mc_progress":2,"op":"apply","step":"deploy","state":"start","pct":null}"#,
            br#"{"mc_progress":"1","op":"apply","step":"deploy","state":"start","pct":null}"#,
            br#"{"mc_progress":1.0,"op":"apply","step":"deploy","state":"start","pct":null}"#,
            br#"{"mc_progress":true,"op":"apply","step":"deploy","state":"start","pct":null}"#,
            // values off the allowlists
            br#"{"mc_progress":1,"op":"destroy","step":"deploy","state":"start","pct":null}"#,
            br#"{"mc_progress":1,"op":"apply","step":"ws-secret","state":"start","pct":null}"#,
            br#"{"mc_progress":1,"op":"apply","step":"deploy","state":"running","pct":null}"#,
            br#"{"mc_progress":1,"op":"apply","step":"Deploy","state":"start","pct":null}"#,
            // pct out of range or not an integer
            br#"{"mc_progress":1,"op":"apply","step":"deploy","state":"start","pct":101}"#,
            br#"{"mc_progress":1,"op":"apply","step":"deploy","state":"start","pct":-1}"#,
            br#"{"mc_progress":1,"op":"apply","step":"deploy","state":"start","pct":4.5}"#,
            br#"{"mc_progress":1,"op":"apply","step":"deploy","state":"start","pct":"4"}"#,
            br#"{"mc_progress":1,"op":"apply","step":"deploy","state":"start","pct":1000}"#,
            // not UTF-8
            b"{\"mc_progress\":1,\"op\":\"apply\",\"step\":\"deploy\",\"state\":\"start\",\"pct\":null,\xff}",
            // two objects on one line
            br#"{"mc_progress":1,"op":"apply","step":"deploy","state":"start","pct":null}{"mc_progress":1,"op":"apply","step":"deploy","state":"done","pct":null}"#,
        ];
        for line in rejected {
            assert_eq!(
                parse_progress_line(line, "modal"),
                None,
                "accepted {:?}",
                String::from_utf8_lossy(line)
            );
        }
    }

    #[test]
    fn lines_split_on_newlines_and_overlong_lines_are_dropped_whole() {
        let mut input = Vec::new();
        input.extend_from_slice(b"first\n");
        input.extend(std::iter::repeat_n(b'x', MAX_PROGRESS_LINE_BYTES + 10));
        input.extend_from_slice(b"\nsecond\r\n\nlast");
        let mut seen = Vec::new();
        for_each_line(input.as_slice(), |line| {
            seen.push(String::from_utf8_lossy(line).into_owned())
        });
        assert_eq!(seen, ["first", "second\r", "", "last"]);
    }

    #[test]
    fn stdout_is_drained_past_the_cap_and_the_overflow_reported() {
        let input = vec![b'a'; 20_000];
        let (kept, overflowed) = read_bounded(input.as_slice(), 1000);
        assert_eq!(kept.len(), 1000);
        assert!(overflowed);
        let (kept, overflowed) = read_bounded(&input[..1000], 1000);
        assert_eq!(kept.len(), 1000);
        assert!(!overflowed);
    }

    #[test]
    fn cancel_flags_every_running_helper_and_forgets_finished_ones() {
        let slot = RunSlot::register();
        assert!(stop_running_helpers());
        assert!(slot.stop.load(Ordering::SeqCst));
        let id = slot.id;
        drop(slot);
        assert!(!running_helpers().contains_key(&id));
    }

    #[test]
    fn an_exit_waits_for_the_runner_to_give_up_its_slot() {
        let slot = RunSlot::register();
        let stop = slot.stop.clone();
        let id = slot.id;
        let runner = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            drop(slot);
        });
        let started = Instant::now();
        stop_helpers_for_exit();
        assert!(started.elapsed() < EXIT_GRACE);
        assert!(!running_helpers().contains_key(&id));
        runner.join().unwrap();
    }

    // ---------------------------------------------------------------- fake helper

    /// A throwaway directory under the system temp dir.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mc-provision-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write an executable `/bin/sh` script standing in for the helper.
    #[cfg(unix)]
    fn fake_helper(dir: &Path, body: &str) -> HelperCommand {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-provisioner");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        HelperCommand::Executable(path)
    }

    /// Run the fake helper, retrying the spawn when another test thread's fork
    /// still holds the freshly written script open ("Text file busy").
    #[cfg(unix)]
    fn run_fake(
        command: &HelperCommand,
        journal: &Path,
        timeout: Duration,
        stop: &AtomicBool,
    ) -> (Value, Vec<ProvisionProgress>) {
        for _ in 0..20 {
            let events = Arc::new(Mutex::new(Vec::new()));
            let sink = events.clone();
            let response = execute_helper_subprocess(
                &HelperRun {
                    command,
                    journal_root: journal,
                    request: br#"{"request_id":"req-fake-1"}"#,
                    request_id: "req-fake-1",
                    provider: "modal",
                    timeout,
                    stop,
                },
                Box::new(move |progress: &ProvisionProgress| sink.lock().unwrap().push(*progress)),
            );
            let busy = response["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("Text file busy"));
            if !busy {
                let events = events.lock().unwrap().clone();
                return (response, events);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the fake helper never became executable");
    }

    #[cfg(unix)]
    #[test]
    fn the_helper_inherits_only_the_allowlisted_environment() {
        let dir = scratch("env");
        let seen = dir.join("env.txt");
        let helper = fake_helper(
            &dir,
            &format!(
                r#"read -r request
env > '{}'
echo '{{"protocol_version":"1.0.0","request_id":"req-fake-1","success":true,"data":{{}},"error":null}}'"#,
                seen.display()
            ),
        );
        let stop = AtomicBool::new(false);
        let (response, _) = run_fake(&helper, &dir, Duration::from_secs(30), &stop);
        assert_eq!(response["success"], true, "{response}");
        let env = std::fs::read_to_string(&seen).unwrap();
        // The shell running the fake helper sets these itself.
        let shell = ["PWD", "OLDPWD", "SHLVL", "_"];
        for line in env.lines() {
            let Some((key, _)) = line.split_once('=') else {
                continue;
            };
            assert!(
                HELPER_ENV_ALLOWLIST.contains(&key) || shell.contains(&key),
                "the helper inherited {key}"
            );
        }
        // cargo runs the tests with this set, so its absence shows the clearing.
        assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
        assert!(!env
            .lines()
            .any(|line| line.starts_with("CARGO_MANIFEST_DIR=")));
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_run_keeps_only_valid_progress_and_the_envelope() {
        let dir = scratch("happy");
        let helper = fake_helper(
            &dir,
            r#"read -r request
echo '{"mc_progress":1,"op":"apply","step":"validate","state":"start","pct":null}' >&2
echo 'Traceback: token_secret=ws-leak' >&2
echo '{"mc_progress":1,"op":"apply","step":"deploy","state":"done","pct":50}' >&2
echo '{"protocol_version":"1.0.0","request_id":"req-fake-1","success":true,"data":{"stage":"completed"},"error":null}'"#,
        );
        let stop = AtomicBool::new(false);
        let (response, events) = run_fake(&helper, &dir, Duration::from_secs(30), &stop);
        assert_eq!(response["success"], true, "{response}");
        assert_eq!(response["data"]["stage"], "completed");
        assert_eq!(
            events,
            [
                ProvisionProgress {
                    op: "apply",
                    provider: "modal",
                    step: "validate",
                    state: "start",
                    pct: None
                },
                ProvisionProgress {
                    op: "apply",
                    provider: "modal",
                    step: "deploy",
                    state: "done",
                    pct: Some(50)
                },
            ]
        );
        assert!(!response.to_string().contains("ws-leak"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_stop_kills_the_helper_and_what_it_started() {
        let dir = scratch("stop");
        let pid_file = dir.join("grandchild.pid");
        let helper = fake_helper(
            &dir,
            &format!(
                "sleep 60 &\necho $! > '{}'\necho '{{\"mc_progress\":1,\"op\":\"apply\",\"step\":\"image\",\"state\":\"start\",\"pct\":null}}' >&2\nwait",
                pid_file.display()
            ),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stopper = {
            let stop = stop.clone();
            let pid_file = pid_file.clone();
            std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(10);
                while !pid_file.exists() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                std::thread::sleep(Duration::from_millis(100));
                stop.store(true, Ordering::SeqCst);
            })
        };
        let started = Instant::now();
        let (response, _) = run_fake(&helper, &dir, Duration::from_secs(60), &stop);
        stopper.join().unwrap();
        assert_eq!(response["error"]["code"], ERR_CANCELLED, "{response}");
        assert!(started.elapsed() < Duration::from_secs(15));

        // The grandchild went down with the group.
        let pid: libc::pid_t = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        // SAFETY: signal 0 only checks that the process exists.
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        // SAFETY: as above.
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "the helper's child survived the stop"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_past_its_timeout_is_killed() {
        let dir = scratch("timeout");
        let helper = fake_helper(&dir, "exec sleep 60");
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        let (response, _) = run_fake(&helper, &dir, Duration::from_millis(300), &stop);
        assert_eq!(
            response["error"]["code"], ERR_EXECUTION_TIMEOUT,
            "{response}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_answering_for_another_request_is_rejected() {
        let dir = scratch("mismatch");
        let helper = fake_helper(
            &dir,
            r#"cat > /dev/null
echo '{"protocol_version":"1.0.0","request_id":"req-other","success":true,"data":{},"error":null}'"#,
        );
        let stop = AtomicBool::new(false);
        let (response, _) = run_fake(&helper, &dir, Duration::from_secs(30), &stop);
        assert_eq!(response["success"], false);
        assert_eq!(response["error"]["code"], ERR_INVALID_PAYLOAD);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------------------------------------------------------------- IC-1

    const MODAL_ENDPOINT: &str = "https://acme--mc-ab12cd-gateway.modal.run/mc/v1";
    const BEAM_ENDPOINT: &str = "https://mc-ef34gh-gateway.app.beam.cloud/mc/v1";

    fn installed(provider: &str, id: &str, endpoint: &str, credential: Value) -> Value {
        json!({
            "protocol_version": HELPER_PROTOCOL_VERSION,
            "request_id": "req-install-1",
            "success": true,
            "data": {
                "installation_id": id,
                "provider": provider,
                "stage": "completed",
                "endpoint_url": endpoint,
                "runtime_credential": credential,
                "gpu": "L4",
                "idle_seconds": 120,
                "compatibility_status": "compatible",
                "setup_credential_forgotten": true,
            },
            "error": null,
        })
    }

    fn modal_credential() -> Value {
        json!({"kind": "modal_proxy", "token_id": "wk-token-id-1", "token_secret": "ws-token-secret-1"})
    }

    fn reachable(
        _: &InferenceConfig,
        provider: CloudProvider,
        profile_id: &str,
    ) -> CloudConnectionStatus {
        CloudConnectionStatus {
            ok: true,
            status: "reachable".into(),
            provider,
            profile_id: profile_id.into(),
            latency_ms: Some(123),
            message: None,
        }
    }

    fn runtime_key(provider: CloudProvider, id: &str, endpoint: &str) -> SecretKey {
        let (_, fingerprint) = config::validate_https_endpoint(endpoint).unwrap();
        SecretKey::new(provider, id.to_string(), fingerprint, SecretRole::Runtime)
    }

    #[test]
    fn a_modal_installation_is_stored_saved_selected_and_never_returned() {
        let dir = scratch("ic1-modal");
        let path = dir.join("inference.json");
        let secrets = SecretManager::new_in_memory();
        let installer = Installer {
            config_path: Some(&path),
            secrets: &secrets,
            verify: &reachable,
        };

        let out = complete_installation(
            installed("modal", "mc-ab12cd", MODAL_ENDPOINT, modal_credential()),
            CloudProvider::Modal,
            &installer,
        );

        assert_eq!(out["success"], true, "{out}");
        let data = &out["data"];
        assert!(data.get("runtime_credential").is_none());
        let text = out.to_string();
        assert!(!text.contains("ws-token-secret-1") && !text.contains("wk-token-id-1"));
        assert_eq!(
            data["profile"],
            json!({"provider": "modal", "profile_id": "mc-ab12cd", "name": "Modal (mc-ab12cd)", "endpoint_url": MODAL_ENDPOINT})
        );
        assert_eq!(
            data["health"],
            json!({"ok": true, "status": "reachable", "latency_ms": 123})
        );
        assert_eq!(data["selected"], true);
        assert_eq!(data["gpu"], "L4");

        let stored = secrets
            .get_secret(&runtime_key(
                CloudProvider::Modal,
                "mc-ab12cd",
                MODAL_ENDPOINT,
            ))
            .unwrap()
            .expect("runtime secret stored");
        let (token_id, token_secret) = decode_modal_runtime_secret(&stored).unwrap();
        assert_eq!(token_id, "wk-token-id-1");
        assert_eq!(token_secret.expose_str().unwrap(), "ws-token-secret-1");

        let saved = config::read_inference_config_from_path(&path).unwrap();
        assert_eq!(
            saved.selected_target,
            ExecutionTarget::Modal {
                profile_id: "mc-ab12cd".into()
            }
        );
        let profile = &saved.modal_profiles["mc-ab12cd"];
        assert_eq!(profile.endpoint_url, MODAL_ENDPOINT);
        assert_eq!(profile.name, "Modal (mc-ab12cd)");
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("ws-token-secret-1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_beam_resume_keeps_the_users_name_and_reports_a_failed_check() {
        let dir = scratch("ic1-beam");
        let path = dir.join("inference.json");
        let (canonical_origin, canonical_origin_fingerprint) =
            config::validate_https_endpoint(BEAM_ENDPOINT).unwrap();
        let mut existing = InferenceConfig::default();
        existing.beam_profiles.insert(
            "mc-ef34gh".into(),
            CloudProfile {
                id: "mc-ef34gh".into(),
                name: "Studio GPU".into(),
                endpoint_url: BEAM_ENDPOINT.into(),
                canonical_origin,
                canonical_origin_fingerprint,
                created_at_ms: 7,
                updated_at_ms: 7,
            },
        );
        config::write_inference_config_to_path(&path, existing).unwrap();
        let secrets = SecretManager::new_in_memory();
        let unreachable = |_: &InferenceConfig, provider, profile_id: &str| CloudConnectionStatus {
            ok: false,
            status: "unreachable".into(),
            provider,
            profile_id: profile_id.into(),
            latency_ms: None,
            message: Some("gateway endpoint unreachable or connection timed out".into()),
        };
        let installer = Installer {
            config_path: Some(&path),
            secrets: &secrets,
            verify: &unreachable,
        };

        let out = complete_installation(
            installed(
                "beam",
                "mc-ef34gh",
                BEAM_ENDPOINT,
                json!({"kind": "beam_bearer", "token": "b9-bearer-1"}),
            ),
            CloudProvider::Beam,
            &installer,
        );

        assert_eq!(out["success"], true, "{out}");
        assert_eq!(out["data"]["profile"]["name"], "Studio GPU");
        assert_eq!(
            out["data"]["health"],
            json!({"ok": false, "status": "unreachable", "latency_ms": null})
        );
        assert!(!out.to_string().contains("b9-bearer-1"));
        let stored = secrets
            .get_secret(&runtime_key(
                CloudProvider::Beam,
                "mc-ef34gh",
                BEAM_ENDPOINT,
            ))
            .unwrap()
            .unwrap();
        assert_eq!(stored.expose_str().unwrap(), "b9-bearer-1");
        let saved = config::read_inference_config_from_path(&path).unwrap();
        assert_eq!(saved.beam_profiles["mc-ef34gh"].created_at_ms, 7);
        assert_eq!(
            saved.selected_target,
            ExecutionTarget::Beam {
                profile_id: "mc-ef34gh".into()
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct BrokenKeyring;

    impl SecretStore for BrokenKeyring {
        fn get_secret(&self, _: &SecretKey) -> Result<Option<SecretValue>, String> {
            Err("read failed".into())
        }
        fn set_secret(&self, _: &SecretKey, _: &SecretValue) -> Result<(), String> {
            Err("write failed".into())
        }
        fn delete_secret(&self, _: &SecretKey) -> Result<(), String> {
            Err("delete failed".into())
        }
        fn has_secret(&self, _: &SecretKey) -> Result<bool, String> {
            Err("read failed".into())
        }
        fn backend_kind(&self) -> StorageBackendKind {
            StorageBackendKind::Keyring
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    #[test]
    fn a_keyring_failure_is_err_secret_store_and_saves_no_profile() {
        let dir = scratch("ic1-keyring");
        let path = dir.join("inference.json");
        let secrets = SecretManager::new(Box::new(BrokenKeyring));
        let installer = Installer {
            config_path: Some(&path),
            secrets: &secrets,
            verify: &reachable,
        };

        let out = complete_installation(
            installed("modal", "mc-ab12cd", MODAL_ENDPOINT, modal_credential()),
            CloudProvider::Modal,
            &installer,
        );

        assert_eq!(out["success"], false);
        assert_eq!(out["error"]["code"], ERR_SECRET_STORE);
        assert_eq!(out["request_id"], "req-install-1");
        assert!(!out.to_string().contains("ws-token-secret-1"));
        let saved = config::read_inference_config_from_path(&path).unwrap();
        assert!(saved.modal_profiles.is_empty());
        assert_eq!(saved.selected_target, ExecutionTarget::Local);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_settings_file_is_err_config_write_and_stores_no_secret() {
        let dir = scratch("ic1-config");
        // A file where the settings folder should be: every read and write fails.
        let blocker = dir.join("not-a-folder");
        std::fs::write(&blocker, b"x").unwrap();
        let path = blocker.join("inference.json");
        let secrets = SecretManager::new_in_memory();
        let installer = Installer {
            config_path: Some(&path),
            secrets: &secrets,
            verify: &reachable,
        };

        let out = complete_installation(
            installed("modal", "mc-ab12cd", MODAL_ENDPOINT, modal_credential()),
            CloudProvider::Modal,
            &installer,
        );

        assert_eq!(out["error"]["code"], ERR_CONFIG_WRITE);
        assert!(secrets
            .get_secret(&runtime_key(
                CloudProvider::Modal,
                "mc-ab12cd",
                MODAL_ENDPOINT
            ))
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_settings_write_that_fails_takes_a_new_credential_back_out() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("ic1-write");
        // Readable but not writable: the read finds no file and answers the
        // defaults, and the atomic write cannot create its temporary file.
        let folder = dir.join("config");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        let path = folder.join("inference.json");
        let secrets = SecretManager::new_in_memory();
        let installer = Installer {
            config_path: Some(&path),
            secrets: &secrets,
            verify: &reachable,
        };

        let out = complete_installation(
            installed("modal", "mc-ab12cd", MODAL_ENDPOINT, modal_credential()),
            CloudProvider::Modal,
            &installer,
        );

        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(out["error"]["code"], ERR_CONFIG_WRITE);
        assert!(secrets
            .get_secret(&runtime_key(
                CloudProvider::Modal,
                "mc-ab12cd",
                MODAL_ENDPOINT
            ))
            .unwrap()
            .is_none());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_installation_saves_nothing_and_leaks_nothing() {
        let dir = scratch("ic1-malformed");
        let path = dir.join("inference.json");
        let secrets = SecretManager::new_in_memory();
        let installer = Installer {
            config_path: Some(&path),
            secrets: &secrets,
            verify: &reachable,
        };
        let cases = [
            // Beam credential for a Modal installation
            installed(
                "modal",
                "mc-ab12cd",
                MODAL_ENDPOINT,
                json!({"kind": "beam_bearer", "token": "ws-token-secret-1"}),
            ),
            // Provider in the data disagrees with the request
            installed("beam", "mc-ab12cd", MODAL_ENDPOINT, modal_credential()),
            // Endpoint not HTTPS
            installed(
                "modal",
                "mc-ab12cd",
                "http://acme--mc-ab12cd-gateway.modal.run/mc/v1",
                modal_credential(),
            ),
            // Installation id unsafe as a profile id
            installed("modal", "../mc", MODAL_ENDPOINT, modal_credential()),
            // No credential at all
            installed("modal", "mc-ab12cd", MODAL_ENDPOINT, Value::Null),
        ];
        for case in cases {
            let out = complete_installation(case, CloudProvider::Modal, &installer);
            assert_eq!(out["error"]["code"], ERR_INVALID_PAYLOAD, "{out}");
            assert!(!out.to_string().contains("ws-token-secret-1"));
        }
        assert!(!path.exists());
        assert!(secrets
            .get_secret(&runtime_key(
                CloudProvider::Modal,
                "mc-ab12cd",
                MODAL_ENDPOINT
            ))
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_run_passes_through_minus_any_credential() {
        let secrets = SecretManager::new_in_memory();
        let installer = Installer {
            config_path: None,
            secrets: &secrets,
            verify: &reachable,
        };
        let mut failed = installed("modal", "mc-ab12cd", MODAL_ENDPOINT, modal_credential());
        failed["success"] = json!(false);
        failed["error"] = json!({"code": ERR_EXECUTION_FAILED, "message": "deploy failed"});

        let out = complete_installation(failed, CloudProvider::Modal, &installer);

        assert_eq!(out["success"], false);
        assert_eq!(out["error"]["code"], ERR_EXECUTION_FAILED);
        assert!(out["data"].get("runtime_credential").is_none());
        assert_eq!(out["data"]["installation_id"], "mc-ab12cd");
    }
}
