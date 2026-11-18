/* A static inline function has no symbol, so bindgen wraps it in C. */
#include <stddef.h>

static inline int add(int a, int b) { return a + b; }
static inline double scale(double x, const double *factor) { return x * *factor; }
static inline void bump(int *counter) { ++*counter; }
static inline size_t length(const char *text) {
    size_t n = 0;
    while (text[n]) ++n;
    return n;
}
static inline int variadic_sum(int count, ...) { return count; }
static int internal_helper(int x);
int external(int x);
