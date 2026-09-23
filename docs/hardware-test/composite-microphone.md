# Hardware test: Composite input/output microphone

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

## Composite input/output microphone (headphone-equipped USB microphone)

The private aggregate is composed as `[microphone, virtual output]`
(the microphone is the clock master and must stay first so that
aggregate input channel 0 is the microphone, not the loopback's own
input). An aggregate's output channels are its subdevices' output
channels concatenated in that order, so a microphone that has output
channels of its own — a USB microphone with a headphone jack such as
the Shure MV7+, or an audio interface — places them *ahead* of the
virtual output. The transport rendered a mono stream, and AUHAL's
default output channel map sends client channel 0 to device output
channel 0: on such a device that is the microphone's own headphone
output, and the virtual output received silence. Recordings from the
virtual microphone were completely silent while the preview (its own
AUHAL on the default output) kept working, with no error and no
underrun line in the log; the built-in microphone, having no outputs,
was never affected — which is why earlier acceptance runs, all made
with the built-in microphone, did not see it.

The fix sets an explicit AUHAL channel map on the aggregate unit. The
transport renders **one client channel per virtual-output channel** and
writes the mono engine sample into every channel of each frame (dual
mono — the shape the split transport and the preview monitor always
produced), and the map places those client channels on the virtual
output one-to-one: virtual output channel *i* receives client channel
*i*, and every device output channel ahead of it — the microphone's own
outputs — is left silent. The virtual output's position is computed by
the control plane from the subdevice list it composes and re-checked on
the Rust side against the channel count the aggregate reports. The map's
tail is as wide as the virtual output: with the current 1-channel Noican
driver (0.2.0, docs/driver.md "History") the built-in-microphone layout
gives `[0]` and a stereo-headphone microphone `[-1, -1, 0]`; on a
2-channel virtual output (stock BlackHole 2ch, the 0.1.0 driver) the
same layouts give `[0, 1]` and `[-1, -1, 0, 1]`.

The first version of this fix (PR #26) kept a mono client stream and
mapped it to the virtual output's first channel only, leaving channel 1
silent (measured on the built-in microphone: channel 0 −16.5 dBFS,
channel 1 silent). That was heard in the left ear only on headphones,
and consumers that average a stereo input to mono — common in meeting
applications — received it 6 dB down. Duplicating in the *map* instead
(`[-1, -1, 0, 0]`) was rejected because no primary source states that
an AUHAL map may name one client channel twice, and a rejected map
would fail every aggregate start; a one-to-one map is the documented
shape, and its single-entry form was accepted and read back on hardware
(PR #26's record) — the two-entry form was pinned by the 2026-09-05
record on the 0.1.0 driver (see `noican_coreaudio::routing` for the full
decision record). Step 8 and acceptance criterion 3 pin the shape:
every virtual-output channel carries the same signal, and channel 0's
level is unchanged against the previous build. On the 1-channel driver
the recording is mono (`channels: 1` from the script in
[level-integrity.md](level-integrity.md) — or, should
QuickTime widen a 1-channel device to a 2-channel file, two identical
channels), and channel 0's level must equal the one measured on the
2-channel driver. Note for consumers that *sum* L+R without scaling
(rare; most average): a dual-mono 2-channel signal reads +6 dB there
compared with a single-channel one. The capture direction is untouched.
The split (native-rate) transport is not involved: its output AUHAL
sits on the virtual output device alone, so a microphone's outputs
never precede the virtual output there.

1. Connect the composite device and make sure it advertises 48 kHz
   (Audio MIDI Setup, input side), so the engine takes the aggregate
   path. Note its output channel count from the output side (2 for a
   stereo headphone jack).
2. Plug wired headphones into the *device's own* headphone jack, and
   make a different device (the built-in output, or other headphones)
   the system default output.
3. Select the composite device as the microphone, select `Passthrough`
   or `DeepFilterNet3 48k`, then select On. The engine must reach
   `Running`. The private aggregate is hidden from Audio MIDI Setup, so
   read its composition from Console instead (subsystem
   `com.lightsound.noican`, category `engine-diagnostics`, or the
   `log stream` command from
   [output-underrun-diagnostics.md](output-underrun-diagnostics.md)) —
   two info lines
   appear on every aggregate-path start:
   - `Aggregate composed: microphone "<name>" (in X / out Y), virtual
     output "<name>" (out Z); aggregate reports W output channel(s) in
     S stream(s)` — Core Audio's view, written when the aggregate is
     created (Y is the microphone's own output channel count, W must be
     Y + Z, and S the number of subdevices contributing outputs);
   - `Aggregate output routing: aggregate output channels N, virtual
     output at channels A..B, channel map requested [...], channel map
     read back after initialize [...]` — AUHAL's view, written about a
     second after start. N must equal W, `A..B` must be `Y..Y+Z`, the
     requested map must be `-1` at every index below A and `0, 1, …`
     (client channel *i* at index A + *i*) from A to the end — with the
     current 1-channel driver `[-1, -1, 0]` for a stereo-headphone
     microphone and `[0]` for the built-in microphone; on a 2-channel
     virtual output `[-1, -1, 0, 1]` and `[0, 1]` — and the read-back
     must equal the request.
   Record both lines verbatim in the result record.
4. Record 30+ seconds of speech from the Noican virtual microphone in
   QuickTime (or CleanShot / OBS). The recording must be non-silent,
   intelligible, and — at 100% strength — processed (materially differs
   from a raw-microphone control). This is the check that failed before
   the fix (a completely silent file).
5. While recording, listen on the headphones plugged into the
   microphone's own jack: **nothing from Noican** may come out of them
   (the device's own hardware monitoring, if it has any, is unrelated
   and may be audible — distinguish it by muting the device's monitor
   control). Processed voice on the microphone's headphone jack is the
   pre-fix misrouting and fails this check.
6. Select Preview: the processed voice must play on the system default
   output as before, the recording must continue unaffected, and the
   microphone's own headphone jack must still stay silent.
7. Switch models while recording (aggregate-path criteria: bounded fade
   only) and watch Console for the underrun line — a light model must
   log zero underruns as before.
8. Switch to the built-in microphone while running: after the brief
   busy state, recordings from the virtual microphone must still carry
   audio (regression check for the no-own-outputs layout, where the
   virtual output is at channels 0–1). Pin the signal shape, not just
   presence: record a fixed reference (a sentence at constant distance,
   or a tone played into the room) once on this build and once on the
   previous build (`main` before this change) with the same settings —
   Passthrough, strength 100%, the microphone's system input slider
   and the Noican Microphone slider both at maximum — and compare per
   channel with the script in [level-integrity.md](level-integrity.md).
   Expected on this
   build: **every virtual-output channel carries the same signal** —
   on the 1-channel driver the file reads `channels: 1` (or, if
   QuickTime widens the device to a 2-channel file, two channels whose
   RMS agree within 0.1 dB); on a 2-channel virtual output (stock
   BlackHole 2ch, the 0.1.0 driver) two channels within 0.1 dB of each
   other — and channel 0's RMS is within ±1 dB of the previous build's
   channel 0 (references from the 2026-09-05 measurements on the 0.1.0
   driver: −16.5 dBFS, later −19.6 dBFS, on the built-in microphone;
   your absolute figure depends on voice and distance, the *difference*
   between builds is what is pinned). A level change on channel 0
   beyond that, or a channel left silent, fails. Switch back to the
   composite device: audio must return, on every channel.
9. *(If available)* Repeat 3–4 with an audio interface that has more
   than two outputs: the virtual output sits after all of them, and the
   recording must still carry audio.

A start refusal reading `virtual output routing failed: the aggregate
device reports N output channel(s), but the virtual output was expected
at channels A..B` means the composed layout and the device disagree;
record N, A, B, the device's Audio MIDI Setup input/output channel
counts, and the `Aggregate composed` line (the only one of the two
Console lines that prints on a refusal — there is no running transport,
so the `Aggregate output routing` line never appears; N, A and B are in
the refusal message itself). This refusal is deliberate
(the alternative is a guessed map that may misroute silently); do not
characterize the path as working. The counts are re-read after the
48 kHz switch and immediately before the aggregate is composed, so a
rate-dependent channel count (ADAT/S-MUX interfaces expose 8 channels
at 48 kHz but 4 at 96 kHz) is not a cause of this refusal; if it still
appears, the first number to question is N — whether AUHAL reports the
aggregate's total output channels or only its first stream, which no
primary source states outright.

## Acceptance checklist (composite input/output microphone)

Run the composite input/output microphone procedure above; the build
passes when:

1. **Recordings carry audio**: with a headphone-equipped USB microphone
   (or an interface with outputs) as the input, a QuickTime recording
   from the Noican virtual microphone contains the processed speech —
   not silence. Record the device, its input/output channel counts, and
   the aggregate's output channel count.
2. **Nothing leaks to the microphone's own outputs**: Noican's output
   is inaudible on headphones plugged into the microphone's own jack,
   in both On and Preview.
3. **Every channel fed, level pinned**: the same recording with the
   built-in microphone carries the same signal on every
   virtual-microphone channel — `channels: 1` on the current 1-channel
   Noican driver (two identical channels if QuickTime widens it);
   per-channel RMS within 0.1 dB of each other on a 2-channel virtual
   output (stock BlackHole 2ch, the 0.1.0 driver) — and channel 0's RMS
   is within ±1 dB of a same-settings recording on the previous build
   (per-channel measurement with the
   ["Level integrity" script](level-integrity.md)): the
   engine level is unchanged by the routing or by the driver's width.
   The composite device must show the same shape.
4. **Everything else as before**: Preview, model switching, meters, and
   the underrun diagnostics behave exactly as on the earlier records
   with the composite device selected.
5. *(Optional)* **Split path unaffected**: a recording through a
   native-rate (Bluetooth or 44.1 kHz) microphone still carries audio —
   the split transport has no aggregate and takes no channel map.
