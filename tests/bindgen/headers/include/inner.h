#ifndef INNER_H
#define INNER_H

#define INNER_CONSTANT 99

typedef int inner_int;

struct Inner {
    int a;
    int b;
};

struct Hidden {
    char secret[32];
};

struct Opaque;

int inner_function(int x);

#endif
