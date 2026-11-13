/* The Burdin stage 0 runtime: process start-up and output.
 *
 * The driver compiles this file together with the program's object file. Output is unbuffered for now, so there
 * is nothing to flush when a panic aborts. */

#include <stddef.h>
#include <stdint.h>

#ifdef _WIN32
#include <fcntl.h>
#include <io.h>
#define BURDIN_WRITE _write
typedef int burdin_write_len;
#else
#include <unistd.h>
#define BURDIN_WRITE write
typedef size_t burdin_write_len;
#endif

/* Defined by the compiler: calls the program's `main`. */
void burdin_entry(void);

static void write_all(int fd, const uint8_t *ptr, size_t len) {
    while (len > 0) {
        size_t chunk = len > 0x40000000u ? 0x40000000u : len;
        long written = (long)BURDIN_WRITE(fd, ptr, (burdin_write_len)chunk);
        if (written <= 0) {
            return;
        }
        ptr += written;
        len -= (size_t)written;
    }
}

void burdin_rt_print(int32_t fd, const uint8_t *ptr, size_t len, int32_t newline) {
    write_all(fd, ptr, len);
    if (newline) {
        write_all(fd, (const uint8_t *)"\n", 1);
    }
}

int main(int argc, char **argv) {
    (void)argc;
    (void)argv;
#ifdef _WIN32
    /* Output is binary-safe: no newline translation (tests/README.md, assumption 10). */
    _setmode(0, _O_BINARY);
    _setmode(1, _O_BINARY);
    _setmode(2, _O_BINARY);
#endif
    burdin_entry();
    return 0;
}
