// Builds a bundle through the C interface, as ScrawlKit's Swift does.
//   cargo build -p scrawl-ffi && crates/scrawl-ffi/tests/smoke.sh
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "scrawl.h"

static const char *capture =
    "{\"id\":\"ffi-test\",\"createdAtMs\":1700000000000,\"source\":\"ios\","
    "\"target\":{\"mode\":\"window\",\"app\":\"Example\",\"displayId\":0,"
    "\"frame\":{\"x\":0,\"y\":0,\"width\":64,\"height\":40,\"scale\":1}},"
    "\"durationMs\":2000,\"events\":[{\"kind\":\"click\",\"t\":900,\"x\":0.5,\"y\":0.5}],"
    "\"transcript\":{\"source\":\"test\",\"text\":\"this button\","
    "\"segments\":[{\"t0\":800,\"t1\":1500,\"text\":\"this button\"}]}}";

static void add(ScrawlCompiler *c, uint64_t t, uint32_t w, uint32_t h, int full) {
  size_t len = (size_t)w * h * 4;
  uint8_t *rgba = malloc(len);
  memset(rgba, (int)(t / 10), len);
  assert(full ? scrawl_compiler_add_full_frame(c, t, w, h, rgba, len)
              : scrawl_compiler_add_frame(c, t, w, h, rgba, len));
  free(rgba);
}

int main(void) {
  ScrawlCompiler *c = scrawl_compiler_new();
  for (uint64_t t = 0; t <= 1800; t += 900) add(c, t, 64, 40, 0);

  char *error = NULL;
  size_t count = 0;
  uint64_t *times = scrawl_compiler_keyframe_times(c, capture, &count, &error);
  assert(times && !error && count > 0);
  for (size_t i = 0; i < count; i++) add(c, times[i], 128, 80, 1);
  scrawl_times_free(times, count);

  ScrawlBundle *b = scrawl_compiler_compile(c, capture, &error);
  assert(b && !error);
  size_t files = scrawl_bundle_file_count(b);
  assert(files >= 4);
  assert(strcmp(scrawl_bundle_file_name(b, 0), "contact-sheet.png") == 0);
  assert(strcmp(scrawl_bundle_file_name(b, files - 1), "summary.md") == 0);
  assert(scrawl_bundle_file_name(b, files) == NULL);
  for (size_t i = 0; i < files; i++) {
    size_t len = 0;
    const uint8_t *data = scrawl_bundle_file_data(b, i, &len);
    assert(data && len > 0);
    const char *name = scrawl_bundle_file_name(b, i);
    if (strncmp(name, "frames/", 7) == 0) {
      // A PNG drawn from its full frame: IHDR width 128.
      assert(memcmp(data, "\x89PNG", 4) == 0);
      uint32_t width = (uint32_t)data[16] << 24 | data[17] << 16 | data[18] << 8 | data[19];
      assert(width == 128);
    }
  }
  assert(strstr(scrawl_bundle_summary(b), "this button"));
  printf("ok: %zu files, %zu keyframes\n", files, count);
  scrawl_bundle_free(b);

  // Errors come back as NULL and a message, never a crash.
  ScrawlBundle *none = scrawl_compiler_compile(c, "{}", &error);
  assert(!none && error && strstr(error, "valid JSON"));
  scrawl_string_free(error);
  error = NULL;
  none = scrawl_compiler_compile(c, capture, &error);
  assert(!none && error && strstr(error, "at least one frame"));
  scrawl_string_free(error);
  assert(!scrawl_compiler_add_frame(c, 0, 1, 1, NULL, 4));
  scrawl_compiler_free(c);

  // A ScrawlKit timeline becomes a capture that compiles.
  error = NULL;
  const char *timeline =
      "{\"app\":\"UpDown\",\"screen\":{\"width\":400,\"height\":800,\"scale\":3},"
      "\"markers\":[0.5],\"strokes\":[{\"t0\":0.2,\"t1\":0.4,"
      "\"points\":[[100,200,0.2],[300,600,0.4]]}]}";
  char *kit = scrawl_capture_from_timeline(timeline, 40, 80, 1000, "{\"id\":\"kit-test\"}", &error);
  assert(kit && !error && strstr(kit, "\"source\":\"iphone\""));
  c = scrawl_compiler_new();
  add(c, 0, 40, 80, 0);
  b = scrawl_compiler_compile(c, kit, &error);
  assert(b && !error && strstr(scrawl_bundle_summary(b), "kit-test.scrawl"));
  scrawl_bundle_free(b);
  scrawl_compiler_free(c);
  scrawl_string_free(kit);
  assert(!scrawl_capture_from_timeline("[", 40, 80, 1000, NULL, &error) && error);
  scrawl_string_free(error);
  scrawl_compiler_free(NULL);
  scrawl_bundle_free(NULL);
  return 0;
}
