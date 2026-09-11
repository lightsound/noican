# Level integrity

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

## Level integrity

What decides how loud consumers hear the virtual microphone, and how to
tell the pieces apart when "Noican sounds quiet". Established by the
2026-09-05 hardware investigation (built-in microphone, MV7i, Bluetooth
headset; all figures from QuickTime recordings measured with the script
below).

**What sets the level.**

- **The engine path is unity gain.** Passthrough at 100% strength
  changes nothing; the models change the signal, not its nominal level
  (Hush carries a measured makeup gain, see
  [functional test 10b](functional-test.md)).
- **Noican captures the microphone at unity and does not apply the
  microphone's system input slider on the aggregate path.** Measured
  with the built-in microphone: moving System Settings › Sound ›
  Input's slider for the *microphone* from the middle to maximum
  changed a direct recording by +7.6 dB and the Noican recording by
  1.4 dB — within the spread of repeated speech, i.e. no effect. The
  aggregate's AUHAL reads the device's raw input stream. On the split
  transport (non-48 kHz microphones, which are opened directly rather
  than through an aggregate) the same holds within the evidence so far:
  the 2026-09-05 Bluetooth measurement
  ([record](../acceptance/2026-09-05-dual-mono-level-integrity.md)) read
  −23.2 dBFS with the headset's slider at the middle and −21.0 dBFS at
  maximum (+2.2 dB), which the operator attributed to speech variance —
  far from the +7.6 dB a direct recording shows for a slider that is
  applied. One pair of recordings; a repeat would firm the figure up.
- **The Noican Microphone device has a volume control and a mute of its
  own** (System Settings › Sound › Input, with the Noican Microphone
  selected; Audio MIDI Setup shows the same controls). The
  BlackHole-derived driver applies this one value (−64…0 dB) to every
  sample it loops, so it attenuates what *every* consumer receives. It
  was the cause of the owner's "quiet" report: the slider sat at about
  −35 dB. Who moved it is unknown — a user, or a meeting application's
  "automatically adjust microphone volume" feature writing the selected
  input device's system volume. Noican **does not** restore it (that
  would fight such an app and take away the user's own adjustment); it
  detects the condition and says so (below).
- **Where to adjust level, then:** the Noican Microphone slider (which
  is what consumers hear), or the microphone itself (MV7i: MOTIV Mix's
  Auto Level / gain; an interface's preamp). The microphone's *system*
  slider is not in the chain.

**Detection of a turned-down or muted virtual output.** The app reads
the Noican Microphone device's volume scalar and mute (input scope,
output scope as fallback; devices without the controls are not judged)
when an engine start settles and on every 1 Hz health-poll tick. Below
unity (scalar < 0.999) shows one orange line under the mode control —
"Noican Microphone volume is turned down in System Settings › Sound ›
Input — apps will hear you quietly." — and mute shows "Noican
Microphone is muted in System Settings › Sound › Input."; a nominal
reading clears it within a second. Each detection and resolution is
also written to the unified log (subsystem `com.lightsound.noican`,
category `engine-diagnostics`, prefix `Virtual output level:`) with the
scalar reading — one line per *distinct* reading, so every slider move
logs but a restart with the slider still down does not — so the
frequency of unexplained changes can be established over time. Nothing
is written back to the device.

**Isolating "Noican sounds quiet".** Record the same sentence at the
same distance twice in QuickTime (File › New Audio Recording, maximum
quality), once with the Noican Microphone selected and once with the
microphone directly, with Passthrough at 100% and both the Noican
Microphone slider and the microphone's slider at maximum. Convert and
measure each file:

```bash
afconvert -f WAVE -d LEI16 in.m4a out.wav
```

```bash
# Per-channel RMS/peak of a 16-bit WAV (reads the RIFF chunks directly:
# Python's wave module rejects the WAVE_FORMAT_EXTENSIBLE header afconvert
# writes).
python3 - "$WAV" <<'EOF'
import sys, struct, math
raw = open(sys.argv[1], "rb").read()
assert raw[:4] == b"RIFF" and raw[8:12] == b"WAVE", "not a WAV file"
pos, ch, bits, data = 12, None, None, None
while pos + 8 <= len(raw):
    cid, size = raw[pos:pos+4], struct.unpack("<I", raw[pos+4:pos+8])[0]
    body = raw[pos+8:pos+8+size]
    if cid == b"fmt ":
        ch, bits = struct.unpack("<H", body[2:4])[0], struct.unpack("<H", body[14:16])[0]
    elif cid == b"data":
        data = body
    pos += 8 + size + (size & 1)
assert ch and bits == 16 and data is not None, f"unexpected format: ch={ch} bits={bits}"
samples = struct.unpack("<%dh" % (len(data) // 2), data)
print(f"channels: {ch}")
for c in range(ch):
    s = samples[c::ch]
    rms = math.sqrt(sum(x * x for x in s) / len(s)) / 32768
    peak = max(abs(x) for x in s) / 32768
    db = 20 * math.log10(rms) if rms > 0 else float("-inf")
    print(f"ch{c}: rms {db:6.1f} dBFS  peak {peak:.3f}  ({'SILENT' if peak < 0.001 else 'signal'})")
EOF
```

Read the two outputs together. The Noican file's `channels:` line says
which virtual output made it: `1` on the current Noican driver (0.2.0),
`2` on stock BlackHole 2ch or the 0.1.0 driver (a `2` on the 1-channel
driver would be QuickTime widening the device — then the two channels
must be identical). "Noican channels alike" below means: every channel
the file has carries the same RMS within 0.1 dB — trivially so for one
channel.

1. Noican channels differ from each other (on a 2-channel file: one
   `SILENT`, or a gap larger than 0.1 dB), or the single channel of a
   1-channel file is `SILENT`: a routing regression — the aggregate path
   must feed every virtual-output channel on this build; check the
   `Aggregate output routing` line and file it.
2. Noican channels alike, far below the direct recording, and the
   popover shows the turned-down/muted line: the Noican Microphone
   slider. Raise it in System Settings and re-record; if it drops again
   without your doing, note which meeting app was running (the log's
   `Virtual output level:` lines carry the scalar and the time).
3. Noican channels alike, no notice shown, and still quieter than the
   direct recording by about the difference the microphone's system
   slider makes: the direct recording was taken with that slider up,
   which Noican does not apply — set the level at the microphone or on
   the Noican Microphone slider instead. Verify by moving the
   microphone's slider and re-recording through Noican: the level must
   not follow.
4. Both recordings equally quiet: the microphone or the room; not
   Noican.

Then, with the engine running on the Noican Microphone:

5. Move the Noican Microphone slider to about 50%: within one second
   the orange line appears under the mode control and the log gains a
   `Virtual output level:` warning with a scalar around 0.5. Move it
   back to maximum: the line disappears within a second and the log
   records the resolution. Repeat with the mute checkbox (Audio MIDI
   Setup shows one for the device): the mute wording appears and
   clears the same way. The level itself must not move on its own at
   any point — Noican never writes it.

## Acceptance checklist (level integrity)

Run the Level integrity procedure above; the build passes when:

1. **Every virtual-output channel fed on the aggregate path**: with the
   built-in microphone (Passthrough, 100%, Noican Microphone slider at
   maximum) the virtual-microphone recording carries the same signal on
   every channel — `channels: 1` on the current 1-channel Noican
   driver, two channels within 0.1 dB on a 2-channel virtual output —
   and channel 0's RMS is within ±1 dB of a same-settings recording on
   the previous build (across the driver swap: within ±1.5 dB of the
   2-channel driver's channel 0, ideally measured in the same session
   by swapping drivers). The `Aggregate output routing` line shows
   `channel map requested [0], read back [0]` on the 1-channel driver
   (`[0, 1]` on a 2-channel virtual output).
2. **The same on a composite device**: the same with the MV7i (or
   another input/output device), and the `Aggregate output routing`
   line shows the one-to-one map read back unchanged after initialize
   — `[-1, -1, 0]` for a stereo headphone jack on the 1-channel driver,
   `[-1, -1, 0, 1]` on a 2-channel virtual output.
3. **Both ears**: the virtual microphone's signal, played back from a
   recording or monitored through a consumer app, is heard in both
   ears of a pair of headphones.
4. **Turned-down/muted notice**: moving the Noican Microphone slider to
   50% shows the notice within one second and moving it back to
   maximum clears it; mute behaves the same with its own wording; the
   log carries each transition with the scalar; the level is never
   changed by Noican.
5. **Everything else as before**: Preview, model switching, and the
   underrun diagnostics (zero on a light model) behave exactly as on
   the earlier records.
6. **Split path level**: a Bluetooth (or other native-rate) headset
   recording is unchanged (every channel fed — `channels: 1` on the
   1-channel driver — at the same level as before; the `Split output
   routing` line's three counts equal the device's channel count), and
   two additional recordings through Noican — the headset's *system*
   input slider at the middle and at maximum — settle whether the
   split transport applies that slider. Record the two RMS values and
   the conclusion, and keep the Level integrity section's second
   bullet in step with the accumulated evidence (first pair recorded
   2026-09-05: +2.2 dB, attributed to speech variance).
