/* A C `__int128` has no Burdin equivalent, so a function that uses one is skipped with a note, and the others are
   still generated (spec/int128.md, I128-13). */

__int128 wide_value(__int128 x);
int small_value(int x);
