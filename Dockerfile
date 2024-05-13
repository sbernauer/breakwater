FROM rust:1.78.0-bookworm as builder

WORKDIR /breakwater
COPY breakwater-core/ breakwater-core/
COPY breakwater-parser/ breakwater-parser/
COPY breakwater/ breakwater/
COPY Cargo.toml .
COPY Cargo.lock .
COPY rust-toolchain.toml .
COPY Arial.ttf .

RUN apt-get update && \
    apt-get install -y clang libvncserver-dev && \
    rm -rf /var/lib/apt/lists/*

# We don't want to e.g. set "-C target-cpu=native", so that the binary should run everywhere
# Also we can always build with vnc server support as the docker image contains all needed dependencies in any case
RUN RUSTFLAGS='' cargo build --release --features vnc


FROM debian:bookworm-slim as final
RUN apt-get update && \
    apt-get install -y libvncserver1 ffmpeg && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /breakwater/target/release/breakwater /usr/local/bin/breakwater

ENTRYPOINT ["breakwater"]
