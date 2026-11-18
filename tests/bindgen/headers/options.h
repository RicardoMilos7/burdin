/* The C front end's options reach libclang, so a conditional declaration follows them. */
#ifndef OPTIONS_H
#define OPTIONS_H

#ifdef WITH_EXTRA
int extra_function(int x);
#endif

#ifndef WITHOUT_PRESENT
int present(int x);
#endif

#endif
