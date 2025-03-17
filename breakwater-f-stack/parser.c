// Port of breakwater-parser/src/original.rs to C
// Yes, I know this is ugly
// Yes, I only care about performance :)
// PRs welcome to improve the situation without compromising on the performance!

#include "parser.h"

size_t parse(const unsigned char *buffer, size_t length) {
    return length;
}