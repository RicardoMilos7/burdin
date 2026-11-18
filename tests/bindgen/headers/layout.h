/* Unions, and the structs whose layout Burdin cannot spell field by field. */
#include <stdint.h>

union Number {
    int32_t i;
    double d;
    uint8_t bytes[12];
};

struct Packed {
    char tag;
    int32_t value;
} __attribute__((packed));

struct Flags {
    unsigned ready : 1;
    unsigned mode : 3;
    unsigned count : 12;
};

struct Header {
    int32_t length;
    uint8_t data[];
};

struct Wide {
    char c;
    _Alignas(16) char wide;
};

struct Anonymous {
    int tag;
    union {
        int32_t i;
        float f;
    };
};

struct Plain {
    char a;
    int32_t b;
    char c;
};

struct Holder {
    union Number number;
    struct Plain plain;
    union Number *pointer;
};

void use_number(union Number *n);
void use_packed(struct Packed *p, struct Flags *f, struct Header *h, struct Wide *w, struct Anonymous *a);
