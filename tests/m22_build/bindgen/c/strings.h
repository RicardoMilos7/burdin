#include <stddef.h>

#define ANSWER 42

struct Pair {
    int first;
    int second;
};

enum Mode {
    MODE_OFF,
    MODE_ON = 5
};

size_t strlen(const char *s);
