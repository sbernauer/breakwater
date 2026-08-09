#ifndef _FRAMEBUFFER_H_
#define _FRAMEBUFFER_H_

struct framebuffer {
    uint16_t width;
    uint16_t height;

    uint32_t* pixels;
};

int create_fb(struct framebuffer** framebuffer, uint16_t width, uint16_t height, char* shared_memory_name);
void fb_set(struct framebuffer* framebuffer, uint16_t x, uint16_t y, uint32_t rgba);
uint32_t fb_get(struct framebuffer* framebuffer, uint16_t x, uint16_t y);

#endif
