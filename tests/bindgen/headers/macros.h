/* Object-like macros are constants where they are constant expressions. */
#include <stdint.h>

#define EMPTY
#define COUNT 10
#define COUNT_TWICE (COUNT * 2)
#define HEX 0xff
#define OCTAL 017
#define BINARY 0b101
#define NEGATIVE (-7)
#define UNSIGNED 4000000000u
#define WIDE 5000000000
#define SHIFTED (1u << 31)
#define FLAG_A (1 << 0)
#define FLAG_B (1 << 1)
#define FLAGS (FLAG_A | FLAG_B)
#define CHARACTER 'A'
#define RATIO 1.5
#define HALF (RATIO / 2)
#define GREETING "hello" " " "world"
#define CAST ((uint8_t)300)
#define BOTH (COUNT > 5 ? 1 : 2)

enum { ENUM_ONE = 1, ENUM_TWO };
#define FROM_ENUM (ENUM_ONE + ENUM_TWO)

#define SQUARE(x) ((x) * (x))
#define NOT_A_CONSTANT sizeof(int)
#define CALL getpid()
#define UNKNOWN_NAME some_global
