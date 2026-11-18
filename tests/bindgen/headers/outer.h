/* Only the declarations of this file are generated; what it takes from an included header is declared as far as
   this file needs it. */
#include <stdint.h>
#include "inner.h"

#define OUTER_CONSTANT (INNER_CONSTANT + 1)

struct Outer {
    struct Inner inner;
    struct Hidden *hidden;
    inner_int count;
    uint16_t small;
};

int outer_function(struct Outer *outer, struct Opaque *opaque, inner_int x);
