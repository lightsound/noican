/*
 * C ABI over the noican engine.
 *
 * Written by hand and kept in step with `crates/noican-ffi/src/lib.rs`, whose
 * tests assert that the two agree on every struct's size and on the catalog's
 * contents. Strings are fixed-size arrays rather than owned pointers, so
 * nothing here has to be freed except the engine handle itself.
 *
 * Threading: every function may be called from the main thread. The engine
 * handle is not thread-safe; call into it from one thread only, which for a
 * SwiftUI app means the main actor.
 */

#ifndef NOICAN_H
#define NOICAN_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Longest device name or identifier the ABI carries, including the
 * terminator. Longer values are truncated rather than rejected. */
#define NOICAN_STRING_CAPACITY 256

/* An audio device the user can choose. */
typedef struct {
  /* Persistent identifier; remember this, not the name or the index. */
  char uid[NOICAN_STRING_CAPACITY];
  /* Name as shown in Sound Settings. */
  char name[NOICAN_STRING_CAPACITY];
  uint32_t input_channels;
  uint32_t output_channels;
  uint32_t sample_rate;
  /* True when the HAL reports the device as virtual, which is how the
   * virtual output device is identified without matching on its name. */
  bool is_virtual;
} NoicanDevice;

/* A model in the catalog. */
typedef struct {
  char id[NOICAN_STRING_CAPACITY];
  char display_name[NOICAN_STRING_CAPACITY];
  uint32_t sample_rate;
  /* True once the weights are on disk and pass their checksum. */
  bool downloaded;
  /* False for models that can only run a block at a time, which costs seconds
   * of latency. They are still selectable -- offline comparison is what they
   * are for -- but a picker must say so, or a user who chooses one gets
   * several seconds of silence and assumes the app is broken. */
  bool live_capable;
} NoicanModel;

/* What the engine is doing right now. */
typedef struct {
  bool running;
  bool bypassed;
  bool switching;
  /* Times the audio callback found no audio ready and emitted silence.
   * Anything above zero is audible. */
  uint64_t dropouts;
  /* Peak levels since the previous call, in [0, 1]. */
  float input_peak;
  float output_peak;
  /* End-to-end delay of the active model. */
  float latency_ms;
} NoicanStatus;

/* Opaque engine handle. */
typedef struct NoicanEngine NoicanEngine;

/* Initialises logging. Safe to call more than once; only the first call has
 * any effect. */
void noican_init_logging(void);

/* The most recent error message, or an empty string. The pointer is valid
 * until the next call on the same thread. */
const char *noican_last_error(void);

/* --- Catalog ------------------------------------------------------------ */

/* Number of models in the catalog. */
size_t noican_model_count(void);

/* Writes up to `capacity` models into `out`, returning how many were written.
 * Pass NULL to query the count without writing. */
size_t noican_models(NoicanModel *out, size_t capacity);

/* Downloads the weights for `model_id`, verifying their checksum. Blocking;
 * call it off the main thread. Returns true on success. */
bool noican_fetch_model(const char *model_id);

/* --- Devices ------------------------------------------------------------ */

/* Writes up to `capacity` capture-capable devices into `out`, returning how
 * many were written. */
size_t noican_input_devices(NoicanDevice *out, size_t capacity);

/* Writes up to `capacity` playback-capable devices into `out`. */
size_t noican_output_devices(NoicanDevice *out, size_t capacity);

/* Writes the UID of the most likely virtual output device into `out`, which
 * must hold NOICAN_STRING_CAPACITY bytes. Returns false when none was found. */
bool noican_suggested_output_uid(char *out);

/* Writes the UID of the system's current microphone into `out`, which must
 * hold NOICAN_STRING_CAPACITY bytes. Returns false on failure. */
bool noican_default_input_uid(char *out);

/* --- Engine ------------------------------------------------------------- */

/* Creates a stopped engine. Returns NULL on failure. */
NoicanEngine *noican_engine_new(void);

/* Stops the engine if it is running and frees it. NULL is accepted. */
void noican_engine_free(NoicanEngine *engine);

/* Starts capture from `input_uid` into `output_uid` with `model_id` active.
 * Returns true on success; on failure call noican_last_error. */
bool noican_engine_start(NoicanEngine *engine, const char *input_uid,
                         const char *output_uid, const char *model_id);

/* Stops capture. Safe to call when already stopped. */
void noican_engine_stop(NoicanEngine *engine);

/* Switches to a different model, ramping so the change does not click.
 * Returns true on success. */
bool noican_engine_set_model(NoicanEngine *engine, const char *model_id);

/* Bypasses or re-enables the active model. */
void noican_engine_set_bypass(NoicanEngine *engine, bool bypassed);

/* Reads the current status. Peak levels are reset by the read, so poll at a
 * steady rate to get meaningful meters. */
void noican_engine_status(NoicanEngine *engine, NoicanStatus *out);

/* Identifier of the active model, or an empty string when stopped. The
 * pointer is valid until the next call on the same thread. */
const char *noican_engine_active_model(NoicanEngine *engine);

#ifdef __cplusplus
}
#endif

#endif /* NOICAN_H */
