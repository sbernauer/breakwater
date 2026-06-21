// Port of breakwater-parser/src/original.rs to C
// Yes, I know this is ugly
// Yes, I only care about performance :)
// PRs welcome to improve the situation without compromising on the performance!

#include <stdio.h>
#include <string.h>
#include <errno.h>

#include "ff_api.h"

#include "breakwater-f-stack.h"
#include "framebuffer.h"
#include "parser.h"

#define HELP_TEXT "Pixelflut server, see https://github.com/sbernauer/breakwater/ and https://wiki.cccgoe.de/wiki/Pixelflut\n"

// Fast ASCII to int (no error checking)
static inline uint16_t fast_atoi(const char **p) {
    int v = 0;
    while (**p >= '0' && **p <= '9') {
        v = v * 10 + (**p - '0');
        (*p)++;
    }
    return v;
}

// Fast hex to int (handles rrggbb or rrggbbaa)
static inline uint32_t fast_hex(const char *p, int len) {
    uint32_t v = 0;
    for (int i = 0; i < len; i++) {
        v <<= 4;
        char c = *p++;
        if (c >= '0' && c <= '9') v |= (c - '0');
        else if (c >= 'a' && c <= 'f') v |= (c - 'a' + 10);
        else if (c >= 'A' && c <= 'F') v |= (c - 'A' + 10);
        else break;
    }
    return v;
}

size_t parse(const char *buffer, size_t length, struct framebuffer* framebuffer, int clientfd) {
    const char *p = buffer;
    const char *end = p + length;

    while (p < end) {
        if (memcmp(p, "PX ", 3) == 0) {
            p += 3;
            int x = fast_atoi(&p);
            if (*p != ' ') {
                p += 1;
                continue;
            }
            p += 1;
            int y = fast_atoi(&p);

            // Request out of screen bounds
            if (x >= WIDTH || y >= HEIGHT) {
                continue;
            }

            // Command to set pixel
            if (*p == ' ') {
                p += 1;
                uint32_t rgb = fast_hex(p, 6);
                p += 6;

                rgb =
                    // Green
                    rgb & 0x0000ff00
                    // Red
                    | ((rgb >> 16) & 0x000000ff)
                    // Blue
                    | ((rgb << 16) & 0x00ff0000);

                fb_set(framebuffer, x, y, rgb);
                continue;
            }

            // Command to read pixel
            else if (*p == '\n') {
                p += 1;
                uint32_t rgb = fb_get(framebuffer, x, y);

                char out[32];
                int len = snprintf(out, sizeof(out), "PX %d %d %06x\n", x, y, rgb);
                ff_write(clientfd, out, len);
                continue;
            }

            else {
                // The parsing already moved p
                continue;
            }
        }

        else if (memcmp(p, "SIZE", 4) == 0) {
            char out[32];
            int len = snprintf(out, sizeof(out), "SIZE %d %d\n", WIDTH, HEIGHT);
            ff_write(clientfd, out, len);

            p += 4;
            continue;
        }

        else if (memcmp(p, "HELP", 4) == 0) {
            ff_write(clientfd, HELP_TEXT, sizeof(HELP_TEXT) - 1);

            p += 4;
            continue;
        }

        p++;
    }

    return length;
}
