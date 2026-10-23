// Scrawl's bundle compiler for apps (crates/scrawl-ffi). Add a recording's frames as RGBA pixels,
// optionally ask which keyframes it picks and add those again at full size, then compile the
// capture (scrawl.json's fields as JSON) into the bundle's files, all in memory.
//
// Strings are UTF-8 and NUL-terminated. What a function returns is freed with its _free
// function; what a bundle's getters return is borrowed and lives as long as the bundle. On an
// error a function returns NULL (or false) and sets *error to a message for scrawl_string_free.

#ifndef SCRAWL_H
#define SCRAWL_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ScrawlCompiler ScrawlCompiler;
typedef struct ScrawlBundle ScrawlBundle;

// A new, empty compiler; free it with scrawl_compiler_free.
ScrawlCompiler *scrawl_compiler_new(void);
void scrawl_compiler_free(ScrawlCompiler *compiler);

// Adds the screen as it looked t_ms into the capture: len bytes of width x height RGBA, copied.
// Candidates can be small (480 px) when the keyframes are added again at full size.
bool scrawl_compiler_add_frame(ScrawlCompiler *compiler, uint64_t t_ms, uint32_t width,
                               uint32_t height, const uint8_t *rgba, size_t len);
// Adds a keyframe at full size, at one of scrawl_compiler_keyframe_times.
bool scrawl_compiler_add_full_frame(ScrawlCompiler *compiler, uint64_t t_ms, uint32_t width,
                                    uint32_t height, const uint8_t *rgba, size_t len);

// The times (ms) of the keyframes scrawl_compiler_compile will pick from the frames added so far;
// their count goes to *count. Free with scrawl_times_free.
uint64_t *scrawl_compiler_keyframe_times(const ScrawlCompiler *compiler, const char *capture_json,
                                         size_t *count, char **error);
void scrawl_times_free(uint64_t *times, size_t count);

// Builds the bundle from the frames added so far (they are used up). Free with scrawl_bundle_free.
ScrawlBundle *scrawl_compiler_compile(ScrawlCompiler *compiler, const char *capture_json,
                                      char **error);
void scrawl_bundle_free(ScrawlBundle *bundle);

// The bundle's files in the order to save them (summary.md last): a path inside the bundle
// (frames/frame-01.png) and its bytes. NULL out of range.
size_t scrawl_bundle_file_count(const ScrawlBundle *bundle);
const char *scrawl_bundle_file_name(const ScrawlBundle *bundle, size_t index);
const uint8_t *scrawl_bundle_file_data(const ScrawlBundle *bundle, size_t index, size_t *len);
// summary.md's text.
const char *scrawl_bundle_summary(const ScrawlBundle *bundle);

// A ScrawlKit recording's capture JSON, from its timeline JSON, the movie's size in pixels and
// length: touches become taps and swipes, ink moves to the bundle's coordinates. fields_json is
// NULL or a JSON object of capture fields that replace the timeline's (id, createdAtMs,
// transcript, video, audio, ...). Free with scrawl_string_free.
char *scrawl_capture_from_timeline(const char *timeline_json, uint32_t width, uint32_t height,
                                   uint64_t duration_ms, const char *fields_json, char **error);

// Frees an error message or a string from scrawl_capture_from_timeline.
void scrawl_string_free(char *message);

#ifdef __cplusplus
}
#endif

#endif
