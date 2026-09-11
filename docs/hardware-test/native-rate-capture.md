# Hardware test: Non-48 kHz microphone (native-rate capture)

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

## Non-48 kHz microphone (native-rate capture)

Microphones that cannot run at 48 kHz — telephony-profile Bluetooth
headsets (HFP/SCO at 8/16/24 kHz) and 44.1 kHz-family devices
(44.1/22.05/11.025 kHz; 88.2/96 kHz-only interfaces too) — are captured
through a **split transport** instead of the private Aggregate Device:
an input-only AUHAL on the microphone at its native rate, an output-only
AUHAL on the virtual output at 48 kHz, and the inference worker bridging
the two clock domains through one arbitrary-ratio polyphase FIR
resampler (`PolyphaseResampler` in `noican-core`): the exact reduced
ratio — 160/147 for 44.1 kHz, 3/1 for 16 kHz — with the clock-drift
correction folded into its fractional phase step and steered by ring
occupancy (there is no aggregate to absorb the clock split, so drift
compensation is our own — docs/tech-research.md §4.2). The split path
adds a ~50 ms output cushion; the 48 kHz aggregate path is untouched.
The engine still runs at 48 kHz throughout, so recordings from the
virtual device remain 48 kHz regardless of the capture rate.

Before testing, confirm the device's actual native rate: open Audio
MIDI Setup, select the device's input side, and read the Format
pop-up's rate list — or run
`system_profiler SPAudioDataType | grep -A 12 "<device name>"` and read
`Current SampleRate`. Record the rate in the result record; the
microphone list must show the same value.

1. Connect the device (pair the Bluetooth headset). Confirm the
   microphone list shows its native rate next to the name in audio
   notation (e.g. "16 kHz", "24 kHz", "44.1 kHz").
2. Select it. A secondary-style notice must appear under the list. For
   a telephony profile: capture is narrow-band (phone profile) and
   headset playback quality drops while the microphone is in use. For a
   44.1 kHz-family device: the device is resampled to the 48 kHz engine
   rate inside Noican — the notice must **not** call the audio
   narrow-band. Either way this is information, not an error — no red
   text, no refusal.
3. Select On and grant microphone access if prompted: the engine must
   reach `Running` (green indicator). No Aggregate Device appears in
   Audio MIDI Setup for this path (two AUHAL instances instead). The
   one-time transport diagnostics line in Console must show
   `worker realtime scheduling true`, and it is followed by the split
   transport's routing line — `Split output routing: virtual output
   channels N, render format requested N ch, render format read back
   after initialize N ch` — where N is the virtual output's output
   channel count as AUHAL reports it for the output-only unit (1 for
   the current Noican driver, 0.2.0; 2 for stock BlackHole 2ch and for
   the 0.1.0 Noican driver). The render format is sized from that count
   rather than fixed at two channels, so all three numbers must agree
   and match the device's channel count in Audio MIDI Setup; record the
   line verbatim. A start refusal reading `virtual output routing
   failed: the virtual output device reports no output channels (split
   transport)` means AUHAL reported zero channels for the device — the
   transport refuses rather than guessing a width; record the device's
   Audio MIDI Setup channel counts.
4. Record 30+ seconds from the virtual device in QuickTime: the
   recording must be 48 kHz, non-silent, and intelligible. On a
   telephony profile expect telephony bandwidth (the source is 8–16 kHz
   capture) and expect the headset's own playback to sound worse while
   recording — both by design. On a 44.1 kHz device expect full-band
   quality indistinguishable from the aggregate path (passband flat to
   ≈15 kHz, gentle roll-off above 17 kHz): no pitch shift, no
   periodic click, no metallic/aliased coloration.
5. Speak continuously at 50% strength: the voice must stay single (no
   doubled/hollow voice) — the dry path taps the engine input *after*
   the input resampler, so the alignment must hold exactly as on the
   aggregate path.
6. Switch models while recording: same bounded-fade criteria as the
   aggregate path.
7. Switch between the native-rate microphone and the built-in
   microphone while running: each direction must rebuild into Running
   (aggregate ↔ split transport switch behind the same busy machine).
8. **Drift/endurance**: record 30 continuous minutes through the
   native-rate microphone. Pass criteria: no periodic click, no
   accumulating gap/overlap, no engine fault, no stall — the drift
   servo must keep the two clock domains aligned for the whole session.
   On a 44.1 kHz device the resampler runs at a non-integer ratio, so
   this is also the check that the servo's occupancy control holds
   there; a slow periodic pitch wobble or a click every few seconds
   would point at it.
9. **Underrun diagnostics on the split path**: while recording, watch
   Console for the underrun line (see
   [output-underrun-diagnostics.md](output-underrun-diagnostics.md)).
   With a light model (FastEnhancer-B) the split transport must log
   zero underruns over 60+ s of continuous speech; record any line
   verbatim.
10. **Profile flip (A2DP ↔ HFP)**: while running, force a rate
    renegotiation (e.g. play music to the headset before/while starting,
    or toggle the headset's own transparency/ANC features if they
    trigger one). The app must either keep running unaffected or rebuild
    automatically within a moment (a brief busy spinner, then Running) —
    never a permanently dead session. Unplugging/re-pairing mid-session
    may still surface as "Audio stalled"/"Microphone disconnected" like
    any device loss; selecting the device again must recover.
11. **Virtual-output loss on the split path**: while running on the
    native-rate microphone, uninstall (or otherwise remove) the virtual
    output device — the engine must stop with "Virtual output device
    removed" like on the aggregate path (device-list listener). The
    split transport additionally watches for a *wedged* output side —
    the device still listed but its IO no longer calling back — via an
    output-callback pulse counter: capture alone keeps the frame
    heartbeat advancing on this path, so a pulse counter frozen for
    ~3 s of live capture raises an engine fault ("Audio fault — turn
    noise cancellation off and on") instead of rendering perpetual
    silence under a green pill. A transient hiccup that merely fills
    the output ring must *not* trip it (the callback keeps pulsing).

## Acceptance checklist (native-rate capture, issue #7)

Run the non-48 kHz microphone procedure above; the build passes when:

1. **Running on a native rate**: a microphone that cannot run at 48 kHz
   — a Bluetooth headset on HFP (16 kHz) *or* a 44.1 kHz-family device
   — can be selected and the engine reaches `Running`. Record which
   rate was exercised; both kinds are in scope, but one device proves
   the transport.
2. **48 kHz output**: recordings through the virtual device remain
   48 kHz and intelligible (telephony bandwidth expected at a telephony
   source; full-band expected from a 44.1 kHz source).
3. **No drift artifacts**: a 30-minute session produces no clock-drift
   clicks, gaps, pitch wobble, or accumulating timing error — on a
   44.1 kHz device this also proves the servo at a non-integer ratio.
4. **Aggregate path unchanged**: the built-in / USB 48 kHz microphone
   path behaves exactly as before (behavior, quality, latency).
5. **Strength alignment**: 50% strength on the split path produces a
   single voice (no comb-filter/double voice).
6. **Rate-change recovery**: an A2DP ↔ HFP renegotiation while running
   rebuilds automatically (or passes through unaffected); it never
   leaves a dead session. (Bluetooth devices only; a fixed-rate
   44.1 kHz interface has nothing to renegotiate.)
7. **UI truthfulness**: the native rate shows in the microphone list in
   audio notation, the notice for the selection matches the device kind
   (telephony trade-offs vs. plain conversion), and devices outside
   8–192 kHz (or with an unreadable rate) are still refused with a
   clear reason.
8. **Real-time constraints hold**: re-run the
   [Real-time audit](real-time-audit.md) on the
   split transport — both callbacks (capture and virtual output) stay
   allocation- and lock-free; resampling runs on the inference worker
   with buffers preallocated at start (no growth in the worker after
   the first second).
9. **Split-path underruns**: with FastEnhancer-B the split transport
   logs zero underruns over 60+ s of continuous speech (this also
   closes criterion 4 of the
   [output-underrun checklist](output-underrun-diagnostics.md)).
10. **Split render format follows the device**: the `Split output
    routing` line (procedure step 3) reports a virtual-output channel
    count equal to the device's output channel count in Audio MIDI
    Setup, and the requested and read-back render formats both equal
    it; the recording carries the same signal on every channel of that
    width. Record the line verbatim. This scores the one Core Audio
    behaviour the render-format decision record
    (`noican_coreaudio::routing`, "The split transport's render
    format") rests on without prior hardware evidence: that the
    device-side stream-format read behaves on an output-only AUHAL as
    it does on the aggregate unit.
