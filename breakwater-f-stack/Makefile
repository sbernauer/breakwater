# Copied and modified from https://github.com/F-Stack/f-stack/blob/dev/example/Makefile
SERVER_SOURCES := parser.c framebuffer.c breakwater-f-stack.c

ifeq ($(FF_PATH),)
$(error "Please export the environment variable `FF_PATH` and point it to the f-stack root directory. This Makefile assumes you have run 'make' in the '<f-stack root>/lib' folder")
endif

ifneq ($(shell pkg-config --exists libdpdk && echo 0),0)
$(error "No installation of DPDK found, maybe you should export environment variable `PKG_CONFIG_PATH`")
endif

PKGCONF ?= pkg-config

CFLAGS += -O3 -g -gdwarf-2 $(shell $(PKGCONF) --cflags libdpdk)

LIBS+= $(shell $(PKGCONF) --static --libs libdpdk)
LIBS+= -I${FF_PATH}/lib -L${FF_PATH}/lib -Wl,--whole-archive,-lfstack,--no-whole-archive
LIBS+= -Wl,--no-whole-archive -lrt -lm -ldl -lcrypto -pthread -lnuma
# WIP, start with "sudo LD_LIBRARY_PATH=../target/release build/breakwater-f-stack"
LIBS += -l breakwater_parser_c_bindings -L ../target/release/

TARGET="breakwater-f-stack"
all:
	mkdir -p build/
	cc ${CFLAGS} -DINET6 -o build/${TARGET} $(SERVER_SOURCES) ${LIBS}
	cc ${CFLAGS} -o build/${TARGET}-v4-only $(SERVER_SOURCES) ${LIBS}

.PHONY: clean
clean:
	rm -rf build/
