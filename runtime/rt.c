/* The Burdin stage 0 runtime: process start-up, output, strings, and the panic path.
 *
 * The driver compiles this file together with the program's object file. Output is unbuffered for now, so there
 * is nothing to flush when a panic aborts. */

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

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

/* The layout of a Burdin `String`. The compiler uses the same layout: { ptr, usize, usize }. */
typedef struct {
    uint8_t *ptr;
    size_t len;
    size_t cap;
} BurdinString;

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

static void write_cstr(int fd, const char *s) {
    write_all(fd, (const uint8_t *)s, strlen(s));
}

void burdin_rt_print(int32_t fd, const uint8_t *ptr, size_t len, int32_t newline) {
    write_all(fd, ptr, len);
    if (newline) {
        write_all(fd, (const uint8_t *)"\n", 1);
    }
}

static void die(void) {
#ifdef _WIN32
    /* No "abnormal program termination" message box or text. */
    _set_abort_behavior(0, _WRITE_ABORT_MSG | _CALL_REPORTFAULT);
#endif
    abort();
}

/* Prints `path:LINE:COL: panic: message` to stderr and aborts. */
static void panic_start(const uint8_t *file, size_t file_len, uint32_t line, uint32_t col) {
    char buf[64];
    write_all(2, file, file_len);
    snprintf(buf, sizeof buf, ":%u:%u: panic: ", (unsigned)line, (unsigned)col);
    write_cstr(2, buf);
}

void burdin_rt_panic(const uint8_t *msg, size_t msg_len, const uint8_t *file, size_t file_len, uint32_t line,
                     uint32_t col) {
    panic_start(file, file_len, line, col);
    write_all(2, msg, msg_len);
    write_cstr(2, "\n");
    die();
}

void burdin_rt_panic_bounds(uint64_t index, uint64_t len, const uint8_t *file, size_t file_len, uint32_t line,
                            uint32_t col) {
    char buf[128];
    panic_start(file, file_len, line, col);
    snprintf(buf, sizeof buf, "index out of bounds: the index is %llu but the length is %llu\n",
             (unsigned long long)index, (unsigned long long)len);
    write_cstr(2, buf);
    die();
}

static void alloc_string(BurdinString *out, const uint8_t *bytes, size_t len) {
    out->ptr = NULL;
    out->len = len;
    out->cap = len;
    if (len == 0) {
        return;
    }
    out->ptr = (uint8_t *)malloc(len);
    if (out->ptr == NULL) {
        write_cstr(2, "panic: out of memory\n");
        die();
    }
    memcpy(out->ptr, bytes, len);
}

void burdin_rt_string_free(uint8_t *ptr, size_t cap) {
    if (cap != 0) {
        free(ptr);
    }
}

/* The allocation behind a `Heap<T>`. */
void *burdin_rt_alloc(size_t size) {
    void *ptr = malloc(size == 0 ? 1 : size);
    if (ptr == NULL) {
        write_cstr(2, "panic: out of memory\n");
        die();
    }
    return ptr;
}

void burdin_rt_free(void *ptr) {
    free(ptr);
}

void burdin_rt_string_new(BurdinString *out) {
    out->ptr = NULL;
    out->len = 0;
    out->cap = 0;
}

void burdin_rt_string_clone(BurdinString *dst, const BurdinString *src) {
    alloc_string(dst, src->ptr, src->len);
}

/* Appends `len` bytes, growing the buffer when needed. */
void burdin_rt_string_push_str(BurdinString *s, const uint8_t *ptr, size_t len) {
    if (len == 0) {
        return;
    }
    size_t need = s->len + len;
    if (need < s->len) {
        write_cstr(2, "panic: out of memory\n");
        die();
    }
    if (need > s->cap) {
        size_t cap = s->cap + s->cap / 2 + 8;
        if (cap < need) {
            cap = need;
        }
        uint8_t *grown = (uint8_t *)realloc(s->ptr, cap);
        if (grown == NULL) {
            write_cstr(2, "panic: out of memory\n");
            die();
        }
        s->ptr = grown;
        s->cap = cap;
    }
    memcpy(s->ptr + s->len, ptr, len);
    s->len = need;
}

void burdin_rt_str_to_string(const uint8_t *ptr, size_t len, BurdinString *out) {
    alloc_string(out, ptr, len);
}

void burdin_rt_u64_to_string(uint64_t v, BurdinString *out) {
    uint8_t buf[24];
    size_t i = sizeof buf;
    do {
        buf[--i] = (uint8_t)('0' + v % 10);
        v /= 10;
    } while (v != 0);
    alloc_string(out, buf + i, sizeof buf - i);
}

void burdin_rt_i64_to_string(int64_t v, BurdinString *out) {
    uint8_t buf[24];
    size_t i = sizeof buf;
    /* Negate in unsigned arithmetic so that INT64_MIN works. */
    uint64_t u = v < 0 ? (uint64_t)0 - (uint64_t)v : (uint64_t)v;
    do {
        buf[--i] = (uint8_t)('0' + u % 10);
        u /= 10;
    } while (u != 0);
    if (v < 0) {
        buf[--i] = '-';
    }
    alloc_string(out, buf + i, sizeof buf - i);
}

/* UTF-8 encoding of a Unicode scalar value. */
void burdin_rt_char_to_string(uint32_t c, BurdinString *out) {
    uint8_t buf[4];
    size_t n;
    if (c < 0x80) {
        buf[0] = (uint8_t)c;
        n = 1;
    } else if (c < 0x800) {
        buf[0] = (uint8_t)(0xC0 | (c >> 6));
        buf[1] = (uint8_t)(0x80 | (c & 0x3F));
        n = 2;
    } else if (c < 0x10000) {
        buf[0] = (uint8_t)(0xE0 | (c >> 12));
        buf[1] = (uint8_t)(0x80 | ((c >> 6) & 0x3F));
        buf[2] = (uint8_t)(0x80 | (c & 0x3F));
        n = 3;
    } else {
        buf[0] = (uint8_t)(0xF0 | (c >> 18));
        buf[1] = (uint8_t)(0x80 | ((c >> 12) & 0x3F));
        buf[2] = (uint8_t)(0x80 | ((c >> 6) & 0x3F));
        buf[3] = (uint8_t)(0x80 | (c & 0x3F));
        n = 4;
    }
    alloc_string(out, buf, n);
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
