FROM rust:1.86.0-bookworm AS builder

RUN apt-get update && \
    apt-get install -y clang libvncserver-dev && \
    rm -rf /var/lib/apt/lists/*

# Installing it explicitly to make better use of the docker cache
RUN rustup toolchain install nightly

WORKDIR /breakwater
COPY breakwater-parser/ breakwater-parser/
COPY breakwater-egui-overlay/ breakwater-egui-overlay/
COPY breakwater/ breakwater/
COPY Cargo.toml .
COPY Cargo.lock .
COPY rust-toolchain.toml .
COPY Arial.ttf .

# We don't want to e.g. set "-C target-cpu=native", so that the binary should run everywhere
# Also we can always build with vnc server support as the docker image contains all needed dependencies in any case
# While the "native-display" feature compiles successfully, we'd rather not offer the CLI option, as it might cause
# users to think it should work (which it doesn't). So let's not enable that feature
RUN RUSTFLAGS='' cargo build --release --no-default-features --features vnc,binary-set-pixel

FROM debian:bookworm-slim AS final
RUN apt-get update && \
    apt-get install -y libvncserver1 ffmpeg && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /breakwater/target/release/breakwater /usr/local/bin/breakwater

ENTRYPOINT ["breakwater"]
