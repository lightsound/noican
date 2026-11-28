# Hardware test: Split transport on 48 kHz microphones (experiment)

Part of the [macOS Build and Hardware Test Plan](../macos-hardware-test.md).

## Split transport on 48 kHz microphones (experiment)

48 kHz-capable microphones take the aggregate transport. This
procedure measures what it would cost to run them on the split
transport instead, which is the evidence needed before the aggregate
transport can be retired. The criteria for retiring it are an owner
decision that has not been taken; this procedure produces the numbers
and has no acceptance checklist yet.

The environment variable `NOICAN_SPLIT_CUSHION_MS` routes every
microphone to the split transport and sets its output cushion in
milliseconds (a whole number from 1 to 500; anything else refuses the
start with a message naming the variable). Unset, the app behaves as
shipped.

```bash
open --env NOICAN_SPLIT_CUSHION_MS=50 dist/Noican.app
```

If `open` does not pass the variable through, set it for the login
session with `launchctl setenv NOICAN_SPLIT_CUSHION_MS 50`, launch the
app normally, and clear it afterwards with
`launchctl unsetenv NOICAN_SPLIT_CUSHION_MS`.

Keep the log stream of
[output-underrun-diagnostics.md](output-underrun-diagnostics.md) open
for every step. On the split transport the underrun counter counts
every output callback the ring could not fill completely.

1. **Baseline.** Launch without the variable, start a 48 kHz
   microphone, and confirm the log says `Aggregate output routing`.
   Measure the delay from the physical microphone to the virtual
   microphone (record a sharp click on both at once and compare the
   two tracks).
2. **Routing line.** Quit, relaunch with the variable at 50, start the
   same microphone. The log must say `Split output routing` and end
   with `cushion 2400 samples, capture buffer N frames, render buffer
   M frames, phase estimate E samples`. Record N, M and E. E is a
   rough estimate of the cushion needed before scheduling jitter, not
   a cutoff: the simulator is gap-free somewhat below it in some
   configurations, so let the sweep decide.
3. **Sweep.** For each cushion of 50, 40, 35, 30, 25 and 20 ms, in
   that order: relaunch, run ten minutes of speech with the default
   model, and record the underrun count and the delay measured as in
   step 1. Stop descending at the first cushion that underruns.
4. **Endurance.** With the smallest cushion that stayed at zero, run
   the two-hour procedure of
   [clock-drift-and-endurance.md](clock-drift-and-endurance.md).
5. **Disconnect.** With that cushion, unplug the microphone mid-run
   and plug it back in. Unplugging ends the session (the app stops
   with "Microphone disconnected"), and a Bluetooth profile change
   that alters the rate starts a new one, so this step checks the stop
   and the restart, not recovery inside a session: the app stops
   without an engine fault, starts again on request, logs a new
   routing line, and counts underruns from zero. Recovery inside one
   session after a capture stall is covered by the simulator only. If
   a stall that keeps the device and its rate happens during any run
   (a Bluetooth glitch), note whether the underrun count stops growing
   within a few seconds.
6. **Shipped cushion on Bluetooth.** Without the variable, start a
   Bluetooth microphone in its telephony profile and read the routing
   line. If the cushion is near or below the estimate, a periodic
   underrun is likely; repeat with the variable at 60 and compare.

Record, per run: microphone, the routing line, cushion, duration,
underruns, worker blocks over budget, and measured delay. What the
simulator predicts for comparison (ideal scheduling, ±300 ppm): the
smallest gap-free cushion is 35 ms for 48 kHz at 512-frame buffers,
25 ms at 256 frames and 20 ms at 128 frames; the aggregate transport
settles at about 9 ms of buffered output.
