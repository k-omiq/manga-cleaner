//! `readSettings` and `writeSettings`.
//!
//! A patch is **merged**
//! and the whole snapshot comes back. Two consequences the implementation turns
//! on:
//!
//! - The merge is shallow, one level, matching what the interface sends: it
//!   writes `{cloud: {allowed: true}}`-shaped patches only at the top level, and
//!   a deep merge would make "unset this key" unexpressible.
//! - The vocabulary belongs to the interface, not here. This module stores and
//!   returns JSON without knowing what a setting means, for the same reason
//!   the manifest's `settings` snapshot does: a second definition of the
//!   settings shape is a second thing to disagree with the first.
//!
//! The defaults therefore come from the interface too - the adapter sends them
//! once at startup, and until it does, an unwritten settings file reads as `{}`
//! and the interface's own defaults apply.
//!
//! ## One key does not live here
//!
//! `hfToken` arrives through the same patch as every other setting and is the
//! one that never lands in the file. [`write`] lifts it out and hands it to
//! [`crate::weights::store_token`], which puts it in the operating system's
//! credential store; the merged snapshot that comes back has no such key. Only
//! when there is no usable store - a headless Linux box, a keychain that will
//! not answer - does it stay here, which is still better than a token
//! the user has to paste again every launch.
//!
//! The interception is in `write` rather than in the command wrapper so that
//! there is one place a secret can leak into this file from, and so that a
//! second caller cannot acquire the old behaviour by accident. The *other*
//! direction is filtered at the commands instead, by `without_token`: `write`
//! and `read` are what `weights` itself calls to find the fallback copy, and a
//! filter down there would hide the token from the only code entitled to it.
//!
//! A Clear the credential store refuses is an `Err`, not a quiet success. The
//! two failures look identical from here - no store at all, or a store that
//! will not let go - and `weights::write_token` is what tells them apart; only
//! the second one can leave a secret behind, and the user has to hear about it.

use std::path::PathBuf;

use serde_json::{Map, Value};

/// Where the file lives, under the app's config directory.
const FILE: &str = "settings.json";

pub fn path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(FILE))
        .map_err(|e| e.to_string())
}

pub fn read(app: &tauri::AppHandle) -> Result<Value, String> {
    let path = path(app)?;
    match std::fs::read(&path) {
        // No file yet is not an error: it is a first launch, and the interface's
        // own defaults are the right answer.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(err) => Err(format!("{}: {err}", path.display())),
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display())),
    }
}

/// The token a patch is asking to store, as a string.
///
/// **Only a string is a token**, and the empty string is the Clear. The
/// interface sends exactly those two - `writeSettings({hfToken: value.trim()})`
/// to save and `{hfToken: ''}` to clear - so anything else is a caller that has
/// gone wrong, and the old reading (`as_str().unwrap_or_default()`) turned
/// `0`, `false`, `null` and `{}` into a silent Clear: a patch with a typo in it
/// would delete the user's token and report success. `null` is refused with the
/// rest rather than made to mean "unset", because a merge patch has no unset
/// (see the module docs) and giving one key a private one would be a second
/// rule for the interface to know.
fn token_of(value: &Value) -> Result<&str, String> {
    value.as_str().map(str::trim).ok_or_else(|| {
        format!(
            "{} must be a string; the empty string clears it",
            crate::weights::TOKEN_SETTING
        )
    })
}

/// Merge a patch and return the whole snapshot.
pub fn write(app: &tauri::AppHandle, mut patch: Value) -> Result<Value, String> {
    // Taken out of the patch before the merge so that no path through this
    // function can write it into the file by accident. See the module docs.
    let token = match &mut patch {
        Value::Object(map) => map.remove(crate::weights::TOKEN_SETTING),
        _ => None,
    };

    let mut current = match read(app)? {
        Value::Object(map) => map,
        // A settings file that is not an object is not repairable by merging
        // into it; start again rather than lose the patch.
        _ => Map::new(),
    };
    if let Value::Object(patch) = patch {
        for (key, value) in patch {
            current.insert(key, value);
        }
    }

    // A refused Clear is reported **after** the file is written, not instead of
    // writing it. The patch that carried the token can carry other settings
    // too, and an early return dropped every one of them to report a failure
    // about a different key.
    let mut refusal = None;
    if let Some(token) = token {
        let token = token_of(&token)?;
        match crate::weights::store_token(token) {
            // The credential store took it, or removed it. Either way no
            // plaintext copy survives - including any this file was still
            // holding from before the migration.
            crate::weights::TokenWrite::Stored => {
                current.remove(crate::weights::TOKEN_SETTING);
            }
            // No usable store, so this file is the store. An empty value is a
            // Clear and removes the key rather than writing `""`.
            crate::weights::TokenWrite::Fallback if !token.is_empty() => {
                current
                    .insert(crate::weights::TOKEN_SETTING.to_owned(), Value::String(token.to_owned()));
            }
            crate::weights::TokenWrite::Fallback => {
                current.remove(crate::weights::TOKEN_SETTING);
            }
            // The store still holds the secret the user asked to destroy. The
            // copy that *can* be destroyed still is - the user asked for that
            // too - and then the press fails, because answering `Ok` would
            // report a deletion that only half happened.
            crate::weights::TokenWrite::NotCleared(err) => {
                current.remove(crate::weights::TOKEN_SETTING);
                refusal = Some(err);
            }
        }
    }

    let merged = Value::Object(current);
    write_file(&path(app)?, &merged)?;
    match refusal {
        Some(err) => Err(format!("the credential store kept the token: {err}")),
        None => Ok(merged),
    }
}

/// Write the whole settings object, atomically and privately.
///
/// One place, because there are two callers and both of them can be holding the
/// Hugging Face token: on a machine with no credential store this file *is* the
/// store, and
/// [`cleaner_core::project::buffers::write_atomic`] creates its temporary file
/// with the process umask - 0644 on a typical Linux box, which is a secret
/// every account on the machine can read.
///
/// The mode is narrowed unconditionally rather than only when a token is
/// present: a file that is 0600 sometimes is a file whose permissions say
/// something about its contents, and settings are small and rarely written, so
/// there is nothing to save by being clever. Not on Windows, where the bits do
/// not mean this and the ACL that does is not `Permissions`' to set.
fn write_file(path: &std::path::Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    // The same temp-and-rename the manifest gets. Settings are small and rarely
    // written, and a truncated settings file on a power cut is a first-launch
    // experience for someone who has been using the application for a year.
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    cleaner_core::project::buffers::write_atomic(path, &bytes).map_err(|e| e.to_string())?;
    restrict(path)
}

/// Narrow a file to its owner. A no-op where the mode bits do not decide.
#[cfg(unix)]
fn restrict(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

/// Remove one top-level key.
///
/// A merge patch cannot express "unset" - that is the module doc's first
/// consequence - so the token migration, which has to take `hfToken` out of a
/// file that already has it, needs a way in that is not a patch. Nothing is
/// written when the key was not there.
pub fn forget(app: &tauri::AppHandle, key: &str) -> Result<(), String> {
    let Value::Object(mut current) = read(app)? else { return Ok(()) };
    if current.remove(key).is_none() {
        return Ok(());
    }
    write_file(&path(app)?, &Value::Object(current))
}

/// The snapshot the interface is allowed to see.
///
/// The seam says the Hugging Face token never
/// travels in this direction, and on a machine with no credential store this
/// file is holding one - so the key is taken out at the boundary rather than
/// relied on to be absent. The fallback is unaffected: `weights::token` reads
/// the file directly and never through this.
fn without_token(mut snapshot: Value) -> Value {
    if let Value::Object(map) = &mut snapshot {
        map.remove(crate::weights::TOKEN_SETTING);
    }
    snapshot
}

#[tauri::command]
pub fn read_settings(app: tauri::AppHandle) -> Result<Value, String> {
    read(&app).map(without_token)
}

#[tauri::command]
pub fn write_settings(app: tauri::AppHandle, patch: Value) -> Result<Value, String> {
    // The answer is a whole snapshot and travels the same direction a read
    // does, so it is filtered by the same rule.
    write(&app, patch).map(without_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam's rule about the token, enforced at
    /// the boundary rather than assumed: on a machine with no credential store
    /// the file really is holding one, and `readSettings` still must not hand
    /// it back.
    #[test]
    fn a_snapshot_that_crosses_the_seam_carries_no_token() {
        let snapshot = serde_json::json!({
            "accelerator": "auto",
            "hfToken": "hf_secret",
            "theme": "dark",
        });
        let filtered = without_token(snapshot);

        assert!(filtered.get("hfToken").is_none(), "the token does not travel this way");
        assert_eq!(filtered.get("accelerator").and_then(|v| v.as_str()), Some("auto"));
        assert_eq!(filtered.get("theme").and_then(|v| v.as_str()), Some("dark"));
        assert!(!serde_json::to_string(&filtered).unwrap().contains("hf_secret"));
    }

    /// A settings file that is not an object is passed through rather than
    /// replaced: the filter has one job and repairing the file is `write`'s.
    #[test]
    fn a_snapshot_that_is_not_an_object_is_left_alone() {
        assert_eq!(without_token(Value::Null), Value::Null);
        assert_eq!(without_token(serde_json::json!([1, 2])), serde_json::json!([1, 2]));
    }

    /// **Only a string is a token, and only the empty string is a Clear.**
    ///
    /// The reading this replaces took `as_str().unwrap_or_default()`, which
    /// made every non-string an empty string and therefore a silent, successful
    /// deletion of the user's token. A patch with `hfToken: null` in it - a
    /// caller that meant to send nothing and sent something - must be refused,
    /// not obeyed.
    #[test]
    fn only_a_string_is_a_token_and_a_non_string_is_refused() {
        assert_eq!(token_of(&serde_json::json!("hf_abc")), Ok("hf_abc"));
        assert_eq!(token_of(&serde_json::json!("  hf_abc  ")), Ok("hf_abc"), "trimmed");
        assert_eq!(token_of(&serde_json::json!("")), Ok(""), "the empty string is the Clear");

        for wrong in [
            serde_json::json!(null),
            serde_json::json!(0),
            serde_json::json!(false),
            serde_json::json!({}),
            serde_json::json!([]),
        ] {
            let refused = token_of(&wrong).expect_err(&format!("{wrong} is not a token"));
            assert!(refused.contains("hfToken"), "{refused}");
        }
    }

    /// The fallback copy of the token is written into a file the rest of the
    /// machine can read, unless something narrows it. On a shared Linux box
    /// `write_atomic` creates 0644 through the umask, so the mode is set after
    /// the rename rather than assumed.
    #[cfg(unix)]
    #[test]
    fn the_settings_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("mc-settings-mode-{}", std::process::id()));
        let path = dir.join("nested").join("settings.json");
        write_file(&path, &serde_json::json!({ "hfToken": "hf_secret" })).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a file holding a token is the owner's alone");

        // And a rewrite does not widen it again.
        write_file(&path, &serde_json::json!({ "theme": "dark" })).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        std::fs::remove_dir_all(&dir).ok();
    }
}
