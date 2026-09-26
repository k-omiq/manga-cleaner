# Windows and Linux installation validation, 2026-09-26

## Environments and result boundaries

The development host was a Mac17,3 MacBook Air with an Apple M5, 32 GiB RAM, macOS 27.0 (26A428), arm64, Rust 1.96.0, Node 26.3.0 and npm 11.16.0. Its graphics device reported Metal 4. Colima ran an Ubuntu 22.04.5 LTS `linux/amd64` container under QEMU emulation. The container reported `x86_64`/`amd64`, with neither `/dev/dri` nor `/dev/nvidia0`. No Windows machine, Windows VM, Linux desktop, or Linux GPU was available. The container was **not** an installed Linux desktop test.

No current-branch NSIS, AppImage or Debian package was produced on this host. The Windows and Linux release workflow builds on native GitHub runners. A manual run can now set `validate_only` to build and inspect artifacts while skipping the publish job. That run was not dispatched from this local branch, so the new package checks are committed gates awaiting a build; they are not reported as passing runs.

## Confirmed defect and fixes

The local `scripts/release.sh` path used a nonexistent `npm run tauri build` command, looked for bundles under `src-tauri/target` even though this workspace emits them under `target/<triple>`, and did not stage the frozen cloud helper or `uv`. A release from that path could fail before publishing or produce an installer without setup helpers. It now freezes and smoke tests the cloud helper, stages managed Python's `uv`, builds with `npx tauri build --target <triple>`, and reads the correct bundle directory. Its Python 3.11+ requirement is explicit.

`src-tauri/build.rs` now refuses release builds unless both generated helper binaries and their `bundle.externalBin` declarations exist. The tagged release workflow now checks the NSIS listing for both helpers, the four app-local VC++ runtime DLLs, SAM setup sources, and the updater signature. For Linux it extracts both the `.deb` and `.AppImage`, checks executable helpers and the bundled FLUX/SAM source files in each, prints Debian dependencies, and checks the AppImage signature. These checks inspect artifacts; they do not run model installation or inference.

## Commands and observed evidence

- `bash -n scripts/release.sh`, `git diff --check`, and Ruby `YAML.load_file('.github/workflows/release.yml')` passed.
- `colima start --arch aarch64 --cpu 4 --memory 8 --disk 20` succeeded. `docker run --rm --platform linux/amd64 ubuntu:22.04 ...` reported Ubuntu 22.04.5 LTS, `x86_64`, `amd64`, and no DRI or NVIDIA device. This verifies an emulated container is available; it does not verify a package or app launch.
- A fresh **macOS** release `.app` was built with `npx tauri build --target aarch64-apple-darwin --bundles app` after staging the existing frozen provisioner and a local `uv` binary as external binaries, and temporarily disabling updater artifacts because the signing key was not supplied. It completed in 1m 11s and produced a 102 MB app. The app contained executable `manga-cleaner-provisioner` and `manga-cleaner-uv`, `sidecar/bootstrap.py`, `sidecar/pyproject.toml`, and the SAM bootstrap, export, environment and synthetic fixture files. The packaged provisioner returned `ok: true`, `frozen: true` from `--self-check`; the packaged `uv` returned version 0.12.10. This confirms the shared Tauri resource map on macOS only. The normal release workflow pins `uv==0.12.19`; the locally available Homebrew binary was used solely for this package layout check. The temporary Tauri config edits were restored after the build.
- The first attempt using only Tauri's `--config` override failed the existing `build.rs` helper gate because that gate reads `src-tauri/tauri.conf.json` directly. The successful build temporarily patched that file and restored its original bytes in a `finally` block. This did not change the committed config.

## Unverified acceptance checks

| Check | Status and concrete reason |
| --- | --- |
| Build and inspect current NSIS, AppImage, Debian packages | Unverified locally: no Windows host or native Linux build toolchain; only the future CI artifact gates were added. |
| Clean Windows install, launch, update, uninstall/reinstall, WebView2 and VC++ loader resolution | Unverified: no Windows machine or VM. Bundle listing checks cannot establish loader or GUI behavior. |
| Windows path with spaces or non-ASCII characters | Unverified: no Windows installer run. |
| Linux `.deb` or AppImage startup, library loading, updater, desktop integration | Unverified: the emulated container has no package artifact or display session. |
| Managed Python, model setup and persistence after clean install | Unverified on Windows/Linux: no installed app or clean user profile was run. macOS packaging showed the setup executable and source files only. |
| Interrupted downloads, retry, offline errors, insufficient disk | Unverified on Windows/Linux: these need installation and controlled network/storage tests. |
| Windows DirectML/CUDA and Linux CUDA/WebGPU inference | Unverified: no corresponding OS and GPU device; the Linux container exposed no GPU. |

The release workflow's `validate_only` path should be run before using these packages as installation evidence. A physical Windows and Linux desktop pass remains necessary for clean-install, launch, model setup, GPU and update claims.
