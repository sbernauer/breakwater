FROM rust:1.70.0-bookworm as builder

WORKDIR /breakwater
COPY src/ src/
COPY Cargo.toml .
COPY Arial.ttf .

RUN apt-get update && \
    apt-get install -y clang libvncserver-dev && \
    rm -rf /var/lib/apt/lists/*
RUN rustup toolchain install nightly
# We don't want to e.g. set "-C target-cpu=native", so that the binary should run everywhere
RUN RUSTFLAGS='' cargo +nightly install --path .

FROM debian:bookworm-slim
RUN apt-get update && \
    apt-get install -y libvncserver1 ffmpeg && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/breakwater /usr/local/bin/breakwater

ENTRYPOINT ["breakwater"]
