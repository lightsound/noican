#ifndef NOICAN_H
#define NOICAN_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void *noican_engine_create(const char *model_directory);
void noican_engine_destroy(void *handle);
int32_t noican_engine_start(void *handle, uint32_t aggregate_device, const char *model_slug);
void noican_engine_stop(void *handle);
int32_t noican_engine_set_model(void *handle, const char *model_slug);
int32_t noican_engine_is_running(const void *handle);
int32_t noican_engine_is_faulted(const void *handle);
size_t noican_engine_last_error(const void *handle, char *buffer, size_t capacity);

size_t noican_model_count(void);
size_t noican_model_slug(size_t index, char *buffer, size_t capacity);

#ifdef __cplusplus
}
#endif

#endif
