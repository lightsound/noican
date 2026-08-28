#ifndef NOICAN_H
#define NOICAN_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void *noican_engine_create(const char *models_directory);
void noican_engine_destroy(void *handle);
int32_t noican_engine_start(void *handle, uint32_t aggregate_device, const char *model_id);

/* Split transport for microphones that cannot run at the 48 kHz engine
 * rate (issue #7): captures input_device at capture_sample_rate (its
 * current nominal rate in Hz — must be a proper integer divisor of
 * 48000; Bluetooth telephony profiles are 8/16/24 kHz) and feeds
 * output_device (the Noican/BlackHole virtual output) at 48 kHz, with
 * the rate conversion and clock-drift compensation inside the
 * transport. No Aggregate Device is involved; the 48 kHz path
 * (noican_engine_start) is unchanged. */
int32_t noican_engine_start_native(void *handle, uint32_t input_device, uint32_t output_device,
                                   double capture_sample_rate, const char *model_id);
void noican_engine_stop(void *handle);
int32_t noican_engine_set_model(void *handle, const char *model_id);
int32_t noican_engine_is_running(const void *handle);
int32_t noican_engine_is_faulted(const void *handle);
/* Preview pre-flight: reason the current system default output must not
 * receive the preview (virtual loopback or aggregate/multi-output — the
 * built-in speakers are allowed since the self-monitor AEC), or 0 when
 * preview may start. */
size_t noican_monitor_target_error(char *buffer, size_t capacity);
/* Mid-play safety watch: reason the device the running monitor plays on
 * must no longer receive the preview, or 0 while it stays safe (or no
 * monitor is up). Catches the headphone jack flipping to the internal
 * speakers by comparing against the data source recorded at enable time
 * — a preview deliberately started on the speakers keeps playing. */
size_t noican_engine_monitor_unsafe_reason(const void *handle, char *buffer, size_t capacity);
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
