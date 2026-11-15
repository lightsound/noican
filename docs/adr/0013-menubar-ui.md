# 0013. Menu bar UI

Status: Accepted

Moved from code comments on 2026-11-21 (the delete-comment pass).
The original wording is preserved under each topic; the source
location of each comment at the time of the move (commit 397e3df)
is noted above its text.

## level-bar-db-scale

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 445–447.

One small horizontal peak bar. The linear peak is drawn on a
−60 dB…0 dB scale so quiet-but-present signal stays visible and the
input/output gap reads as suppression depth.

## menu-popover-animation

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 51–60.

Deliberately no layout animation on height-changing state:
every intermediate height would update the hosting
controller's preferredContentSize and make the popover chase
the animation frame by frame, which visibly warped the layout
(including the mode control's sliding pill — its own spring in
ModePicker is unaffected and still animates). System menus
change height instantly too.
Poll the engine's peak meters (and the feedback-trip flag) only
while the popover is open; the task is cancelled when the view
disappears.

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 72–75.

Always one line: the header must never change height,
or everything below it (including the sliding pill)
would shift mid-animation. Full errors render below
the mode control.

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 99–102.

Opt out of the popover's eased layout animation: the fill
styles (hierarchical gray vs. plain colors) cannot
interpolate, so animating them cross-fades through
transparent — the dot visibly blinked on Off transitions.

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 250–252.

Deliberately no expand/collapse animation: every animation
frame would change the content's ideal height and make the
menu popover chase it (see the note on MenuView.body).

## menubar-icon-rendering

Source: `macos/Sources/NoicanMenuBar/NoicanMenuBarApp.swift` lines 37–49.

Menu bar icon renderer: the glyph is the selected mode (`mic.slash`
Off, `waveform.badge.mic` On, `headphones` Preview), the color is its
health — monochrome Off, green while the mode is actually delivering,
red (matching the status dot and the pill's warning tint) while the
selection is not delivering, so trouble is visible without opening
the menu.

Every state draws into the same fixed-size canvas: the glyphs have
different intrinsic widths, and a status item that changes width
shifts the menu's anchor on every mode change.

Main-actor bound: the image cache is UI state and every caller is
the status-item controller.

Source: `macos/Sources/NoicanMenuBar/NoicanMenuBarApp.swift` lines 65–69.

One prerendered image per mode × health, keyed
"<mode>-<isUnfulfilled>". Off is a template image (the system
adapts it to the menu bar appearance); colored states are
non-template with palette colors, because status items render
template images tint-free.

Source: `macos/Sources/NoicanMenuBar/NoicanMenuBarApp.swift` lines 92–95.

System symbols are template images; drawn directly, a template
is treated as a monochrome mask and the palette tint would not
survive the bake. The canvas re-applies templateness for the
untinted (Off) state below.

## microphone-selection-recovery

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift` lines 205–209.

A microphone pick pre-flights the device, then rebuilds the
transport around it when the engine is meant to run. Because the
mode keeps the user's intent across failures, this same path
auto-recovers: picking a working microphone after a failed start
restarts straight into the selected mode.

## mode-picker-selection-intent

Source: `macos/Sources/NoicanMenuBar/ModePicker.swift` lines 4–13.

Custom three-segment mode control: a capsule track with a highlight
pill that slides between segments. Every segment stays tappable
(except while busy); when a selection is refused — Preview on an
unsafe output — the caller keeps the mode where it was and explains
why below the control, which confuses less than a segment that
cannot be pressed.

The selection is the user's *intent* and never moves on its own: when
the selected mode is not actually delivering (`isUnfulfilled`), the
pill turns red instead, and tapping the same segment retries.

## mode-picker-visual-feedback

Source: `macos/Sources/NoicanMenuBar/ModePicker.swift` lines 29–34.

One *persistent* pill that moves between the segments' anchor
frames. The previous per-segment `if` pill was inserted/removed
on every switch, so an opacity transition ran alongside the
slide (ghosting) and a mid-flight retarget could snap (visible
teleport). A single view that only changes its matched target
has neither failure mode.

Source: `macos/Sources/NoicanMenuBar/ModePicker.swift` lines 42–44.

Isolate the sliding pill's matched geometry from outside layout
shifts (status/error text changing the control's vertical
position mid-slide would otherwise distort the animation).

Source: `macos/Sources/NoicanMenuBar/ModePicker.swift` lines 89–91.

The segments use the shared StaticButtonStyle (StaticButtonStyle.swift):
the sliding pill is the selection feedback, and the plain style's
momentary dimming read as flicker on the segments.

## picker-selection-semantics

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift` lines 163–164.

Model chosen in the picker (reverted by the reducer when a switch
fails, so the picker never lies about what is running).

Source: `macos/NoicanState/Sources/NoicanState/AppModel.swift` lines 166–168.

Microphone chosen in the list (reassigned when the device
disappears, restored when a refused/failed switch keeps the
previous one).

## popover-activation-requirement

Source: `macos/Sources/NoicanMenuBar/StatusBarController.swift` lines 89–92.

The app must be active while the menu shows (MenuBarExtra did
this internally): an inactive app renders the popover with the
dimmed inactive-window materials, and transient dismissal on
outside clicks only works reliably for the active app.

## popover-close-teardown

Source: `macos/Sources/NoicanMenuBar/StatusBarController.swift` lines 104–107.

Covers every close path (toggle, click outside, Escape): drop the
popover and its hosting controller so SwiftUI tears the content
down (cancelling MenuView's level-poll task), and stamp the time
for the reopen guard.

## popover-level-polling

Source: `macos/Sources/NoicanMenuBar/AppState.swift` lines 520–524.

Runs while the menu popover is visible (bound to the menu view's
task) and stops when it closes: ~20 Hz, two non-blocking atomic
reads per tick, plus a preview-availability re-check about once
per second (the default-output listener misses same-device
data-source flips such as the headphone jack).

## popover-reopen-guard

Source: `macos/Sources/NoicanMenuBar/StatusBarController.swift` lines 30–33.

When the menu last closed. A click on the status item while the
menu is open dismisses the transient popover on mouse-down and
then fires the button action on mouse-up; ignoring an open
request right after a close keeps that from reopening the menu.

## progressive-disclosure-settings

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 5–15.

The menu bar popover, top to bottom: a status header, the single
Off / Preview / On mode control, a monitoring section that exists only
while the engine runs (level bars, and the headphone caption while
previewing), the Microphone list, a collapsed-by-default
"Model & strength" section holding the model selector and strength
slider (progressive disclosure: the default model is meant to be good
enough that first-time users only touch the mode control and the
microphone), the License section (`LicenseSection`), and a utility
footer. Spacing and typography follow
macOS menu bar app conventions (14 pt content margins, caption-weight
section labels, secondary text for status detail).

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 20–23.

Whether the "Model & strength" section is expanded. Pure view
chrome, so it lives in AppStorage rather than the reducer;
remembered across launches so power users keep it open while
first-time users start with the short menu.

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 218–225.

Progressive disclosure: the default model is meant to be
good enough that first-time users only touch the mode
control and the microphone, so the model list and the
strength slider live behind this header. The expansion
state is remembered across launches (power users keep it
open; new users start with the short menu). The collapsed
row advertises what is inside — the active model and
strength — so the fold never hides state.

## sandbox-build-disclosure

Source: `macos/Sources/NoicanMenuBar/LicenseModel.swift` lines 18–19.

Whether this build talks to Polar's sandbox (shown next to the
status so a sandbox build is never mistaken for a release).

## selection-must-exist-in-list

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift` lines 414–416.

A stored microphone that is not currently connected is skipped
(not remembered as a dangling selection): the reducer's other
transitions assume the selection exists in the list.

## selection-vs-session-input

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift` lines 460–461.

The transport is bound to the session's input (the selection
can legitimately differ, e.g. right after a refused switch).

Source: `macos/NoicanState/Sources/NoicanState/AppReducer.swift` lines 670–672.

Capture the pre-flighted device: the selection can be
reassigned by device hot-plug during the busy window, and
the transport is bound to this one.

## standalone-swift-packages

Source: `macos/NoicanLicensing/Package.swift` lines 5–7.

Standalone like NoicanState: the license logic depends on Foundation
only (the Keychain store compiles where Security exists), so its tests
run without the Rust staticlib and without a signed app bundle.

Source: `macos/NoicanState/Package.swift` lines 5–8.

Deliberately a standalone package (not a target of the app package):
it depends on nothing but the standard library, so `swift test` here
never needs the Rust staticlib the app's executable target links, and
the reducer tests run on any platform — including the Linux CI runners.

Source: `macos/Package.swift` lines 14–16.

The pure state machine (reducer + projections). A standalone
package so its tests run without the Rust staticlib this
executable links; see macos/NoicanState/Package.swift.

Source: `macos/Package.swift` lines 18–19.

License activation against Polar (or a replacement backend);
standalone for the same reason as NoicanState.

## static-button-press-feedback

Source: `macos/Sources/NoicanMenuBar/StaticButtonStyle.swift` lines 3–7.

Button style that renders the label untouched in every state — no
pressed-state dimming. The mode control's sliding pill, the menu's
rows, and the disclosure header all carry their own selection or
hover feedback, and `.plain`'s opacity flash on press reads as
flicker on them, so they opt out of any system press effect.

## status-dot-settled-color

Source: `macos/Sources/NoicanMenuBar/MenuView.swift` lines 106–108.

Settled health only — no transitional color: the spinner already
says "busy", and flashing the dot orange for a milliseconds-long
Preview/On toggle just made it churn.

## status-item-popover-architecture

Source: `macos/Sources/NoicanMenuBar/NoicanMenuBarApp.swift` lines 5–15.

The app is an AppKit status-item shell around the SwiftUI menu.

It deliberately does *not* use `MenuBarExtra(.window)`: its window is
sized by `NSHostingView`'s automatic window resizing, which anchors
at the AppKit bottom-left origin, so any content-height reduction
(collapsing the "Model & strength" section, the monitoring section
disappearing) visibly dropped the whole menu — and nothing outside
the framework can deterministically prevent it. The status item and
menu live in `StatusBarController`, which shows the menu in a native
`NSPopover`: content-size changes resize anchored to the status item
(downward only), with the system's own menu chrome and dismissal.

Source: `macos/Sources/NoicanMenuBar/NoicanMenuBarApp.swift` lines 21–22.

The menu lives in the AppKit panel; SwiftUI still requires a
Scene, and an empty Settings scene contributes no UI.

Source: `macos/Sources/NoicanMenuBar/StatusBarController.swift` lines 5–22.

Owns the status item and shows the menu in a native `NSPopover`.

This is the classic status-item + popover architecture, chosen after
two failed shells: `MenuBarExtra(.window)` and a hand-rolled panel
both fell to the same root cause — `NSHostingView` automatically
resizes its window around the AppKit *bottom-left* origin, so any
content-height reduction dropped the whole menu (and fighting that
resize with manual frame corrections raced it, glitching layout).
`NSPopover` solves the geometry natively: its content size follows
the hosting controller's `preferredContentSize` and resizes anchored
to the status item, growing and shrinking downward only. It also
restores the system menu chrome — material, corner radius, shadow —
and the native dismissal contract (click outside, Escape) instead of
reimplementations of them.

The menu content is built fresh on every open and released on close,
which scopes the 20 Hz level poll (`MenuView.task`) exactly to the
time the menu is visible.

Source: `macos/Sources/NoicanMenuBar/StatusBarController.swift` lines 78–80.

The popover tracks the SwiftUI content's ideal size through
preferredContentSize — the one window-sizing path that is
anchored to the status item instead of the bottom-left origin.

## swiftui-sendable-annotation-gap

Source: `macos/Sources/NoicanMenuBar/ModelSelector.swift` lines 72–74.

The macOS 15 SDK marks this closure @Sendable while
preference changes are in fact delivered on the main
actor; assumeIsolated bridges the annotation gap.
