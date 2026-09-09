#!/usr/bin/env bash
# Phase 0, spike 1: the signing matrix.
#
# Builds the probe, signs it four ways, and loads an ONNX Runtime that was never
# part of the signed bundle. What it answers: which of those four an app that
# downloads its runtime after install can actually use.
#
# Needs a codesigning identity. Pass one, or the first valid identity is used:
#   ./matrix.sh ["Developer ID Application: … (TEAMID)"]
#
# The runtime is not downloaded here; run scripts/fetch-runtime.sh first.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$root"

identity="${1:-}"
if [[ -z "$identity" ]]; then
	identity=$(security find-identity -v -p codesigning | sed -n '1s/.*"\(.*\)"/\1/p')
fi
if [[ -z "$identity" ]]; then
	echo "no codesigning identity found; pass one as \$1" >&2
	exit 2
fi
echo "identity: $identity"

runtime="$root/runtimes/onnxruntime-osx-arm64-1.28.0/lib/libonnxruntime.dylib"
if [[ ! -f "$runtime" ]]; then
	echo "no runtime at $runtime - run scripts/fetch-runtime.sh" >&2
	exit 2
fi

work="$root/target/spike-signing"
rm -rf "$work"
mkdir -p "$work/plain" "$work/quarantined" "$work/teamsigned"

# Case 5 deliberately marks a copy of the runtime as quarantined. A file left in
# that state makes Gatekeeper raise "Apple could not verify … is free of
# malware" the next time anything touches it, so the whole working directory
# goes away on exit however this script ends.
cleanup() {
	xattr -dr com.apple.quarantine "$work" 2>/dev/null || true
	rm -rf "$work"
}
trap cleanup EXIT INT TERM

cargo build --release -p spike-ort-loaddynamic

# The filename matters: the dylib's own `LC_LOAD_DYLIB` names
# `@rpath/libonnxruntime.1.dylib`, so a copy under any other name fails to
# resolve its self-reference and the failure looks like a signing failure.
cp "$runtime" "$work/plain/libonnxruntime.dylib"
cp "$runtime" "$work/quarantined/libonnxruntime.dylib"
xattr -w com.apple.quarantine "0083;00000000;spike;" "$work/quarantined/libonnxruntime.dylib"
cp "$runtime" "$work/teamsigned/libonnxruntime.dylib"
codesign --force --options runtime -s "$identity" "$work/teamsigned/libonnxruntime.dylib" >/dev/null 2>&1

probe() {
	local name="$1" entitlements="$2" dylib="$3"
	local bin="$work/$name"
	cp target/release/spike-ort-loaddynamic "$bin"
	if [[ "$entitlements" == "adhoc" ]]; then
		# arm64 refuses to execute an unsigned binary at all, so the baseline is
		# ad-hoc - the same signature `cargo build` leaves behind.
		codesign --force -s - "$bin" >/dev/null 2>&1
	else
		codesign --force --options runtime \
			--entitlements "$root/spikes/ort-loaddynamic/signing/$entitlements" \
			-s "$identity" "$bin" >/dev/null 2>&1
	fi
	echo
	echo "── $name"
	"$bin" "$dylib" 2>&1 | grep -E "^(dlerror|loaded|relu|SPIKE)" || true
}

probe "1-adhoc-no-hardening"        adhoc                             "$work/plain/libonnxruntime.dylib"
probe "2-hardened-no-entitlements"  no-entitlements.plist             "$work/plain/libonnxruntime.dylib"
probe "3-hardened-libval-disabled"  disable-library-validation.plist  "$work/plain/libonnxruntime.dylib"
probe "4-hardened-dylib-resigned"   no-entitlements.plist             "$work/teamsigned/libonnxruntime.dylib"
probe "5-hardened-quarantined"      disable-library-validation.plist  "$work/quarantined/libonnxruntime.dylib"
