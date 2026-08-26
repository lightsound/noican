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
void noican_engine_stop(void *handle);
int32_t noican_engine_set_model(void *handle, const char *model_id);
int32_t noican_engine_is_running(const void *handle);
int32_t noican_engine_is_faulted(const void *handle);
size_t noican_monitor_target_error(char *buffer, size_t capacity);
size_t noican_monitor_device_error(uint32_t device, char *buffer, size_t capacity);
int32_t noican_engine_set_monitor(void *handle, int32_t enabled);
uint32_t noican_engine_monitor_device(const void *handle);
int32_t noican_engine_is_monitoring(const void *handle);
int32_t noican_engine_monitor_tripped(const void *handle);
float noican_engine_input_level(const void *handle);
float noican_engine_output_level(const void *handle);
uint64_t noican_engine_frames_processed(const void *handle);
size_t noican_engine_last_error(const void *handle, char *buffer, size_t capacity);

size_t noican_model_count(void);
size_t noican_model_id(size_t index, char *buffer, size_t capacity);
size_t noican_model_display_name(size_t index, char *buffer, size_t capacity);
int32_t noican_model_needs_enrollment(size_t index);

#ifdef __cplusplus
}
#endif

#endif
