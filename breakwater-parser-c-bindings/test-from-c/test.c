#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// For docs see the Rust docs in `breakwater-parser-c-bindings/src/lib.rs`
extern void breakwater_init_original_parser(int width, int height, char shared_memory_name[]);
extern size_t breakwater_original_parser_parser_lookahead();
extern size_t breakwater_original_parser_parse(
    const char* buffer,
    size_t buffer_len,
    unsigned char** out_response_ptr,
    size_t* out_response_len
);

int main(void) {
    breakwater_init_original_parser(1920, 1080, "breakwater-test");
    size_t parser_lookahead = breakwater_original_parser_parser_lookahead();
    printf("Parser lookahead: %ld\n", parser_lookahead);

    const char* text = 
    "HELP\n"
    "PX 0 0 123456\n"
    "PX 0 1 111111\n"
    "PX 0 2 222222\n"
    "PX 0 3 333333\n"
    "PX 0 4 444444\n"
    "PX 0 5 555555\n"
    "PX 0 6 666666\n"
    "PX 0 7 777777\n"
    "PX 0 8 888888\n"
    "PX 0 9 999999\n"
    "PX 0 0\n"
    "PX 0 1\n"
    "PX 0 2\n"
    "PX 0 3\n"
    "PX 0 4\n"
    "PX 0 5\n"
    "PX 0 6\n"
    "PX 0 7\n";

    size_t text_len = strlen(text);
    size_t buffer_len = text_len + parser_lookahead;
    unsigned char* buffer = malloc(buffer_len);
    if (!buffer) {
        perror("malloc failed");
        return 1;
    }
    memcpy(buffer, text, text_len);

    unsigned char* response = NULL;
    size_t response_len = 0;

    long parsed = breakwater_original_parser_parse(buffer, buffer_len, &response, &response_len);
    printf("Parse bytes: %ld\n", parsed);

    if (response && response_len > 0) {
        printf("Response content: %.*s\n", (int)response_len, response);
        free(response);
    }

    free(buffer);
    return 0;
}
