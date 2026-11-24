/* A C program that links the `greet` library (tests/check_library.py). The header is the one `c-header` writes. */
#include <stdio.h>

#include "greet.h"

int main(void) {
    int base = greet_base();
    printf("base %d\n", base);
    greet_print(5);
    return base == 42 ? 0 : 1;
}
