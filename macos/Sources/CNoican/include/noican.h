/* Generated from crates/noican-ffi/src/lib.rs by cbindgen; do not edit.
 * Regenerate: NOICAN_UPDATE_HEADER=1 cargo test -p noican-ffi header */

#ifndef NOICAN_H
#define NOICAN_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef enum NoicanMonitorState {
  NOICAN_MONITOR_STATE_OFF = 0,
  NOICAN_MONITOR_STATE_PLAYING = 1,
  NOICAN_MONITOR_STATE_TRIPPED = 2,
} NoicanMonitorState;

typedef enum NoicanModelState {
  NOICAN_MODEL_STATE_MISSING = 0,
  NOICAN_MODEL_STATE_FETCHING = 1,
  NOICAN_MODEL_STATE_READY = 2,
  NOICAN_MODEL_STATE_CORRUPT = 3,
} NoicanModelState;

typedef struct NoicanHealth {
  bool running;
  bool faulted;
  bool worker_realtime;
  uint32_t monitor_device;
  uint64_t frames_processed;
  uint64_t output_underruns;
  uint64_t input_overruns;
  uint64_t worker_blocks;
  uint64_t worker_blocks_over_budget;
  uint64_t worker_block_max_ns;
} NoicanHealth;

typedef struct NoicanLevels {
  float input;
  float output;
  enum NoicanMonitorState monitor;
} NoicanLevels;

typedef struct NoicanModelStatus {
  enum NoicanModelState state;
  uint64_t received_bytes;
  uint64_t total_bytes;
} NoicanModelStatus;

typedef struct NoicanModelRatings {
  uint8_t noise_removal;
  uint8_t voice_quality;
  uint8_t responsiveness;
  uint8_t efficiency;
} NoicanModelRatings;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

void *noican_engine_create(const char *models_directory);

void noican_engine_destroy(void *handle);

int32_t noican_engine_start(void *handle,
                            uint32_t aggregate_device,
                            uint32_t virtual_output_first_channel,
                            uint32_t virtual_output_channel_count,
                            const char *model_id);

int32_t noican_engine_start_native(void *handle,
                                   uint32_t input_device,
                                   uint32_t output_device,
                                   double capture_sample_rate,
                                   const char *model_id);

void noican_engine_stop(void *handle);

int32_t noican_engine_set_model(void *handle, const char *model_id);

struct NoicanHealth noican_engine_health(const void *handle);

struct NoicanLevels noican_engine_levels(const void *handle);

struct NoicanModelStatus noican_engine_model_status(const void *handle, const char *model_id);

int32_t noican_engine_set_monitor(void *handle, int32_t enabled);

size_t noican_monitor_target_error(char *buffer, size_t capacity);

size_t noican_monitor_device_error(uint32_t device, char *buffer, size_t capacity);

size_t noican_engine_routing_description(const void *handle, char *buffer, size_t capacity);

void noican_engine_reset_debug_stats(void *handle);

int32_t noican_engine_set_intensity(void *handle, float intensity);

size_t noican_engine_last_error(const void *handle, char *buffer, size_t capacity);

size_t noican_model_count(void);

size_t noican_model_id(size_t index, char *buffer, size_t capacity);

size_t noican_model_display_name(size_t index, char *buffer, size_t capacity);

size_t noican_model_tagline(size_t index, char *buffer, size_t capacity);

size_t noican_model_details(size_t index, char *buffer, size_t capacity);

struct NoicanModelRatings noican_model_ratings(size_t index);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* NOICAN_H */
