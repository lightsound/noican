# 0014. Licensing

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the source
location of each comment at the time of the move (commit 397e3df)
is noted above its text.

## activation-binding-recheck

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/StoredLicense.swift` lines 49–51.

Whether the activation was issued for this backend and this Mac.
Anything else must be activated again (with the stored key) before
it can be validated.

## compiled-in-license-configuration

Source: `macos/Sources/NoicanMenuBar/LicenseConfiguration.swift` lines 4–11.

The license server this build talks to. Compiled in rather than read
from Info.plist: an unconfigured build runs unrestricted, and a plist
edit must not be able to turn a shipping build into one.

Placeholders until the Polar organization exists — see
docs/licensing.md ("Polar setup") for where each value comes from.
Until both IDs are UUIDs the app reports "Not configured in this
build" and noise cancellation is not gated.

## default-model-licensing

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 37–40.

Model selected on first launch (also labeled "Default" in the
model list). The default must be a model whose training data is
clear for commercial use (docs/tech-research.md §11, licensing
notes); of the registered models, only `dfn3` is.

## device-id-fail-closed

Source: `macos/Sources/NoicanMenuBar/LicenseModel.swift` lines 110–131.

A salted hash of the hardware UUID: identifies this Mac to the
local record without sending the UUID anywhere.

When the UUID cannot be read, the ID is a random value minted
for this launch — never a constant. A constant fallback would be
shared by every Mac in that state, so a license record carried
to another such Mac (Migration Assistant moves the Keychain)
would be mistaken for that Mac's own, including by
`LicenseController.deactivate`, which would then release the
first Mac's activation on the server. A per-launch ID makes any
stored record read as another Mac's — forgotten locally, never
released. The price on a Mac whose UUID stays unreadable is
real: `refreshIfDue` re-activates the stored key automatically
at every launch, spending a fresh activation slot each time
(orphaned slots are releasable only in the customer portal)
until the key's activation limit refuses further launches, and
no launch can release a previous launch's slot. That state
means IOKit is not reporting a platform UUID at all — broken
IOKit or a VM, not a healthy Mac — and failing closed there is
the point. Persisting the random ID would defeat this:
Migration Assistant would carry it along with the record it is
meant to disown.

## device-identity-migration-detection

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseBackend.swift` lines 31–34.

Opaque per-Mac identifier (a salted hash of the hardware UUID).
Kept locally with the activation to notice a Keychain that moved
to another Mac (Migration Assistant); never used as a server-side
condition, so a logic-board swap cannot lock the customer out.

## device-metadata-contract

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseBackend.swift` lines 39–40.

Extra facts stored with the activation for support (app and macOS
versions). Values must be non-empty.

## keychain-code-identity

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/KeychainLicenseStore.swift` lines 5–9.

Keeps the activation as one generic-password item in the user's login
Keychain. The item's access control list admits the app that created
it, so a Developer ID build reads it back silently across updates
(same designated requirement); an ad-hoc development build is a new
code identity after every rebuild and gets a Keychain access prompt.

## license-activation-slot-reuse

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseController.swift` lines 145–151.

Activates a key the customer entered. When this Mac still holds an
activation (for example after the customer rotated the key in the
portal — activations survive a rotation), the new key is first
tried against it, so no second device slot is spent. The Keychain
is read again first if the last read failed, for the same reason;
only when it stays unreadable does the key take a new slot (the
old one can be released in the customer portal).

## license-backend-portability

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseBackend.swift` lines 3–10.

The license server as the app sees it: activate a key on this Mac,
re-validate that activation, release it. `PolarLicenseBackend` is the
shipping implementation; another provider (Paddle with a small
activation server, Keygen, a self-hosted server holding keys exported
from Polar) replaces it by conforming to this protocol and changing
`identifier` — `LicenseController` then re-activates every stored key
against the new backend on the next launch, so customers never
re-enter their key.

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseBackend.swift` lines 12–14.

Stable name of the backend, persisted with each activation. A
stored activation whose identifier differs was issued by another
backend and is re-activated rather than validated.

## license-deactivation-semantics

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseBackend.swift` lines 24–25.

Releases the device slot. An activation the server no longer
knows counts as released.

## license-error-taxonomy-grace

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseBackend.swift` lines 67–69.

Backend failures, split by what they mean for a customer who already
paid: only a definitive answer from the license server may take a
license away; anything else keeps the offline grace period running.

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseBackend.swift` lines 73–74.

No usable answer: offline, timeout, rate limit, server error, or a
response this build does not understand. `reason` is for the UI.

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseBackend.swift` lines 82–83.

Validation: the key or this Mac's activation is gone — released
from the customer portal, key rotated, refunded, or revoked.

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseBackend.swift` lines 85–86.

Activation refused with the server's explanation: the device limit
is reached, or the key is revoked, disabled, or expired.

## license-foreign-activation

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseController.swift` lines 174–177.

Releases this Mac's device slot and forgets the key. An activation
issued for another Mac (a Keychain carried over by Migration
Assistant) is forgotten locally only: releasing it on the server
would deactivate the other Mac.

## license-gate-live-session-exemption

Source: `macos/NoicanState/Sources/NoicanState/LicenseGate.swift` lines 4–7.

Records whether Preview/On may be started. Deliberately no effect
when the license lapses mid-session: the check runs in the
background (hourly), and cutting the microphone off in the middle
of a call is worse than one more session.

Source: `macos/NoicanState/Sources/NoicanState/LicenseGate.swift` lines 20–23.

The gate on mode taps: a Preview/On tap without a license is
refused in place like an unsafe Preview (mode and engine stay as
they are, the reason shows under the control); Off always passes.
Returns whether the tap was refused.

Source: `macos/NoicanState/Sources/NoicanState/LicenseGate.swift` lines 33–40.

The gate on engine starts, for every path that reaches one (mode
taps, microphone picks, rate-change rebuilds, the failed-switch
fallback). Without a license only a rebuild of a session that is
still live passes — the one exemption: a running session is never
interrupted. A session that already died (runtime stop, failed
start) is not restarted. Returns whether the start was refused;
the refusal leaves the machine and the torn-down engine as they
are.

Source: `macos/NoicanState/Sources/NoicanState/LicenseGate.swift` lines 49–52.

Whether a start claimed from `state` replaces a live transport:
a settled session that is still up, or the fallback of a live
microphone switch (claimed while that switch's attempt, which
carried the working device to return to, is still in flight).

## license-gating

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift` lines 210–213.

Whether the license lets the user start noise cancellation. Gates
Preview/On taps and every engine start except a rebuild of a
session that is still live: a running session is never
interrupted by a license change (see `LicenseGate.swift`).

## license-keychain-read-retry

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseController.swift` lines 50–53.

Whether the last Keychain read failed (locked Keychain at login,
a denied access prompt). Every later entry point reads it again
before acting, so a transient failure does not leave a paying
customer unlicensed for the whole session.

## license-notice-presentation

Source: `macos/Sources/NoicanMenuBar/LicenseSection.swift` lines 61–62.

Offline, the status caption already says the license
still works; the reason is detail, not an error.

## license-offline-grace

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseStatus.swift` lines 27–28.

The last verification attempt could not reach the server; the
license keeps working offline until `graceEndsAt`.

## license-operation-serialization

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseController.swift` lines 3–4.

The operation in flight, if any. At most one runs at a time; requests
made while one runs are ignored (the UI disables its buttons).

## license-processing-gate

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseStatus.swift` lines 35–38.

The one switch for unlicensed behavior: whether noise cancellation
(Preview and On) may be started. Everything else in the app —
Off, the microphone and model pickers, the driver itself — works
regardless of the license.

## license-recheck-scheduling

Source: `macos/Sources/NoicanMenuBar/LicenseModel.swift` lines 8–13.

The app's license shell: builds the `LicenseController` from
`LicenseConfiguration`, publishes its state to the menu, forwards
whether noise cancellation may start to the engine reducer, and
re-checks the license hourly (the continuous clock keeps counting
through sleep, so the first tick after a long sleep comes right away)
and whenever the network comes back.

Source: `macos/Sources/NoicanMenuBar/LicenseModel.swift` lines 80–82.

Re-checks when connectivity returns (not on the monitor's first
report, which the launch check already covers). `refreshIfDue`
only asks the server when a check is actually due.

## license-refresh-policy

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseController.swift` lines 98–101.

Launch and hourly entry point: re-activates a record issued by
another backend or for another Mac with its stored key, and
re-validates one that is due — including a rejected record, so a
mistaken rejection heals by itself.

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseController.swift` lines 118–121.

A healthy record is due when its last success needs renewal. A
rejected or foreign record has no recent success to count from, so
after a definitive "no" it waits one revalidation interval rather
than asking the server again every hour.

## license-revalidation-and-grace

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseStatus.swift` lines 61–62.

A validation older than this is repeated at the next opportunity
(launch or the hourly check).

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseStatus.swift` lines 64–65.

How long a license keeps working without a successful validation,
counted from the last one.

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseStatus.swift` lines 96–97.

Whether the activation should be validated again. A validation
time in the future (the clock was set back) is due as well.

## license-section-folding

Source: `macos/Sources/NoicanMenuBar/LicenseSection.swift` lines 4–8.

The popover's license section. Without a working license the key
field is always visible (it is what the refused mode control points
at); a working license folds into one row that advertises its status
and expands to the key, the last verification, and "Deactivate this
Mac".

## license-state-sync

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseController.swift` lines 27–30.

Runs activation, validation, and deactivation against a
`LicenseBackend` and keeps the Keychain record and the published
`LicenseState` in step. Main-actor bound: it is driven by the UI and
its state feeds the menu directly.

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseController.swift` lines 247–250.

A definitive answer from the server (yes or no) was received:
persist it. A Keychain failure is reported but does not undo the
answer for this session, and the in-memory record stays
authoritative (a later read must not bring back an older item).

## license-status-model

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseStatus.swift` lines 20–21.

The build carries no license-server configuration (development
builds before the Polar organization exists).

## license-unconfigured-build

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseController.swift` lines 59–60.

`backend` is nil when the build has no license-server
configuration; the status is then `.unconfigured`.

## offline-grace-period

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/StoredLicense.swift` lines 22–23.

Local clock time of the last successful activation or validation;
the offline grace period counts from here.

## polar-activation-slot-release

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 133–134.

Activation takes no benefit filter, so a key for another
product got a slot; give it back before refusing.

## polar-api-contract

Source: `macos/NoicanLicensing/Tests/NoicanLicensingTests/PolarLicenseBackendTests.swift` lines 6–10.

Request and response shapes follow Polar's customer-portal license-key
reference (API version 2026-10): POST /v1/customer-portal/license-keys/
{activate,validate,deactivate}. The response bodies below are the
documented examples, trimmed to the fields the backend reads plus a
few it must ignore.

## polar-api-versioning

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 87–97.

Polar's public customer-portal license-key API
(`POST /v1/customer-portal/license-keys/{activate,validate,deactivate}`),
which needs no access token and is meant for desktop clients.

No `Polar-Version` header is sent, deliberately: Polar removes each
dated API version about nine months after release and then answers
requests pinned to it with 404, so every customer on an old build
would run out the grace period and be locked out. Unpinned requests
use Polar's Current version; the decoding below reads only the
handful of fields every version has carried, and anything it cannot
read is `.unavailable` (grace period), never a rejection.

## polar-config-case-folding

Decided 2026-11-27; recorded here because code comments are banned.

`PolarConfiguration` lower-cases the organization and benefit IDs
after trimming them. `isComplete` parses them with
`UUID(uuidString:)`, which accepts either case, while the benefit
check compares strings against the lower-case UUIDs Polar returns:
an ID pasted in upper case configured the build as gated and then
refused every key as belonging to another product. Folding once at
construction keeps every later use — request bodies and
comparisons — on one spelling.

## polar-config-gating

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 48–51.

Whether both IDs are filled in with UUIDs. Anything less leaves the
build unconfigured (ungated) rather than half-scoped: a missing
benefit ID would accept keys for any product of the organization,
and a malformed one would fail every validation.

## polar-environment-separation

Source: `macos/Sources/NoicanMenuBar/LicenseConfiguration.swift` lines 14–15.

Switch to .production for release builds; sandbox keys do not
exist on production and vice versa.

## polar-error-format

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 409–410.

Polar's error body: `{"error": "ResourceNotFound", "detail": "…"}`.
Validation errors (422) carry an array in `detail`, read as nil.

## polar-metadata-limits

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 285–286.

Polar limits metadata keys to 40 characters and values to 1–500;
the label has no documented bound and gets the same one.

## polar-public-config

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 6–8.

Where the app's Polar integration points. Every value is public
(the customer-portal license endpoints take no secret), so it is
compiled into the app.

## polar-response-handling

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 101–103.

The `.unavailable` reason for a 200 response this build cannot
use — undecodable, or missing a field the grant depends on.
One constant so the two cases can never drift apart in wording.

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 163–170.

A field that is present and wrong is definitive evidence
against the key: reject, as before. A field that is absent
proves nothing either way — the response is unsigned, so a
body that merely omits the fields the grant depends on
must not mint one — but absence is not evidence of
revocation either, so it takes the unavailable path (the
offline grace) rather than recording a rejection the
server never made.

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 344–351.

Only the fields the app acts on. Decoding stays lenient — all but
the key's own ID are optional, so a field a later API version drops
does not make the body unreadable — but the grant decision is not
lenient: Polar's response is unsigned, so `status` and `benefit_id`
must be present and correct, and validation must name this Mac's
activation, before any of them counts as evidence for a grant. The
remaining fields (display key, expiry, activation limit) stay
optional in effect as well: none of them decides a grant.

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 375–377.

Polar answers 404 for revoked or disabled keys, so a 200 should
always say `granted`; any other status is refused — and so is a
response without the field, which proves nothing about the key.

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 382–384.

The configured benefit, named in the response: a key for another
product is refused, and a response that does not name the benefit
at all has not shown the key is for this product.

Source: `macos/NoicanLicensing/Tests/NoicanLicensingTests/PolarLicenseBackendTests.swift` lines 257–260.

The response is unsigned, so a body carrying only IDs — the
shape anything interposing on the connection could serve —
must not mint a grant, however forward-compatible that would
be. The fresh activation is given back before refusing.

Source: `macos/NoicanLicensing/Tests/NoicanLicensingTests/PolarLicenseBackendTests.swift` lines 280–282.

Absence downgrades only the absent field to unavailable; it
must not shield a field that is present and definitively
wrong. (licenseKeyJSON's default activation is null.)

## polar-timestamp-parsing

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/PolarLicenseBackend.swift` lines 245–246.

Polar timestamps carry microseconds (`2024-09-02T13:48:13.251621Z`);
the fraction is dropped rather than trusted to a formatter.

## rejection-record-retention

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/StoredLicense.swift` lines 25–27.

Set when the server's last definitive answer was no. The record is
kept (and re-checked) rather than deleted, so a mistaken rejection
heals on the next successful validation.

## stored-license-key-retention

Source: `macos/NoicanLicensing/Sources/NoicanLicensing/StoredLicense.swift` lines 3–6.

The activation as persisted on this Mac (in the Keychain). The key
itself is kept, not just the activation: it is what lets a backend
swap or a migrated Mac re-activate without asking the customer, and
what a definitive rejection can later be re-checked with.
