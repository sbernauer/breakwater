#ifndef _PARSER_H_
#define _PARSER_H_

#include <stddef.h>

#include "framebuffer.h"

// Longest possible command
#define PARSER_LOOKAHEAD (sizeof("PX 1234 1234 rrggbbaa\n") - 1) // Excludes null terminator

typedef struct {
    size_t leftover_bytes;
    char leftover[PARSER_LOOKAHEAD];
    long long bytes_parsed;
} client_state;

// Returns the last byte parsed. The next parsing loop will again contain all data that was not parsed.
size_t parse(const char *buffer, size_t length, struct framebuffer* framebuffer, int socket);

#endif
