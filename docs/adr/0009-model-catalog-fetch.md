# 0009. Model catalog, weights and fetching

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the file and
symbol each comment was attached to are noted above its text and
checked by `scripts/adr-links.py`.

## atomic-download-temp-file

Source: `crates/noican-models/src/fetch.rs`, `fetch_model_dyn`.

Write via a temp file so an interrupted download never looks
complete to `is_fetched`; the download streams straight into
it, hashing as it goes.

Source: `crates/noican-models/src/fetch.rs`, `download_to`.

Streams `url` into `tmp`, hashing as it goes, and returns the digest
and byte count. The body never lives in memory: a stopped or corrupt
transfer leaves a partial `tmp` for the caller to remove, never a
file that looks complete.

## download-size-cap

Source: `crates/noican-models/src/fetch.rs`, `FetchError::TooLarge`.

A download grew past the maximum plausible size of a weights
file, so it was aborted before the digest check could run.

Source: `crates/noican-models/src/fetch.rs`, `MAX_DOWNLOAD_BYTES`.

Hard ceiling on one downloaded file. The largest legitimate weights
are tens of MB, so this never fires on a real model; it exists
because the digest is only compared after the whole body lands, and
without a cap a malicious or broken endpoint could fill the disk
before that check gets a chance to reject the bytes.

Source: `crates/noican-models/src/fetch.rs`, `copy_body`.

The streaming body of [`download_to`], split out so the size cap can
be tested without a server. Aborts with [`FetchError::TooLarge`]
once more than `limit` bytes have arrived.

Source: `crates/noican-models/src/fetch.rs`, `tests::copy_body_enforces_the_size_cap`.

The download cap aborts an oversized body and passes a body at
or under the limit through with its digest.

## download-timeout-bounds

Source: `crates/noican-models/src/fetch.rs`, `download_agent`.

The fetcher agent, configured once: hard bounds on every phase of a
download so a stalled or trickling server can never hang a fetch (or
the engine start it blocks) forever. Connect and header waits stay
short; the body deadline is generous — the largest weights are tens
of MB, so 15 minutes covers very slow links — and resets per call,
so a redirect chain gets its own allowance per leg.

## fetch-offline-test-fixture

Source: `crates/noican-models/src/fetch.rs`, `tests::VERIFY_SPEC`.

A spec whose file needs no network: the pinned digest is the
SHA-256 of `b"correct weights"`, and the URL is unroutable, so
any real download attempt fails instantly and offline.

## fetch-verification-tests

Source: `crates/noican-models/src/fetch.rs`, `tests::fetch_verifies_present_files_against_the_pinned_digest`.

A present file whose bytes match the pinned digest is reported
as present with no download attempt; one whose bytes do not is
detected and re-downloaded (surfacing as a fetch error here
because the URL is unroutable — the progress lines are what
prove the mismatch was caught).

Source: `crates/noican-models/src/fetch.rs`, `tests::verify_model_gates_on_the_pinned_digest`.

The load path's gate: a pinned file that is present and correct
verifies; a substituted one is a checksum error; an absent one
is reported missing. Nothing is downloaded or repaired.

Source: `crates/noican-models/src/fetch.rs`, `tests::verify_model_recurses_into_dependencies`.

Verification recurses into dependencies: `hush-48k` has no files
of its own, so its gate is exactly the pinned tarball of `hush`
it depends on — missing and substituted states of that file must
both be reported under the dependency's identity.

## hf-token-optional-auth

Source: `crates/noican-models/src/fetch.rs`, `hf_token`.

Returns a Hugging Face access token from the environment
(`NOICAN_HF_TOKEN` or `HF_TOKEN`), if any. Optional: every registry
entry hosted there is public, but authenticated requests get higher
rate limits and can reach gated repos.

## model-backend-selection

Source: `crates/noican-models/src/lib.rs`.

Model registry, weight fetching, and stage implementations.

Every supported model is described by a [`manifest::ModelSpec`] and
implemented as a [`noican_core::FrameProcessor`], wrapped into a
[`noican_core::Stage`] by [`factory::create_stage`] so the engine and
CLI can treat all models uniformly (docs/tech-research.md §12).

Inference backends: ONNX Runtime (`ort`) for DPDFNet and UL-UNAS;
tract (via the upstream `deep_filter` crate) for the
DeepFilterNet-architecture models (DeepFilterNet3, Hush), whose
three-graph exports are only streamable through tract-pulse.

## model-catalog-projection

Source: `crates/noican-ffi/src/lib.rs`, `noican_model_count`.

Number of runtime-selectable models (bypass included), taken from the
registry catalog.

Source: `crates/noican-ffi/src/lib.rs`, `noican_model_rating`.

One picker rating for the model at `index`, 0–5 with "more is better".

`trait_id` follows `NoicanModelTrait` in noican.h: 0 = noise removal,
1 = voice quality, 2 = responsiveness (inverse latency),
3 = efficiency (inverse compute cost).

Returns -1 for an invalid index or trait selector.

## model-catalog-single-source

Source: `crates/noican-models/src/factory.rs`, `CatalogEntry`.

One user-selectable entry of the model catalog.

Covers the built-in bypass and every registry stage. This is the single
source UIs project their model list from (via the C ABI); nothing about
the catalog is defined elsewhere.

Source: `crates/noican-models/src/factory.rs`, `CatalogEntry::id`.

Stable identifier, valid as [`create_stage`] input.

## model-depends-on

Source: `crates/noican-models/src/manifest.rs`, `ModelSpec::files`.

Files required at runtime (empty when the model is embedded in the
binary or when everything comes from [`ModelSpec::depends_on`]).

Source: `crates/noican-models/src/manifest.rs`, `ModelSpec::depends_on`.

Ids of registry models whose files this model also needs at
runtime (a composite stage built around another entry's weights).
Fetching and fetch-status checks follow these transitively; the
stage factory resolves their file paths through the dependency's
own spec, so the weights live in one place on disk.

Source: `crates/noican-models/src/manifest.rs`, `ModelSpec::dependencies`.

The registry entries named in [`ModelSpec::depends_on`] (direct
dependencies only; the registry keeps dependency chains one level
deep, which a unit test pins).

Source: `crates/noican-models/src/manifest.rs`, `ALL_MODELS`.

Runs the `hush` weights inside a 48 kHz band-split wrapper (see
`stages::hush_wideband`); no files of its own.

## model-digest-verification

Source: `crates/noican-models/src/fetch.rs`, `FetchError::Missing`.

A file with a pinned digest is absent (verification only; the
fetcher itself downloads missing files instead of erroring).

Source: `crates/noican-models/src/fetch.rs`, `FetchError::Missing`.

File identity, as `<model id>/<file name>`.

Source: `crates/noican-models/src/fetch.rs`, `is_fetched`.

True when every file of `model` — and of every entry it
[depends on](ModelSpec::depends_on) — is already present under
`models_dir`.

Presence is the only thing checked: this runs on hot paths (the
picker's "fetched" status, the CLI's default model set), so
re-hashing every file would be too expensive. Integrity is enforced
separately: [`fetch_model`] re-verifies and repairs pinned files
whenever it runs, and [`verify_model`] gates the load path.

Source: `crates/noican-models/src/fetch.rs`, `verify_model`.

Verifies that every file of `model` — and of every entry it
[depends on](ModelSpec::depends_on) — that carries a pinned digest
is present under `models_dir` and matches that digest.

Unlike [`fetch_model`] this never downloads or repairs: it is the
load path's gate, called by [`crate::create_stage`] so weight bytes
are never handed to a parser unverified, however they were placed
in `models_dir`. Files without a pinned digest are not checked
(presence is all that can be said about them; a missing one fails
in the stage constructor as before).

# Errors

Returns [`FetchError::Missing`] for an absent pinned file,
[`FetchError::Checksum`] for a digest mismatch, and
[`FetchError::Io`] when a present file cannot be read.

Source: `crates/noican-models/src/fetch.rs`, `fetch_model`.

Downloads any missing files of `model` into `models_dir`, verifying
pinned SHA-256 digests.

Entries the model [depends on](ModelSpec::depends_on) are fetched
first, into their own directories. A present file with a pinned
digest is re-verified and re-downloaded when it does not match, so
corrupt or wrongly-placed files (including manually placed ones)
are repaired here instead of surfacing as a load error later.
Calls `progress` with a human-readable line per file event.

# Errors

Returns [`FetchError`] on network, filesystem, or checksum failure.

Source: `crates/noican-models/src/fetch.rs`, `fetch_model_dyn`.

A file without a pinned digest can only be checked for
presence; a pinned one is re-verified so corrupt or
wrongly-placed files are repaired instead of reported as
fetched (is_fetched stays presence-only for hot paths).

## model-download-sources

Source: `crates/noican-models/src/manifest.rs`, `ALL_MODELS`.

Not on the sherpa-onnx GitHub release yet (docs are ahead of
the release); the official ceva-ip Hugging Face repo hosts it.

## model-fetch-skip-semantics

Source: `crates/noican-models/tests/model_integration.rs`, `run_model`.

`is_fetched` is true for embedded models (no files) and follows
`depends_on`, so composites skip when their dependency is missing.

## model-id-stability

Source: `crates/noican-models/src/manifest.rs`, `ModelSpec::id`.

Stable identifier (CLI argument, config value, UI key).

## model-integration-tests-opt-in

Source: `crates/noican-models/tests/model_integration.rs`.

Integration tests that exercise real model weights.

These are `#[ignore]`d because they need the weights downloaded first
(CI runs lint + unit tests only). Locally:

```text
cargo run -p noican-cli --release -- fetch
cargo test -p noican-models --test model_integration -- --ignored
```

The models directory defaults to `models/` at the workspace root and can
be overridden with `NOICAN_MODELS_DIR`.

## model-pick-revert-path

Source: `macos/Sources/NoicanMenuBar/AppState.swift`, `AppState.selectModel`.

User-initiated model pick (the Picker's binding setter).
Programmatic reverts happen inside the reducer, which never
re-enters this path — so they cannot wipe the failure message
they accompany.

## model-picker-ratings

Source: `crates/noican-models/src/traits.rs`.

Picker-facing model characteristics: four 0–5 ratings, a one-line
tagline, and a detail string for tooltips.

Normal users cannot choose between model code names, so every catalog
entry carries a comparable profile. The ratings follow one rule:
**every axis points the same way — more filled is better** — so a bar
never needs interpreting (latency becomes "responsiveness", CPU cost
becomes "efficiency").

Basis, so the numbers stay honest:
- `responsiveness` is derived from the *measured* engine-reported
  algorithmic latency (`Stage::latency_samples`, 2026-08-27: Hush
  22.5 ms, UL-UNAS 34.5 ms, DFN3 40 ms, DPDFNet 60 ms; 2026-09-23:
  DFN3-LL 20 ms): 5 ≤ 25 ms, 4 ≤ 35 ms, 3 ≤ 45 ms, 2 ≤ 65 ms.
- `efficiency` is derived from parameter count (weight-file size):
  5 ≤ 0.25 M, 4 ≤ 1.5 M, 3 ≤ 2.5 M, 2 above.
- `voice_quality` is the output bandwidth: 48 kHz native scores 4
  (processing always costs some naturalness; only the passthrough
  reference scores 5), 16 kHz telephony-band models score 2. A
  composite that restores the full band around a 16 kHz core
  (`hush-48k`) scores as a 48 kHz model: what the listener hears is
  the full band.
- `noise_removal` is editorial, anchored by the 2026-08-27 hardware
  run: DPDFNet8 / DeepFilterNet3 / Hush suppressed transient clicks
  (trackpad, keyboard); the lighter models let them through.

The raw numbers behind the ratings live in each entry's `details`
string, surfaced as a tooltip.

Source: `crates/noican-models/src/traits.rs`, `ModelTraits`.

Picker-facing profile of one catalog entry. All ratings are 0–5 with
"more is better"; 0 means the axis does not apply (the passthrough
removes nothing).

Source: `crates/noican-models/src/traits.rs`, `ModelTraits::noise_removal`.

How much noise disappears, transient clicks included (editorial,
hardware-anchored).

Source: `crates/noican-models/src/traits.rs`, `ModelTraits::voice_quality`.

How natural the voice stays (native rate: 48 kHz high, 16 kHz low).

Source: `crates/noican-models/src/traits.rs`, `ModelTraits::rated`.

Compact profile constructor for the [`PROFILES`] table; ratings in
removal / quality / responsiveness / efficiency order.

Source: `crates/noican-models/src/traits.rs`, `ModelTraits::for_id`.

The profile for a catalog id. Unknown ids (which the catalog never
produces) fall back to an unrated profile rather than panicking.

## model-picker-revert-path

Source: `macos/Sources/NoicanMenuBar/MenuView.swift`, `MenuView.modelSection`.

User picks route through selectModel; programmatic
reverts write the reducer state directly and must not
re-enter the apply path (they would wipe the failure
message they accompany). Hover any row for the model's
profile card.

## model-selection-behaviour

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift`, `AppReducer.modelSelected`.

A model pick applies immediately when the engine runs; while Off it
only updates the selection (and un-blames a stale failure).

## model-selector-hover-card

Source: `macos/Sources/NoicanMenuBar/ModelSelector.swift`, `ModelSelector`.

The model list with hover profile cards.

Every catalog entry renders as a Control-Center-style row (same
chrome as the microphone list above). Hovering a row pops the
model's profile card out beside that row — four dot-scale ratings
whose axes all point the same way (more filled = better) plus the
raw facts — so a normal user can compare models without knowing
their names.

One card serves the whole list: it appears after a short delay on
the first hover, then **stays up and follows the pointer from row to
row** (position and content update in place — re-presenting per row
made the card blink), and hides after a short grace period once the
pointer leaves the rows (the grace also bridges the gaps between
rows). SwiftUI's `.popover` cannot move while presented, so the card
is hosted by [`HoverCardPresenter`]'s AppKit popover instead.

This replaces a system `Picker`: AppKit menus cannot host hover
tracking or custom row views, so the rows are ordinary SwiftUI
buttons. All text and ratings come from the Rust registry, never
from the UI.

Source: `macos/Sources/NoicanMenuBar/ModelSelector.swift`, `ModelSelector.hoverTransition`.

Debounces the first appearance (skimming into the list must not
flash the card) and the disappearance (row-to-row gaps must not
blink it).

Source: `macos/Sources/NoicanMenuBar/ModelSelector.swift`, `ModelRow`.

One selectable model row: checkmark and name, with the
native-menu-style hover highlight. No per-row descriptions — the
hover card carries the profile, so trailing text would repeat it;
the only annotation is "Default" on the first-launch model. Hover
enter/leave is reported upward — the shared profile card belongs to
the list, not the row.

Source: `macos/Sources/NoicanMenuBar/ModelSelector.swift`, `HoverCardPresenter`.

AppKit-backed presenter for the hover profile card.

SwiftUI's `.popover` cannot reposition while presented — changing
its anchor or item dismisses and re-presents, which blinked the card
on every row change. `NSPopover` can: its `positioningRect` may be
updated live. This representable spans the list as an invisible,
SwiftUI-coordinate (flipped) anchor view and drives one popover:
show on the first hover, move + swap content in place across rows,
close on leave. `.applicationDefined` behavior keeps AppKit from
auto-dismissing it (the hover state machine owns its lifetime).

Source: `macos/Sources/NoicanMenuBar/ModelSelector.swift`, `HoverCardPresenter.Coordinator`.

Owns the popover across SwiftUI updates. Presentation is
deferred one main-actor hop: `updateNSView` runs inside SwiftUI's
view update, where presenting an AppKit window synchronously can
re-enter layout. The latest desired state wins (stale hops apply
whatever is current, so rapid hover changes coalesce).

Source: `macos/Sources/NoicanMenuBar/ModelSelector.swift`, `HoverCardPresenter.Coordinator.hide`.

close() is synchronous; performClose would defer, and
a card outliving its rows is what this must prevent.

Source: `macos/Sources/NoicanMenuBar/ModelSelector.swift`, `HoverCardPresenter.Coordinator.ensurePopover`.

No show/hide animation: an animated close is asynchronous
and can overlap the menu's own resize when the list
collapses while a card is up (the card must be gone before
the rows are).

Source: `macos/Sources/NoicanMenuBar/ModelSelector.swift`, `FlippedAnchorView`.

Invisible anchor with SwiftUI's top-left origin, so row frames
measured in SwiftUI coordinates serve directly as AppKit
positioning rects.

## model-switch-fade

Source: `crates/noican-ffi/src/lib.rs`, `SWITCH_FADE_SAMPLES`.

Fade half-length for model switches: 5 ms at the 48 kHz engine rate.

## model-switch-nil-session

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift`, `EngineTransition.switchingModel`.

Model switch. `session` is nil when no live transport is known
(the switch then fails fast in the engine with a clear reason,
which renders under the Model picker like any other switch
failure).

## model-weight-digest-verification

Source: `crates/noican-models/src/factory.rs`, `create_stage`.

Instantiates the stage for `id`, loading weights from `models_dir`
(fetch them first with [`crate::fetch::fetch_model`]).

Before anything is loaded, every weight file that carries a pinned
digest is verified against it ([`crate::fetch::verify_model`]), so
bytes placed in `models_dir` by hand — or corrupted after download —
are rejected here instead of reaching a parser unverified.

# Errors

Returns [`StageError::Unsupported`] for unknown ids and
[`StageError::Inference`] when weights are missing, fail
verification, or fail to load.

## model-weight-pinning

Source: `crates/noican-models/src/manifest.rs`.

Static registry of supported models and their downloadable weights.

Weights are never committed to the repository; they are fetched from the
official distribution points recorded in docs/tech-research.md §14 (see
[`crate::fetch`] and docs/models.md). SHA-256 digests are pinned to the
artifacts verified during Phase 0 bring-up.

Source: `crates/noican-models/src/manifest.rs`, `FileSpec::sha256`.

Expected SHA-256 (lowercase hex), when pinned.

Source: `crates/noican-models/src/manifest.rs`, `ALL_MODELS`.

Commit-pinned (tag v0.5.6, the `deep_filter` revision the
workspace builds against); not part of the crate's embedded
default model.

Source: `crates/noican-models/src/manifest.rs`, `ALL_MODELS`.

Commit-pinned permalink (the repo has no releases).

## model-weights-not-in-repo

Source: `crates/noican-models/tests/model_integration.rs`, `run_model`.

Treated as a skip: weights are intentionally not part of the repo.

## registry-dispatch-completeness

Source: `crates/noican-models/src/factory.rs`, `tests::every_registry_entry_has_a_stage_implementation`.

Every registry entry reaches the picker and the CLI defaults
unfiltered, so each must dispatch to a stage: without weights the
only acceptable failure is the missing file, never the
no-implementation fallthrough. Embedded models (no files, no
dependencies) are skipped because they would build for real.

## registry-driven-model-catalog

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `ModelRatings`.

Picker ratings of one model, 0–5 with "more is better" on every axis
(the registry inverts latency into responsiveness and compute cost
into efficiency so a longer bar always means better).

Source: `macos/Sources/NoicanMenuBar/RustEngine.swift`, `ModelInfo`.

A model entry exposed by the Rust registry (never hardcoded here).

## send-model-thread-confinement

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `SendModel`.

Moves a whole [`DfTract`] across threads.

`DfTract` is not `Send` because tract's stateful plans hold `Rc`s
internally. Those `Rc`s are created by and confined to the `DfTract`
instance: we never clone the model (its `Clone` impl is unused here),
never hand out references to its internals, and only access it through
`&mut self` from one thread at a time — the whole object graph migrates
together, which is sound for `Rc`. Upstream ships the same pattern in
its C API (`DFState` is used from arbitrary caller threads).

Source: `crates/noican-models/src/stages/dfn_tract.rs`, `SendModel`.

SAFETY: see the type-level comment — no `Rc` inside the wrapped value
can be observed from more than one thread because the value is only
accessible via `&mut` and is never cloned or shared.

## type-erased-recursion

Source: `crates/noican-models/src/fetch.rs`, `verify_model_dyn`.

Type-erased body of [`verify_model`] (recursion over dependencies,
mirroring [`fetch_model_dyn`]).

Source: `crates/noican-models/src/fetch.rs`, `fetch_model_dyn`.

Type-erased body of [`fetch_model`] (recursion over dependencies must
not re-instantiate the generic closure parameter).

## weight-digest-verification

Source: `crates/noican-ffi/src/lib.rs`, `prepare_stage`.

Registry stages may need weights; the bypass (not in the registry's
ModelSpec list) needs nothing. fetch_model runs unconditionally,
not only when files are missing: it no-ops per file whose pinned
digest already matches, and it repairs a present-but-corrupt file
here instead of letting it reach create_stage, which verifies
digests and would refuse the stage. Stage preparation is a
user-initiated action, so the re-hash costs far less than a
refused start.
