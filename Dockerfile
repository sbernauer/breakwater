FROM rust:1.70.0 as builder

WORKDIR /breakwater
COPY src/ src/
COPY Cargo.toml .
COPY Arial.ttf .

RUN apt-get update && \
    apt-get install -y clang libvncserver-dev && \
    rm -rf /var/lib/apt/lists/*
RUN cargo install --path .

FROM debian:bullseye-slim
RUN apt-get update && \
    apt-get install -y libvncserver1 && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/breakwater /usr/local/bin/breakwater

ENTRYPOINT ["breakwater"]
