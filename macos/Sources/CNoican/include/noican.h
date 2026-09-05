#ifndef NOICAN_H
#define NOICAN_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void *noican_engine_create(const char *models_directory);
void noican_engine_destroy(void *handle);
/* Aggregate transport for 48 kHz microphones. The aggregate is composed
 * as [microphone, virtual output], and its output channels are the
 * subdevices' output channels in that order — so a microphone with its
 * own outputs (a USB microphone with a headphone jack, an audio
 * interface) sits ahead of the virtual output. virtual_output_first_channel
 * is the number of output channels ahead of the virtual output (the
 * microphone's own output channel count; 0 for the built-in microphone)
 * and virtual_output_channel_count the virtual output's channel count.
 * The transport renders one client channel per virtual output channel
 * (the mono engine signal duplicated into each — dual mono) and routes
 * them to exactly that range with a one-to-one AUHAL channel map
 * (silence everywhere else); it refuses to start
 * when the range does not end at the aggregate's last output channel or
 * is empty; the capture side (aggregate input channel 0 = microphone) is
 * unchanged. */
int32_t noican_engine_start(void *handle, uint32_t aggregate_device,
                            uint32_t virtual_output_first_channel,
                            uint32_t virtual_output_channel_count, const char *model_id);

/* Split transport for microphones that cannot run at the 48 kHz engine
 * rate (issue #7): captures input_device at capture_sample_rate (its
 * current nominal rate in Hz — whole hertz from 8000 to 192000 whose
 * ratio to 48000 reduces to small terms, i.e. the standard rate
 * families: Bluetooth telephony profiles at 8/16/24 kHz, the 44.1 kHz
 * family, 88.2/96 kHz interfaces; an exotic rate such as 16001 Hz is
 * refused) and feeds output_device (the Noican/BlackHole
 * virtual output) at 48 kHz, with the exact-ratio rate conversion and
 * clock-drift compensation inside the transport. No Aggregate Device is
 * involved; the 48 kHz path (noican_engine_start) is unchanged. */
int32_t noican_engine_start_native(void *handle, uint32_t input_device, uint32_t output_device,
                                   double capture_sample_rate, const char *model_id);
void noican_engine_stop(void *handle);
int32_t noican_engine_set_model(void *handle, const char *model_id);
int32_t noican_engine_is_running(const void *handle);
int32_t noican_engine_is_faulted(const void *handle);
size_t noican_monitor_target_error(char *buffer, size_t capacity);
size_t noican_monitor_device_error(uint32_t device, char *buffer, size_t capacity);
int32_t noican_engine_set_monitor(void *handle, int32_t enabled);
uint32_t noican_engine_monitor_device(const void *handle);

/* Preview monitor state, one lock-free read (never waits on the control
 * lock, so it is safe at UI poll rates). TRIPPED means the feedback
 * guard silenced the preview while the monitor AUHAL is still up; the
 * next noican_engine_set_monitor call in either direction clears it
 * (enable re-arms, disable tears down). Values mirror the Rust
 * MonitorState enum and are frozen. */
typedef enum {
  NOICAN_MONITOR_OFF = 0,
  NOICAN_MONITOR_PLAYING = 1,
  NOICAN_MONITOR_TRIPPED = 2,
} NoicanMonitorState;
int32_t noican_engine_monitor_state(const void *handle);
float noican_engine_input_level(const void *handle);
float noican_engine_output_level(const void *handle);

/* Dry/wet intensity ("strength"): 1.0 = fully processed (default),
 * 0.0 = raw microphone. One atomic — the setter never blocks or rebuilds
 * the engine (safe at UI slider rates, and while stopped: the value
 * seeds the next start). Out-of-range values are clamped; non-finite
 * values are ignored. The dry path is delay-compensated by the active
 * model's reported latency, and the same mix feeds the virtual
 * microphone and the preview monitor. */
int32_t noican_engine_set_intensity(void *handle, float intensity);
float noican_engine_intensity(const void *handle);
uint64_t noican_engine_frames_processed(const void *handle);

/* Diagnostics for real-time budget violations (all 0 while stopped or
 * for a null handle). output_underruns counts output callbacks that
 * delivered no real audio at all — the output ring dry for the entire
 * I/O period — after the ring first carried real audio (start-up ramp
 * excluded; partial zero-fills are benign block-quantization jitter
 * and are not counted) — a growing count means
 * the inference worker misses its 10 ms block budget for the active
 * model, audible as dropouts in recordings from the virtual microphone
 * (the preview monitor masks it behind its re-priming cushion).
 * worker_blocks / worker_blocks_over_budget / worker_block_max_ns are
 * the worker's per-block processing-time statistics behind that: total
 * blocks, blocks that exceeded 10 ms, and the single longest block in
 * nanoseconds. Counters are cumulative since engine start or the last
 * noican_engine_reset_debug_stats call; reset after a model switch to
 * attribute the numbers to one model. worker_realtime is 1 when the
 * inference worker's mach time-constraint (real-time) promotion
 * succeeded at engine start; 0 while running means the promotion failed
 * and budget misses may be scheduling, not model cost. It reports the
 * start-time result, not live membership (the kernel may demote a
 * persistently overrunning thread later). Reads take the control
 * mutex (1 Hz diagnostics, not the 20 Hz meter path); the reset is a
 * no-op while stopped. */
uint64_t noican_engine_output_underruns(const void *handle);
uint64_t noican_engine_worker_blocks(const void *handle);
uint64_t noican_engine_worker_blocks_over_budget(const void *handle);
uint64_t noican_engine_worker_block_max_ns(const void *handle);
int32_t noican_engine_worker_realtime(const void *handle);
void noican_engine_reset_debug_stats(void *handle);

/* Diagnostic: how the running aggregate transport routes the engine
 * output into the Aggregate Device (the aggregate's reported output
 * channel count, the AUHAL channel map requested, and the map read back
 * after AudioUnitInitialize). Copies the description as UTF-8 and returns the
 * required byte count including the terminating NUL; 0 while stopped, on
 * the split transport, or for a null handle. Takes the control mutex —
 * for the one-time start log, not the poll path. */
size_t noican_engine_routing_description(const void *handle, char *buffer, size_t capacity);

size_t noican_engine_last_error(const void *handle, char *buffer, size_t capacity);

size_t noican_model_count(void);
size_t noican_model_id(size_t index, char *buffer, size_t capacity);
size_t noican_model_display_name(size_t index, char *buffer, size_t capacity);
int32_t noican_model_needs_enrollment(size_t index);

/* Picker-facing model characteristics. Ratings are 0-5 with "more is
 * better" on every axis (latency is exposed as responsiveness, compute
 * cost as efficiency); the tagline is a one-line purpose tag for picker
 * rows and the details string carries the raw facts (native rate,
 * measured delay, size) for tooltips. Values mirror the Rust
 * ModelTraits registry and are data, not UI copy baked into the app. */
typedef enum {
  NOICAN_TRAIT_NOISE_REMOVAL = 0,
  NOICAN_TRAIT_VOICE_QUALITY = 1,
  NOICAN_TRAIT_RESPONSIVENESS = 2,
  NOICAN_TRAIT_EFFICIENCY = 3,
} NoicanModelTrait;
int32_t noican_model_rating(size_t index, int32_t trait_id);
size_t noican_model_tagline(size_t index, char *buffer, size_t capacity);
size_t noican_model_details(size_t index, char *buffer, size_t capacity);

#ifdef __cplusplus
}
#endif

#endif
