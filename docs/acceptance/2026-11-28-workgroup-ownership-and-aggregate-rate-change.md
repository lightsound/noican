# Result record — workgroup ownership (L5) and rate change on the aggregate transport (L6), 2026-11-28

Evidence for two findings of the pre-release bug hunt of 2026-11-27,
gathered on the owner's Mac in one day. Part A settles who owns the
`os_workgroup` reference that AUHAL hands out (L5, fixed in PR #80).
Part B observes what happens when a device's nominal sample rate
changes while the engine runs (L6). The run was operator-driven (app
quit, device selection, Off/On/Preview, watching and listening) with
rate changes, logging and the probe program scripted by the agent on
the same Mac. Every statement below is marked **observed**, **read**
(in a named source) or **inferred**.

## Environment

| Item | Value |
|---|---|
| Mac | MacBook Air (arm64), not a VM |
| macOS / Xcode / Swift | macOS 27.2.1 (26B131) / Xcode 27.2 (27B52), SDK MacOSX 27.2 / Swift 6.4.2 |
| Rust | rustc 1.100.0 (pinned by `rust-toolchain.toml`) |
| Part A | A standalone C program outside the repository; no Noican code involved. Code sites read at `main` `17ce6f7` |
| Part B app commit | `main` `25169f8` (merge of PR #80: the L5 fix and the build-script fix), built with the unmodified `bash scripts/build-macos-app.sh` in a separate worktree; ad-hoc signed, `com.apple.security.device.audio-input` present |
| App processes | PID 50030 (launched 20:54:47 JST, `NOICAN_SPLIT_CUSHION_MS` unset in the shell and in launchd) for B1–B4 and the listening checks; PID 71002 (launched 21:20:22 with `NOICAN_SPLIT_CUSHION_MS=50`) for B5 |
| Microphone | built-in "MacBook Airのマイク" (`BuiltInMicrophoneDevice`, in 1; advertises 44100, 48000, 88200, 96000) |
| Virtual output | Noican Microphone (`com.lightsound.noican.mic_UID`, in 1 / out 1; advertises 44100, 48000); the Noican driver, not stock BlackHole |
| Other devices | built-in speakers (out 2); HUAWEI FreeClip over Bluetooth (input 16000, output 44100), used by the operator as the listening output; "CleanShot Aggregate Device" |
| Model | dfn3 (named in the app's log lines) |
| Logging | `log stream --predicate 'subsystem == "com.lightsound.noican"' --level debug`; `coreaudiod` lines for the same period; device rates read every 10 s with a small tool that reads and sets `kAudioDevicePropertyNominalSampleRate` by device UID |

All times are JST on 2026-11-28. No pull request's code was changed
for this run; the microphone picker was used as shipped.

## Part A — who owns the workgroup reference (L5)

**The question.** `audio_workgroup` in
`crates/noican-coreaudio/src/macos/mod.rs` reads
`kAudioOutputUnitProperty_OSWorkgroup`; at `17ce6f7` nothing released
the result. Whether that leaked depended on whether the getter returns
a reference the caller owns.

### SDK headers (read, SDK MacOSX 27.2)

`AudioToolbox.framework/Versions/A/Headers/AudioUnitProperties.h`, at
`kAudioOutputUnitProperty_OSWorkgroup`:

> Note that as an os_object subclass, workgroup objects are
> reference-counted, and that AudioUnitGetProperty returns a +1
> reference, which the client is responsible for releasing when it is
> finished with it.

`CoreAudio.framework/Versions/A/Headers/AudioHardware.h`, at
`kAudioDevicePropertyIOThreadOSWorkgroup`:

> An os_workgroup_t that represents the thread workgroup the
> AudioDevice's IO thread belongs to. The caller is responsible for
> releasing the returned object.

`AudioToolbox.framework/Versions/A/Headers/AudioWorkInterval.h`:

> Core Audio devices own a workgroup whose lifetime is the same as
> that of the device. […] For Audio I/O units which are associated
> with an audio device, that device-owned workgroup is readable as an
> Audio Unit property. […] If an I/O unit's device assignment changes,
> as a side effect, its workgroup will also change.

`usr/include/os/workgroup_object.h` and `os/object.h` describe
`os_workgroup_t` as a reference-counted os object with `os_retain` /
`os_release`; the texts of `os_workgroup_join` and `os_workgroup_leave`
say nothing about ownership. Apple's web documentation and sample
code (14 pages read on 2026-11-28) do not state the rule.

### Probe program

Built with `clang -Wall -O0 -g -o wgprobe wgprobe.c -framework
AudioToolbox -framework CoreAudio -framework CoreFoundation -lobjc`.
It opens an AUHAL output unit on the default output device (the
built-in speakers), renders silence, starts the unit, runs one mode,
then stops and disposes of the unit.

```c
#include <AudioToolbox/AudioToolbox.h>
#include <AudioUnit/AudioUnit.h>
#include <CoreAudio/CoreAudio.h>
#include <os/workgroup.h>
#include <objc/message.h>
#include <objc/runtime.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <unistd.h>

#define CHECK(x, what) do { OSStatus s_ = (x); if (s_ != noErr) { fprintf(stderr, "%s failed: %d\n", what, (int)s_); exit(2); } } while (0)

static OSStatus render(void *ref, AudioUnitRenderActionFlags *flags, const AudioTimeStamp *ts,
                       UInt32 bus, UInt32 frames, AudioBufferList *io) {
    for (UInt32 i = 0; i < io->mNumberBuffers; i++) memset(io->mBuffers[i].mData, 0, io->mBuffers[i].mDataByteSize);
    *flags |= kAudioUnitRenderAction_OutputIsSilence;
    return noErr;
}

static unsigned long rc(void *obj) {
    return ((unsigned long (*)(id, SEL))objc_msgSend)((id)obj, sel_registerName("retainCount"));
}

static os_workgroup_t get_wg(AudioUnit u) {
    os_workgroup_t wg = NULL; UInt32 sz = sizeof(wg);
    CHECK(AudioUnitGetProperty(u, kAudioOutputUnitProperty_OSWorkgroup, kAudioUnitScope_Global, 0, &wg, &sz), "AudioUnitGetProperty(OSWorkgroup)");
    if (!wg) { fprintf(stderr, "null workgroup\n"); exit(3); }
    return wg;
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "twice";
    int n = argc > 2 ? atoi(argv[2]) : 100000;
    setvbuf(stdout, NULL, _IONBF, 0);
    AudioComponentDescription d = { kAudioUnitType_Output, kAudioUnitSubType_HALOutput, kAudioUnitManufacturer_Apple, 0, 0 };
    AudioComponent c = AudioComponentFindNext(NULL, &d);
    if (!c) { fprintf(stderr, "no AUHAL\n"); return 2; }
    AudioUnit u; CHECK(AudioComponentInstanceNew(c, &u), "AudioComponentInstanceNew");
    AudioObjectPropertyAddress a = { kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
    AudioDeviceID dev = 0; UInt32 sz = sizeof(dev);
    CHECK(AudioObjectGetPropertyData(kAudioObjectSystemObject, &a, 0, NULL, &sz, &dev), "default output");
    CHECK(AudioUnitSetProperty(u, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, &dev, sizeof(dev)), "CurrentDevice");
    AURenderCallbackStruct cb = { render, NULL };
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &cb, sizeof(cb)), "SetRenderCallback");
    CHECK(AudioUnitInitialize(u), "AudioUnitInitialize");
    CHECK(AudioOutputUnitStart(u), "AudioOutputUnitStart");
    printf("mode=%s n=%d device=%u pid=%d\n", mode, n, (unsigned)dev, getpid());

    if (!strcmp(mode, "twice")) {
        os_workgroup_t a1 = get_wg(u), a2 = get_wg(u);
        printf("read1=%p read2=%p same=%d\n", (void *)a1, (void *)a2, a1 == a2);
    } else if (!strcmp(mode, "count")) {
        os_workgroup_t w = get_wg(u);
        printf("class=%s retainCount after 1 read=%lu\n", object_getClassName((id)w), rc(w));
        for (int i = 0; i < 9; i++) get_wg(u);
        printf("retainCount after 10 reads=%lu\n", rc(w));
        for (int i = 0; i < 10; i++) os_release(get_wg(u));
        printf("retainCount after 10 more read+release=%lu\n", rc(w));
        os_workgroup_t dw = NULL; UInt32 dsz = sizeof(dw);
        AudioObjectPropertyAddress wa = { kAudioDevicePropertyIOThreadOSWorkgroup, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
        OSStatus s = AudioObjectGetPropertyData(dev, &wa, 0, NULL, &dsz, &dw);
        printf("device IOThreadOSWorkgroup status=%d ptr=%p same_as_unit=%d rc=%lu\n", (int)s, (void *)dw, dw == w, dw ? rc(dw) : 0);
        os_workgroup_join_token_s tok; int j = os_workgroup_join(w, &tok);
        printf("join on main thread=%d\n", j); if (j == 0) os_workgroup_leave(w, &tok);
    } else if (!strcmp(mode, "release") || !strcmp(mode, "keep")) {
        int rel = !strcmp(mode, "release");
        os_workgroup_t first = get_wg(u), last = NULL;
        unsigned long rc0 = rc(first);
        int distinct = 0;
        for (int i = 0; i < n; i++) {
            os_workgroup_t w = get_wg(u);
            if (w != first) distinct++;
            last = w;
            if (rel) os_release(w);
        }
        printf("first=%p last=%p distinct_from_first=%d rc_before=%lu rc_after=%lu\n", (void *)first, (void *)last, distinct, rc0, rc(first));
        usleep(200000);
    } else { fprintf(stderr, "unknown mode\n"); return 64; }

    CHECK(AudioOutputUnitStop(u), "AudioOutputUnitStop");
    CHECK(AudioUnitUninitialize(u), "AudioUnitUninitialize");
    CHECK(AudioComponentInstanceDispose(u), "AudioComponentInstanceDispose");
    printf("disposed cleanly\n");
    return 0;
}
```

### Measured (verbatim, first run of each mode)

Each mode ran twice as a separate process. All eight runs exited 0
with empty stderr; the second runs differ only in pointer values and
PIDs. No new file appeared in `~/Library/Logs/DiagnosticReports`.

```
mode=twice n=100000 device=93 pid=47476
read1=0x78a0c40380 read2=0x78a0c40380 same=1
disposed cleanly

mode=count n=100000 device=93 pid=47486
class=OS_os_workgroup retainCount after 1 read=2
retainCount after 10 reads=11
retainCount after 10 more read+release=11
device IOThreadOSWorkgroup status=0 ptr=0x7846c40380 same_as_unit=1 rc=12
join on main thread=0
disposed cleanly

mode=release n=100000 device=93 pid=47493
first=0x7a0ec40380 last=0x7a0ec40380 distinct_from_first=0 rc_before=2 rc_after=2
disposed cleanly

mode=keep n=100000 device=93 pid=47500
first=0x75cac40380 last=0x75cac40380 distinct_from_first=0 rc_before=2 rc_after=100002
disposed cleanly
```

`leaks --atExit -- ./wgprobe keep` (the run that keeps 100,000
references, `rc_after=100002`):

```
Process 47558: 5845 nodes malloced for 713 KB
Process 47558: 0 leaks for 0 total leaked bytes.
```

### Conclusion for L5

- **Read:** the SDK header states that `AudioUnitGetProperty` returns
  a +1 reference the client must release.
- **Observed:** every read raises the retain count by one; a release
  per read keeps it level; 100,000 paired reads and releases neither
  crash nor change the count; the unit and the device return the same
  object.
- **Observed:** `leaks` reports nothing for 100,000 kept references.
  **Inferred:** the over-retained object stays reachable from the
  device, so `leaks` cannot see this defect and a clean `leaks` run
  must not be cited as evidence either way.
- **Conclusion:** the caller owns the reference. At `17ce6f7` each
  engine start on either transport leaked one. PR #80 releases it
  (ADR 0001, `audio-workgroup-ownership`).

## Part B — sample-rate changes under a running engine (L6)

**The question.** On the split transport the app observes the
microphone's nominal rate and restarts the transport when it changes
(`syncInputRateObservation` and `checkInputRate` in
`macos/Sources/NoicanMenuBar/ListenerObservation.swift`, gated on
`activeCaptureRate != nil`). On the aggregate transport nothing
observes it (read). What a user gets there had not been observed.

**Method.** For each scenario the agent set a device's nominal rate,
verified it, and held that state while the operator opened the
popover, spoke, and reported the status dot and text, whether the
Before and After bars moved, and whether red text was shown. The
operator's answers are quoted in the original Japanese.

### Transport of the first launch (observed)

```
20:56:25.459 Aggregate composed: microphone "MacBook Airのマイク" (in 1 / out 0), virtual output "Noican Microphone" (out 1); aggregate reports 1 output channel(s) in 1 stream(s)
20:56:26.697 Aggregate output routing: aggregate output channels 1, virtual output at channels 0..1, channel map requested [0], channel map read back after initialize [0] (device output channels then 1)
```

`coreaudiod`, 20:56:25.626: `IOWorkLoopInit: 6024
BuiltInMicrophoneDevice (com.lightsound.noican-aggregate.8AB99835-…):
starting`. IO context 6024 is this aggregate; 6491 is the second
aggregate created at 21:09:17 (B4).

### Baseline (20:57:36–20:58:37, everything at 48000)

Observed: no Noican log line. Read (`EngineDiagnostics.swift`): the
app logs counters only when they grow and at start, so silence means
no underrun, overrun or fault was counted.

### Scenarios

| # | Change while running | Noican log (verbatim) | `coreaudiod` (observed) | Operator's answer (verbatim) |
|---|---|---|---|---|
| B1, first pass | Microphone 48000 → 44100 at 20:58:37, held 60 s, back at 20:59:39 | `20:58:37.745 Input overruns: 837 samples dropped (model: dfn3) — the worker fell behind the capture ring, or a capture cycle failed`; nothing else | `changeSamplingRate:(44100.000000)` .476; `IOWorkLoopDeinit: 6024 … stopping with error 0` .485; `IOWorkLoopInit: 6024 … starting` .523; SRC line (below) .523. On the way back: Deinit .517, Init .552, no SRC line | not watching |
| B1, repeat | Microphone → 44100 at 21:04:14, held until answered, back at 21:05:28 | `21:04:15.879 Input overruns: 1674 samples dropped (model: dfn3) …` (cumulative; +837) | same pattern: change .801, Deinit .805, Init .843, SRC line .843 | 「丸の色と、その下の小さな文字　緑色とRunning／Before と After のバーが動くか　動いてました／赤い文字が出ているか　でていません」 |
| B2 | Microphone → 96000 at 21:05:45, back at 21:07:07 | none | `changeSamplingRate:(96000.000000)` .578, then five Deinit/Init pairs (.617, .659, .697, .741, .778), each Init with the SRC line | 「さっきと同じ結果です。」 |
| B3 | Noican Microphone → 44100 at 21:07:24 (microphone stays at 48000), back at 21:08:28 | none | Deinit .308, Init .401, SRC line .401 | 「さっきと同じ結果です。」 |
| B4 | Operator switches Off (21:08:57); microphone → 44100 at 21:09:05; operator switches On | `21:09:17.416 Aggregate composed: …`, `21:09:18.603 Aggregate output routing: …` (same texts as above); no overrun | `changeSamplingRate:(48000.000000)` at 21:09:17.343 / .411 / .412; `IOWorkLoopInit: 6491 … (com.lightsound.noican-aggregate.A8BCB3CB-…): starting` .515 | 「基本同じですが、Afterのバーだけいつもより反応が薄い気がします。」 |
| B5 (control, split) | App relaunched with `NOICAN_SPLIT_CUSHION_MS=50`; operator switches On; microphone → 44100 at 21:20:57, back at 21:21:58 | `21:20:42.468 Split output routing: virtual output channels 1, render format requested 1 ch, render format read back after initialize 1 ch, cushion 2400 samples, capture buffer 512 frames, render buffer 512 frames, phase estimate 1504 samples`; after the change `21:20:58.400 Engine transport diagnostics: worker realtime scheduling true, Rosetta-translated process false` and `21:20:58.400 Split output routing: … phase estimate 1550 samples`; after the change back `21:22:00.002 Split output routing: … phase estimate 1504 samples`; no overrun | `changeSamplingRate:(44100.000000)` .088; no aggregate lines | 「緑、Running／Before と After のバーが動く／赤い文字話」, corrected by the operator six seconds later: 「赤い文字は無し」 |

The SRC line, verbatim, seen at every change away from 48000 on the
aggregate (B1 twice, B2, B3, and both listening checks) and never on
a change back to 48000:

```
HALS_IOProcessorFactoryWorker::do_allocate_processor: sample rate conversion no longer enables drift correction by default [IOC: 6024]
HALS_IOContext_InputGraph_StreamUtils::get_drift_correction_settings: sample rate conversion no longer enables drift correction by default
```

Observed in every aggregate scenario: the changed device kept the
changed rate for the whole hold, the other devices stayed at 48000,
and the app kept running. In B4 the microphone was at 48000 again
right after On although the agent had not set it back. Read
(`AggregateDevice.swift`, `create`): the app calls `ensure48k` on the
microphone before it composes the aggregate and then sets the
aggregate to 48000.

### Listening checks (Preview, aggregate transport, second aggregate)

The operator switched to Preview and listened to the processed signal
on the Bluetooth earphones.

First pass. **Caveat:** a video was playing loudly in the same
earphones during this pass, so the remarks on sound quality are
unreliable; the remark on pitch and speed is supported by the SRC
lines.

| State | Noican log | Operator's answer (verbatim) |
|---|---|---|
| 48000 (21:12:04) | — | 「聞こえる」 |
| Microphone → 44100 at 21:12:08 | `21:12:09.898 Input overruns: 837 samples dropped (model: dfn3) …` | 「さっきより良くなった気がします。」 |
| Microphone → 96000 at 21:12:57 | none | 「喋った瞬間にちょっとした破裂音のようなものが聞こえるように感じます。めちゃくちゃ気になる程度ではないですが。」「高さや速さは同じで音質が良くなった」 |

Second pass, with the video stopped:

| State | Operator's answer (verbatim) |
|---|---|
| 48000 (after 21:14:22) | 「ちょっと破裂音がする気がしますね。なんというかクリックとか、トラックパッドを押した時のような音が聞こえます。」 |
| Microphone → 96000 at 21:18:05 | 「声が小さいときに出やすい気がします。小さい声で話して、発声が終わる瞬間に聞こえることが多い気がします。」 Compared with 48000: 「だいたい同じ」 |

### Conclusions for L6

- **Observed:** on the aggregate transport the app does not react to
  a rate change of the microphone or of the virtual output: no
  restart, no fault, no red text; the status stays green and
  "Running" and both level bars keep moving.
- **Observed:** at each change `coreaudiod` stops and restarts the
  aggregate's IO (five times in a row for 96000); the logged gap
  between stop and start was 30 to 40 ms for the microphone changes
  and 93 ms for the virtual-output change (B3). The gaps are
  log timestamps, not a measurement of how long audio was
  interrupted. For a change away from 48000 it also logs that it
  allocates sample rate conversion. **Inferred:** Core Audio converts the out-of-rate
  sub-device to the aggregate's 48 kHz, which is why the operator
  heard no change in pitch or speed.
- **Observed:** at each change of the microphone to 44100 the app's
  input-drop counter rose by 837 samples, three times out of three
  (837 samples are 17 ms of audio at 48 kHz). The counter is
  cumulative and counts every kind of capture loss, so this is not a
  measurement of one contiguous dropout, and the audible interruption
  during the restart was not measured. The changes to 96000 and the
  change of the virtual output raised no counter; why is not known.
- **Observed and read:** every start resets the microphone to 48000
  (`ensure48k`), so the aggregate transport does not start on a
  microphone left at another rate.
- **Observed:** on the split transport the app rebuilt the transport
  about one second after each change, as designed.
- **Answer to the finding's question:** in this run a rate change on
  the aggregate transport did not leave the app in a state the user
  could not notice or could not recover from. The missing observer is
  real, but its recorded cost here was that counter increase and a
  restart of the aggregate's IO at the moment of the change. The
  operator's answers describe the state after each change (status,
  bars, red text) and, in Preview, pitch and sound quality; they were
  not asked about the moment of the change, so whether the restart
  was audible is unverified.
- **Not settled:** the SRC line is a statement about Core Audio's
  default ("sample rate conversion no longer enables drift correction
  by default"), not about this session. Read
  (`AggregateDevice.swift`, `create`): the app requests drift
  compensation for the virtual output sub-device and not for the
  microphone, which is the aggregate's clock. Whether drift
  correction was in effect during any conversion was not verified,
  and whether a session that stays at a foreign rate for hours runs
  into dropouts was not tested; the longest hold was about 80 s.

## Other observations

- **L5 fix on hardware (observed).** With `25169f8` the engine was
  started at 20:56, 21:09 and 21:20 and by the two split rebuilds,
  and stopped by one Off, two app quits and the split rebuilds. No
  start logged a workgroup failure, both app processes quit normally,
  and no Noican report appeared in `~/Library/Logs/DiagnosticReports`.
- **Build script on Swift 6.4.2 (observed).** The script as merged in
  PR #80 exited 0 and produced `dist/Noican.app`. At `17ce6f7` it
  stopped with "swift build finished but
  …/macos/.build/arm64-apple-macosx/release/NoicanMenuBar does not
  exist" after a successful build, because the product was at
  `.build/out/Products/Release/`.
- **Click at the end of quiet speech (observed by the operator,
  unexplained).** Heard in Preview at 48000 and at 96000 alike, with
  the video stopped, hands off the machine, mostly when a quietly
  spoken phrase ends. It does not depend on the rate. **Inferred, not
  verified:** an artifact of the suppression closing (model dfn3), or
  of the listening chain. Not investigated in this run.
- **Input-volume notice (observed).** At every start the app logged
  `Virtual output level: "Noican Microphone" volume at 88% (scalar
  0.882, below unity) — consumers hear the engine output attenuated;
  not changed by Noican` and showed the orange notice. The slider in
  System Settings was at 88% before the run; unrelated to L6.
- **Device object IDs (observed, unexplained).** Around 21:07 the
  object IDs of the built-in microphone, the speakers and the
  Bluetooth device each dropped by one (100→99, 93→92, 111→110,
  105→104) while `coreaudiod` kept running; Noican Microphone kept 52.

## Limits of this run

- One Mac, the built-in microphone, loopback through the Noican
  driver. Nothing here speaks about USB or Bluetooth microphones that
  change rate by themselves, or about clock drift between real
  devices.
- The listening checks are one person's impression over Bluetooth
  earphones, partly with other audio playing. No recording was made,
  to avoid new permission prompts, so there is no measured level,
  spectrum or pitch.
- The private aggregate device is not visible to other processes, so
  its own nominal rate was not read; its behaviour is taken from the
  `coreaudiod` lines.
- Rates were set by a script, not by a device or by Audio MIDI Setup.
- This is evidence about `25169f8` on this machine on this date. It
  does not measure what the decision on retiring the aggregate
  transport needs (underruns over hours, latency).
