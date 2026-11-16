#define ANSWER 42

typedef unsigned long size_t;

struct Pair {
    int first;
    int second;
};

enum Mode {
    MODE_OFF,
    MODE_ON = 5
};

size_t strlen(const char *s);
