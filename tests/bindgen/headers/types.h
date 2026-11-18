/* Functions, typedefs, structs, enumerations, and constants with fixed-size types. */
#include <stddef.h>
#include <stdint.h>

#define SHAPE_MAX 16
#define SHAPE_NAME "shape \"lib\" {v1}"
#define SHAPE_SCALE 2.5

typedef uint32_t shape_id;
typedef size_t shape_count;
typedef struct Shape Shape;
typedef const char *shape_label;

enum ShapeKind {
    SHAPE_POINT,
    SHAPE_LINE = 5,
    SHAPE_POLY
};

typedef enum {
    COLOR_RED,
    COLOR_GREEN = -1,
    COLOR_BLUE = 7
} Color;

typedef struct {
    double x;
    double y;
} Vec2;

struct Shape {
    shape_id id;
    enum ShapeKind kind;
    Vec2 points[4];
    Shape *next;
    const char *name;
    uint8_t flags;
    int type;
};

struct Tagged {
    int8_t a;
    int64_t b;
    int16_t c;
};

struct Rect {
    struct {
        int x;
        int y;
    } origin;
    int width;
    int height;
};

struct List;
struct Pair { int first; int second; };

shape_id shape_new(enum ShapeKind kind, Color color);
void shape_free(Shape *shape);
shape_count shape_points(const Shape *shape, Vec2 *out, shape_count capacity);
int shape_name(const Shape *shape, char *buffer, size_t length);
const char *shape_kind_name(enum ShapeKind kind);
struct List *list_new(void);
int pair_sum(const struct Pair *pair);
void sum_all(const int values[], int count);
void unnamed(int, const char *);
int in(int type);
