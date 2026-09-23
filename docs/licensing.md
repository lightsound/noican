# License activation

Noican is sold as a one-time purchase through Polar (merchant of record).
Each purchase grants a Polar license key; the app activates it on the Mac,
re-validates it periodically, and lets the customer release the Mac again.
The app talks to Polar's public customer-portal license-key endpoints, which
take no access token, so no secret ships in the app.

## Unlicensed behavior

Without a valid license, **Preview and On are refused** with the message
"Noican needs a license to run — enter your license key below." Everything
else works: Off, the microphone and model pickers, the strength slider,
start at login, and the virtual driver itself (the driver is GPL-3.0 and is
never gated; see `LICENSE.driver`).

- A license that lapses or is rejected while noise cancellation runs does
  not stop it. The check runs in the background, and cutting the microphone
  off mid-call is worse than one more session. Rebuilds of that live session
  (microphone switch, Bluetooth rate change, the fallback after a failed
  switch) still pass; the next Preview/On tap, or any restart of a session
  that already stopped (unplugged microphone, audio stall), is refused.
- Builds whose configuration is still the placeholder show "License · Not
  configured in this build" and are not gated, so development and device
  testing keep working before the Polar organization exists.

The whole policy is one switch, `LicenseStatus.allowsProcessing`
(`macos/NoicanLicensing/Sources/NoicanLicensing/LicenseStatus.swift`). A
trial or a reminder-only mode changes that property (and, for a trial, adds
a status); the reducer gate (`macos/NoicanState/Sources/NoicanState/LicenseGate.swift`)
and the UI follow it.

| Status | Meaning | Preview/On |
|---|---|---|
| `unconfigured` | No Polar configuration compiled in | allowed |
| `unlicensed` | No activation on this Mac | refused |
| `active` | Verified within the last 24 hours, or not yet due | allowed |
| `offline` | The last check could not reach the server; within the grace period | allowed |
| `verificationRequired` | 30 days without a successful check | refused |
| `rejected` | Polar said no (revoked, refunded, released from the portal, key rotated, expired, other product) | refused |

## Design

| Piece | Where |
|---|---|
| Backend abstraction (`LicenseBackend`), rejection reasons | `macos/NoicanLicensing/Sources/NoicanLicensing/LicenseBackend.swift` |
| Polar implementation (`PolarLicenseBackend`) | `…/PolarLicenseBackend.swift` |
| Stored record, Keychain store | `…/StoredLicense.swift`, `…/KeychainLicenseStore.swift` |
| Status, grace period, revalidation interval | `…/LicenseStatus.swift` |
| Activate / validate / deactivate flows | `…/LicenseController.swift` |
| Placeholder configuration | `macos/Sources/NoicanMenuBar/LicenseConfiguration.swift` |
| App shell (device identity, hourly check) and UI | `macos/Sources/NoicanMenuBar/LicenseModel.swift`, `LicenseSection.swift` |

`NoicanLicensing` is a standalone Swift package like `NoicanState`; its tests
(`swift test --package-path macos/NoicanLicensing`) run against a mock backend
and a scripted HTTP transport, with no network and no Keychain.

### Polar endpoints

All `POST`, JSON, no `Authorization` header
([reference](https://polar.sh/docs/api-reference/2026-10/customer_portal/activate-license-key)):

| Call | Body | Outcome mapping |
|---|---|---|
| `/v1/customer-portal/license-keys/activate` | `key`, `organization_id`, `label` (computer name), `meta` (`app_version`, `macos_version`) | 200 → activation stored; 404 `ResourceNotFound` → unknown key; 403 `NotPermitted` → refused with Polar's reason (device limit, revoked, disabled, expired) |
| `/v1/customer-portal/license-keys/validate` | `key`, `organization_id`, `activation_id`, `benefit_id` | 200 → renewed; 404 `ResourceNotFound` → rejected |
| `/v1/customer-portal/license-keys/deactivate` | `key`, `organization_id`, `activation_id` | 204 or 404 `ResourceNotFound` → released |

Production is `https://api.polar.sh`, sandbox `https://sandbox-api.polar.sh`.

- **Only a well-formed Polar error is a rejection.** Offline, timeouts,
  `429` (the unauthenticated license endpoints are limited to 3 requests per
  second), `5xx`, `422`, non-JSON bodies (captive portals), and responses
  this build cannot read are `unavailable`: the license keeps working through
  the grace period and the check is retried.
- **No `Polar-Version` header.** Polar removes each dated API version about
  nine months after release and answers requests pinned to a removed version
  with `404` (probed 2026-09-23: `Polar-Version: 2025-01` returns
  `{"detail":"Not Found"}`). The classifier would read that as `unavailable`,
  not as a rejection, but every customer on an old build would then run out
  the grace period and be locked out 30 days after the removal date.
  Unpinned requests use Polar's Current version (`2026-04` on that date,
  reported in the `polar-version` response header), and the decoder reads
  only the few fields both documented versions carry (the key's `id`,
  `benefit_id`, `display_key`, `limit_activations`, `expires_at`,
  `activation.id`), all but `id` optional. The same probe confirmed the
  error bodies: an unknown key returns `404 {"error":"ResourceNotFound"}` on
  all three endpoints.
- **Benefit scoping.** Validation sends `benefit_id`, so keys from the seller's
  other products fail server-side. Activation takes no benefit filter, so the
  app checks the returned `benefit_id` and releases the fresh activation of a
  foreign key immediately.
- **Key status.** Polar answers revoked, disabled, and expired keys with
  `404` (validate) or `403` (activate), and documents `status` in a `200`
  body as always `granted`. The app still refuses a `200` whose `status` is
  anything else (validate: rejected; activate: the fresh activation is
  released and refused), so a refunded or revoked key can never renew the
  grace period.
- **No `conditions`.** Server-side conditions must match on every validation;
  binding them to hardware would lock a customer out after a logic-board swap.

### Device limit and self-service release

The device limit lives in Polar (the benefit's activation limit), not in the
app, so it can change without an update. Customers release a Mac either with
"Deactivate this Mac…" in the popover or in Polar's customer portal
(`https://polar.sh/<slug>/portal`, linked as "Manage devices"). A Mac released
from the portal learns it at its next check (status `rejected`); entering the
key again re-activates it.

- **Key rotation** (customer portal or dashboard) keeps the activations but
  invalidates the old key. Entering the new key first validates it against
  the Mac's existing activation, so no second device slot is spent.
- **Migration Assistant** copies the login Keychain. The stored record carries
  a salted SHA-256 of the hardware UUID; on another Mac it no longer matches,
  and the app activates the stored key for the new Mac instead of reusing the
  old Mac's activation. "Deactivate" on such a record only forgets it locally.

### Offline grace and revalidation

- Checked at launch, hourly (Swift's continuous clock keeps counting
  through sleep, so the first tick after a long sleep comes right away),
  and when the network comes back; a validation is due 24 hours after the
  last success.
- A license works offline for **30 days** after the last success
  (`LicensePolicy.standard`).
- A rejected record is kept (with its key) and re-checked once a day, so a
  mistaken rejection heals by itself.
- The grace period counts on the local clock; setting the clock back extends
  it. Accepted: it only matters to someone who already has a key.

### Swapping the backend

To replace Polar (Paddle with an activation server, Keygen, or a self-hosted
server holding keys exported with Polar's List License Keys API):

1. Implement `LicenseBackend` with a new `identifier`.
2. Construct it in `LicenseModel` instead of `PolarLicenseBackend`.
3. Ship the update (through the app's update channel once it exists).

At the next launch every stored record from the `polar` backend is
re-activated against the new backend with its stored key — customers do not
re-enter anything, provided the new backend accepts the same key strings.

### Local storage and data sent

- One generic-password item in the login Keychain (service
  `com.lightsound.noican.license`): the key, the activation ID, the masked
  key, expiry, device limit, last validation time, the hashed device ID, and
  the last rejection. Developer ID builds read it back silently across
  updates; ad-hoc builds are a new code identity after every rebuild and get
  a Keychain prompt. An uninstaller must delete this item.
- Sent to Polar: the license key, the organization ID, the benefit ID, the
  computer name (as the activation label, so the customer can tell Macs apart
  in the portal), and the app and macOS versions. The hardware UUID is never
  sent. The privacy policy must list these.

## Polar setup (owner)

Do everything in the **sandbox** first (<https://sandbox.polar.sh/start>),
then repeat in production. Sandbox and production are separate accounts,
organizations, IDs, and keys.

1. **Account and organization.** Create the account and an organization
   (slug, e.g. `noican`). In production, complete payout onboarding (Stripe
   Connect Express, Japan supported) and the account review.
2. **License-key benefit.** Benefits → + New Benefit → Type: License Keys.
   - Prefix: e.g. `NOICAN` (branding only; the app accepts any key).
   - Expiration: none (one-time purchase; the app would honor an expiry).
   - Activation limit: on, set to the number of Macs one purchase covers.
     Required: without it Polar refuses every activation ("does not support
     activations"), and the app always activates.
   - "Enable user to deactivate instances via Polar": on (this is the
     self-service release in the customer portal).
   - Usage limit: off.
3. **Product.** Products → new product → one-time purchase, fixed price (JPY
   and USD; tax-inclusive display for Japan), attach the license-key benefit.
4. **IDs.** Fill `macos/Sources/NoicanMenuBar/LicenseConfiguration.swift`:
   - `organizationID`: the organization ID from the organization settings.
     Both IDs are required; until both are UUIDs the build stays
     unconfigured (ungated).
   - `benefitID`: the license-key benefit's ID.
   - `organizationSlug`: the slug from step 1 (enables "Manage devices").
   - `server`: `.sandbox` for the sandbox build, `.production` for release.
   - `purchaseURL` (optional): the checkout link or product page.

   Both IDs can be confirmed from a test key: `curl -s -X POST
   https://sandbox-api.polar.sh/v1/customer-portal/license-keys/validate -H
   'Content-Type: application/json' -d '{"key":"<key>","organization_id":"<org id>"}'`
   returns the key with its `organization_id` and `benefit_id`.
5. **Test purchase (sandbox).** Buy the product with card `4242 4242 4242
   4242` (sandbox emails reach organization members only). The key appears in
   the customer portal and on the benefit's license-keys page.

## Manual verification (sandbox build)

1. Launch: "License · Not activated · sandbox"; Preview/On are refused with
   the license message; Off and the pickers work.
2. Paste the key, Activate: "Active · sandbox"; the activation appears in the
   portal under the computer name; Preview/On start.
3. Activate on further Macs up to the limit; one more is refused with
   Polar's reason and the portal hint.
4. Release a Mac in the portal; on that Mac "Verify now" (or the next daily
   check) shows the activation as no longer valid and Preview/On are
   refused. Entering the key again re-activates it.
5. Rotate the key in the portal; enter the new key: no new activation
   appears in the portal.
6. "Deactivate this Mac…": the activation disappears from the portal; the
   Keychain item is gone (`security find-generic-password -s
   com.lightsound.noican.license` fails).
7. Offline (Wi-Fi off): "Verify now" shows "Active (offline)" and the grace
   end date; Preview/On still work.
8. Refund the test order in the dashboard and observe whether Polar revokes
   the key; on the next check the app should show it as not valid.

Before release, switch `server` to `.production` with the production IDs and
check the signed production build positively — the absence of "· sandbox"
alone also holds for an unconfigured (ungated) build:

1. Before a key is entered the popover reads "License · Not activated" (not
   "Not configured in this build") and Preview/On are refused.
2. After activating a real production key it reads "License · Active" with
   no suffix, and Preview/On start.
