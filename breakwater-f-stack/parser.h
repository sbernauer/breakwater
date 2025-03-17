#ifndef _PARSER_H_
#define _PARSER_H_

#include <stddef.h>

// Longest possible command
#define PARSER_LOOKAHEAD (sizeof("PX 1234 1234 rrggbbaa\n") - 1) // Excludes null terminator

// Returns the last byte parsed. The next parsing loop will again contain all data that was not parsed.
size_t parse(const unsigned char *buffer, size_t length);

#endif
