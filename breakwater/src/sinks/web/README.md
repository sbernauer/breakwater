# Choosing the frame compressor

The web sink sends every frame to every viewer as zlib-compressed data, because
[`DecompressionStream`] is the only decompressor a browser gives us for free - and it only speaks
`gzip`, `deflate` and `deflate-raw`. Brotli and zstd exist in browsers, but only as HTTP
content-encodings, not as an API we can call. So the *format* is fixed at deflate.

The *encoder* is not. This page records what the candidates actually cost, because compressing
frames is by far the most expensive thing this sink does - roughly 90% of its CPU. (The other 10%
is copying the framebuffer and pushing the bytes into the websockets. Compression happens once for
all viewers, only the sending scales with the number of viewers.)

## Results

Each cell is **server CPU usage** followed by **network traffic per viewer**. Higher compression
levels move traffic down and CPU up; the interesting question is which encoder gives the best
traffic for the CPU spent.

| Implementation | level 1 | level 2 | level 3 | level 4 | level 6 | level 8 | level 9 | level 12 |
|---|---|---|---|---|---|---|---|---|
| flate2 / miniz_oxide | 202% / 794 Mbit/s | – | 394% / 639 Mbit/s | – | 546% / 629 Mbit/s | – | 603% / 626 Mbit/s | – |
| flate2 / zlib-rs | **132% / 889 Mbit/s** | 233% / 690 Mbit/s | 267% / 650 Mbit/s | – | 328% / 635 Mbit/s | – | 514% / 626 Mbit/s | – |
| flate2 / zlib-ng | 142% / 889 Mbit/s | – | 275% / 650 Mbit/s | – | 338% / 635 Mbit/s | – | 611% / 626 Mbit/s | – |
| libdeflate | **169% / 663 Mbit/s** | 243% / 652 Mbit/s | – | 266% / 645 Mbit/s | – | 448% / 634 Mbit/s | – | 1011%, see below |

`–` means that level was not measured for that implementation. Note that the level scales are not
the same: flate2 (i.e. zlib) goes up to 9, libdeflate up to 12, and equal numbers do *not* mean
equal compression. libdeflate level 1 already compresses better than zlib level 2.

Every cell above sustained the full 30 fps, except libdeflate level 12, which managed only **7 of
30 fps at 1011% CPU**. Do not use the top levels - across the whole range from level 1 to level 12
you buy about 6% less traffic for six times the CPU.

### Level 0 (no compression)

Level 0 is a special case and is listed separately, because at 8.30 MB per frame not even a
loopback connection kept up with 30 fps, so these numbers are not comparable to the table above:

| Implementation | CPU | achieved |
|---|---|---|
| libdeflate | 24% | 23 fps |
| flate2 / zlib-ng | 29% | 22 fps |
| flate2 / zlib-rs | 32% | 26 fps |
| flate2 / miniz_oxide | **212%** | 19 fps |

This is worth knowing for a different reason: it shows what everything *except* compressing costs.
Copying the framebuffer and pushing ~250 MB/s into the websocket is nearly free. It also shows that
miniz_oxide has no real "store" fast path - it still runs the whole deflate machinery and burns
212% CPU to not compress anything, while libdeflate just copies the bytes.

## Which one do I want?

**zlib-rs is the default**, because the `web` feature is enabled by default and zlib-rs is pure
Rust: breakwater stays buildable anywhere a Rust toolchain runs, with nothing to install. It is also
the fastest of the zlib-compatible encoders we measured, beating the C library it was ported from
(zlib-ng) at every level.

**Build with `--features web-libdeflate` if you care about traffic or CPU.** libdeflate offers an
operating point that zlib's level scale simply does not have: 663 Mbit/s at 169% CPU. To get the
same traffic out of zlib-rs you need level 3, which costs 267% CPU. The price is a C compiler at
build time - the library is vendored and built from source, so there is no system library to
install.

Careful when switching: **the two level scales are not comparable.** At the default level 1 you get
889 Mbit/s @ 132% CPU with zlib-rs, but 663 Mbit/s @ 169% CPU with libdeflate. Enabling
`web-libdeflate` and keeping the level therefore *lowers* your traffic and *raises* your CPU - it is
a better trade, not a strictly cheaper one. If you want to compare fairly, compare at equal traffic.

The two we did *not* pick:

**flate2 / zlib-ng** buys nothing over zlib-rs (it is the C library zlib-rs was ported from) while
needing both a C compiler *and* cmake.

**flate2 / miniz_oxide** is flate2's default backend, and dominated by both of our choices: it is
slower than zlib-rs at every level, and libdeflate level 1 is cheaper *and* sends less than
miniz_oxide level 1.

## How this was measured

- A static 1920x1080 image (a Minecraft screenshot) flooded onto the canvas via Pixelflut, then
  left alone, so the canvas content is identical for every run:
  `sturmflut ::1 Screenshot_20260712_145807.png -t 1`
- `breakwater --width 1920 --height 1080 --fps 30 --enable-sink web`, release build,
  16 compression chunks (the default).
- Exactly one viewer: a real Chrome watching `http://localhost:8080/`.
- CPU is the sum over all of breakwater's threads across a 12 second window, so 100% means one
  fully busy core. Only the Pixelflut server is measured, not the browser.
- Traffic is the payload actually received over the websocket, measured over a 5 second window
  *after* the CPU window so it cannot disturb it.
- Versions: libdeflater 1.25.2, flate2 1.1.9, miniz_oxide 0.8.9, zlib-rs 0.6.7, libz-ng-sys 1.1.29.
- Hardware: AMD Ryzen AI 9 HX 370 laptop (24 threads).

[`DecompressionStream`]: https://developer.mozilla.org/en-US/docs/Web/API/DecompressionStream
