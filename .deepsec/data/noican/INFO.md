# noican — project context

> **Context note (2026-11-15):** this INFO.md was written for the
> November 2026 deepsec audit run (executed by Muse on a snapshot
> of `main` @ `083891a8`) and was carried into this repository
> unchanged except for this note. It describes the codebase as of
> that snapshot; update it when the architecture moves.


## What this codebase does

noican is a local, single-user macOS voice-isolation / noise-suppression
tool. A Rust workspace (crates/noican-core, noican-coreaudio, noican-models,
noican-ffi, noican-cli) captures microphone audio through CoreAudio, runs
ML noise-suppression models (DeepFilterNet / dpdfnet / hush via tract and
ONNX Runtime), and feeds the cleaned audio to a virtual microphone device.
A Swift menu-bar app (macos/) drives the Rust core through the C ABI in
crates/noican-ffi (header: macos/Sources/CNoican/include/noican.h) and
handles license activation against Polar.

**There is no web server, no HTTP listener, no user accounts or auth
system, and no other people's data.** The only data at stake is the
user's own microphone audio, their license key, and the integrity of
code/models the app downloads and runs. Do not report web-framework
vulnerability classes (SQLi, XSS, CSRF, session issues); they do not
exist here.

## Trust boundaries that matter

- **Model downloads** — `crates/noican-models/src/fetch.rs` fetches model
  files over HTTPS with ureq; `manifest.rs` parses the model manifest and
  pins SHA-256 hashes. Questions: is TLS actually enforced, can a
  redirect or a tampered manifest/file substitute a different model than
  the one hashed, where do downloaded bytes land on disk (paths derived
  from remote data?), and are size limits enforced before allocation?
- **License validation** — `macos/NoicanLicensing/` talks to Polar over
  URLSession (`PolarLicenseBackend.swift`), stores the key in the
  Keychain (`KeychainLicenseStore.swift`, `StoredLicense.swift`), and
  `LicenseController.swift` decides activation state. Questions: is the
  server response validated before it is trusted, can a network attacker
  (or a compromised/MITM'd connection) fake an "activated" state, and is
  the key ever logged or stored in plaintext outside the Keychain?
- **FFI boundary** — `crates/noican-ffi/src/lib.rs` is a hand-written C
  ABI (the crate with by far the most `unsafe`). Swift passes pointers,
  lengths, and C strings in; Rust hands back buffers Swift reads.
  Questions: length/capacity mismatches, null handling, use-after-free
  or double-free of returned buffers, panics crossing the ABI, and
  integer overflow in size computations.
- **CoreAudio FFI** — `crates/noican-coreaudio/src/macos/` wraps
  AudioUnit / AudioDevice APIs in `unsafe` blocks from realtime audio
  callbacks. Questions: buffer sizes trusted from the OS vs. computed,
  callback-reentrancy assumptions, and unchecked casts of audio buffers.
- **Untrusted file parsing** — `crates/noican-cli/src/audio.rs` decodes
  user-supplied audio files with symphonia / hound; `noican-models`
  loads model files from disk (tract / ort). Malformed-file robustness
  (panics, unbounded allocation) matters more than classic injection.

## Project-specific patterns to flag

- Any download, extraction, or file write whose destination path is
  built from manifest/remote content without confinement to the model
  directory.
- Hash or signature checks that are computed but not compared, compared
  on a different byte stream than the one written/loaded, or skipped on
  an error path (fail-open).
- `unsafe` blocks that trust a length, capacity, or pointer supplied
  across the FFI boundary without validation.
- License-state transitions that trust a cached or local value after a
  failed network validation (fail-open activation).

## Known false-positives / intentional design

- The realtime audio path deliberately avoids allocation and locking in
  callbacks; lock-free rings (rtrb) and atomics there are intentional.
- `unsafe` in the CoreAudio/FFI crates is unavoidable by design; flag
  specific unsound uses, not the mere presence of `unsafe`.
- The CLI (`noican-cli`) is a developer/eval tool run by the user on
  their own files; "the user can hurt themselves with their own file"
  is not a finding unless a *third party's* file (e.g. a shared audio
  file or model) can crash or compromise the user who merely opens it.
