#include <stdio.h>
#include <stdint.h>

extern void ffmpeg_log_backend(uintptr_t, int, const char*);

void ffmpeg_log_stub(void* ptr, int level,
                     const char* format, va_list arg) {
  char buf[1024]; // if a message is longer than this... oh well
  vsnprintf(buf, sizeof(buf), format, arg);
  ffmpeg_log_backend((uintptr_t)ptr, level, buf);
}
