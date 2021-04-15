#include <stdio.h>
#include <stdint.h>
#include <stdarg.h>

extern void alsa_log_backend(const char*, int, const char*, int, const char*);

void alsa_log_stub(const char* file, int line, const char* function,
                   int errno, const char* format, ...) {
  char buf[1024]; // if a message is longer than this... oh well
  va_list arg;
  va_start(arg, format);
  vsnprintf(buf, sizeof(buf), format, arg);
  va_end(arg);
  alsa_log_backend(file, line, function, errno, buf);
}
