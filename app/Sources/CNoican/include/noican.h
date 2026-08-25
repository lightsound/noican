// C ABI of the noican engine (crates/noican-ffi).
// Keep in sync with crates/noican-ffi/src/lib.rs.
#ifndef NOICAN_H
#define NOICAN_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct NoicanHandle NoicanHandle;

// Creates an engine handle. `models_dir` is the directory holding
// downloaded model weights. Returns NULL on invalid input.
NoicanHandle *noican_new(const char *models_dir);

// Destroys the handle, stopping the engine if running.
void noican_free(NoicanHandle *handle);

// Last error message for this handle (caller frees with
// noican_string_free) or NULL.
char *noican_last_error(NoicanHandle *handle);

// Frees any string returned by this library.
void noican_string_free(char *s);

// JSON: [{"uid":"...","name":"..."}] — selectable input devices.
char *noican_list_input_devices(void);

// JSON: [{"id":"...","name":"...","fetched":bool,"needsEnrollment":bool}]
char *noican_list_models(NoicanHandle *handle);

// Starts: input (NULL = system default) -> model -> first output device
// whose name starts with "BlackHole". `enroll_wav` may be NULL unless the
// model needs enrollment. Returns 0 on success.
int32_t noican_start(NoicanHandle *handle, const char *input_uid,
                     const char *model_id, const char *enroll_wav);

// Stops the engine (no-op when not running).
void noican_stop(NoicanHandle *handle);

// Crossfaded model switch while running. Returns 0 on success.
int32_t noican_set_model(NoicanHandle *handle, const char *model_id,
                         const char *enroll_wav);

// JSON: {"running":bool,"model":"id","blocks":n,"underruns":n,
//        "overruns":n,"stageFailed":bool}
char *noican_status_json(NoicanHandle *handle);

#ifdef __cplusplus
}
#endif

#endif // NOICAN_H
