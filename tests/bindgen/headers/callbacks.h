/* Function pointers are declared as CPtr<CVoid>; a variadic function keeps its `...`. */
#include <stddef.h>

typedef void (*visit_fn)(int value, void *context);
typedef int (*compare_fn)(const void *left, const void *right);

struct Visitor {
    visit_fn visit;
    void *context;
    int calls;
};

void walk(const int *values, size_t count, visit_fn visit, void *context);
void sort(void *items, size_t count, size_t size, compare_fn compare);
int printf_like(const char *format, ...);
void (*signal_like(int number, void (*handler)(int)))(int);
