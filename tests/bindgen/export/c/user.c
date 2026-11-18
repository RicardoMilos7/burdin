/* Calls the Burdin exports through the generated header and reports what it saw. */
#include <stdio.h>
#include <string.h>

#include "exports.h"

int run_c(void) {
    struct Point p = { 3, 4 };
    struct Segment s;
    uint8_t buffer[4] = { 0, 0, 0, 0 };

    memset(&s, 0, sizeof s);
    s.from.x = 1;
    s.to.x = 10;
    strcpy(s.label, "abc");

    printf("add_one(41) = %d\n", add_one(41));
    printf("scale(2.0, LIB_RATIO) = %.1f\n", scale(2.0, LIB_RATIO));
    printf("point_sum = %d\n", point_sum(&p));
    printf("segment_dx = %d\n", segment_dx(&s));
    fill(buffer, 4, 7);
    printf("fill = %d%d%d%d\n", buffer[0], buffer[1], buffer[2], buffer[3]);
    char name[] = "burdin";
    printf("name_length = %zu\n", name_length(name));
    printf("version = %d, offset = %d, name = %s", LIB_VERSION, LIB_OFFSET, LIB_NAME);
    printf("big = %llu\n", (unsigned long long)LIB_BIG);
    printf("sizes = %zu %zu\n", sizeof(Point), sizeof(Segment));
    nothing();
    fflush(stdout);
    return 0;
}
