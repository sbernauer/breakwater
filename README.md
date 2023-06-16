# breakwater
breakwater is a very fast [Pixelflut](https://wiki.cccgoe.de/wiki/Pixelflut) server written in Rust. It is heavily inspired by [Shoreline](https://github.com/TobleMiner/shoreline).

It claims to be the fastest Pixelflut server in existence - at least at the time of writing 02/2022.
![breakwater logo](docs/images/breakwater.png)

# Features
1. Accepts Pixelflut commands
2. Can provide a VNC server so that everybody can watch
3. As an alternative it can stream to a RTMP sink, so that you can e.g. directly live-stream into Twitch or YouTube
4. Exposes Prometheus metrics
5. IPv6 and legacy IP support

# Available Pixelflut commands
Commands must be sent newline-separated, for more details see [Pixelflut](https://wiki.cccgoe.de/wiki/Pixelflut)
* `HELP`: Prints a help text with the available commands.
* `PX x y rrggbb`: PX x y rrggbb: Color the pixel (x,y) with the given hexadecimal color rrggbb, e.g. `PX 10 10 ff0000`
* `PX x y rrggbbaa`: Color the pixel (x,y) with the given hexadecimal color rrggbb (alpha channel is ignored for now), e.g. `PX 10 10 ff0000ff`
* `PX x y gg`: Color the pixel (x,y) with the hexadecimal color gggggg. Basically this is the same as the other commands, but is a more efficient way of filling white, black or gray areas, e.g. `PX 10 10 00` to paint black
* `PX x y`: Get the color value of the pixel (x,y), e.g. `PX 10 10`
* `SIZE`: Get the size of the drawing surface, e.g. `SIZE 1920 1080`
* `OFFSET x y`: Apply offset (x,y) to all further pixel draws on this connection. This can e.g. be used to pre-calculate an image/animation and simply use the OFFSET command to move it around the screen without the need to re-calculate it, e.g. `OFFSET 100 100`

# Usage
The easiest way is to continue with the provided [Ready to use Docker setup](#run-in-docker-container) below.

If you prefer the manual way (the best performance) you need to have [Rust installed](https://www.rust-lang.org/tools/install).
You may need to install some additional packages with `sudo apt install pkg-config libvncserver-dev `
Then you can directly run the server with
```bash
cargo run --release
```
The default settings should provide you with a ready-to-use server.

| Port | Description                 |
|------|-----------------------------|
| 1234 | Pixelflut server            |
| 5900 | VNC server                  |
| 9100 | Prometheus metrics exporter |

The get a list of options try
```bash
cargo run --release -- --help
```
<details>
  <summary>Output</summary>

```bash
cargo run --release -- --help
    Finished release [optimized] target(s) in 0.04s
     Running `target/release/breakwater --help`
Usage: breakwater [OPTIONS]

Options:
  -l, --listen-address <LISTEN_ADDRESS>
          Listen address to bind to. The default value will listen on all interfaces for IPv4 and IPv6 packets [default: [::]:1234]
      --width <WIDTH>
          Width of the drawing surface [default: 1280]
      --height <HEIGHT>
          Height of the drawing surface [default: 720]
  -f, --fps <FPS>
          Frames per second the server should aim for [default: 30]
  -t, --text <TEXT>
          Text to display on the screen. The text will be followed by "on <listen_address>" [default: "Pixelflut server (breakwater)"]
      --font <FONT>
          The font used to render the text on the screen. Should be a ttf file. If you use the default value a copy that ships with breakwater will be used - no need to download and provide the font [default: Arial.ttf]
  -p, --prometheus-listen-address <PROMETHEUS_LISTEN_ADDRESS>
          Listen address the prometheus exporter should listen on [default: [::]:9100]
      --statistics-save-file <STATISTICS_SAVE_FILE>
          Save file where statistics are periodically saved. The save file will be read during startup and statistics are restored. To reset the statistics simply remove the file [default: statistics.json]
      --statistics-save-interval-s <STATISTICS_SAVE_INTERVAL_S>
          Interval (in seconds) in which the statistics save file should be updated [default: 10]
      --disable-statistics-save-file
          Disable periodical saving of statistics into save file
      --rtmp-address <RTMP_ADDRESS>
          Enable rtmp streaming to configured address, e.g. `rtmp://127.0.0.1:1935/live/test`
      --video-save-folder <VIDEO_SAVE_FOLDER>
          Enable dump of video stream into file. File location will be `<VIDEO_SAVE_FOLDER>/pixelflut_dump_{timestamp}.mp4
  -v, --vnc-port <VNC_PORT>
          Port of the VNC server [default: 5900]
  -h, --help
          Print help
  -V, --version
          Print version
```
</details>

You can also build the binary with `cargo build --release`. The binary will be placed at `target/release/breakwater`.

## Compile time features
Breakwater also has some compile-time features for performance reasons.
You can get the list of available features by looking at the [Cargo.toml](Cargo.toml).
As of writing the following features are supported

* `vnc` (enabled by default): Starts a VNC server, where users can connect to. Needs `libvncserver-dev` to be installed. Please note that the VNC server offers basically no latency, but consumes quite some CPU.
* `alpha` (disabled by default): Respect alpha values during `PX` commands. Disabled by default as this can cause performance degradation.

To e.g. turn the VNS server off, build with

```bash
cargo run --release --no-default-features # --features alpha,vnc to explicitly enable
```

## Usage of SIMD and nightly Rust
[Fabian Wunsch](https://github.com/fabi321) has introduced initial support for SIMD when parsing the hexadecimal color values in [#5](https://github.com/sbernauer/breakwater/pull/5). Thanks!
We might be able to extend the support, parsing the decimal coordinates or blending colors using alpha using SIMD would be awesome as well. PRs welcome!

As `portable_simd` is a unstable feature, we configured this project to use nightly Rust in `rust-toolchain.toml`. Once the feature is stable we can switch back to stable Rust.

If the SIMD or nightly part causes any problems on your setup please reach out by [creating an Issue](https://github.com/sbernauer/breakwater/issues/new)!

# Run in docker container
This command will start the Pixelflut server in a docker container
```bash
docker run --rm --init -t -p 1234:1234 -p 5900:5900 -p 9100:9100 sbernauer/breakwater # --help
```

# Ready to use Docker compose setup
The ready to use Docker compose setup contains the Pixelflut server, a prometheus server and a Grafana for monitoring.
Use the following command to start the whole setup
```bash
docker-compose up
```
You should now have access to the following services

| Port | Description                 |
|------|-----------------------------|
| 1234 | Pixelflut server            |
| 5900 | VNC server                  |
| 9100 | Prometheus metrics exporter |
| 9090 | Prometheus server           |
| 80   | Grafana                     |

If you visit the Grafana server (user=admin, password=admin) you will have access to dashboards like the dashboard below.
![Grafana screenshot](docs/images/Screenshot_20220210_215752.png)

# Performance

:warning: The figures below are outdated. The performance of breakwater has increased significant in the meantime. I sadly don't have access to the server any more to run the benchmarks - happy about any figures for a beefy system. At GPN 21 we were able to reach 80 Gbit/s (40 via network and 40 via loopback) on a 20 core/40 thread system while serving > 500 connections :)

## Laptop
My Laptop has a `Intel(R) Core(TM) i7-8850H CPU @ 2.60GHz` (6 Cores/12 Threads) and 2 DDR4 RAM modules with 16 GB each and 2667 MT/s.
The Pixelflut-server and Pixelflut-client [Sturmflut](https://github.com/TobleMiner/sturmflut) both run on my Laptop using 24 connections.
These are the results of different Pixelflut servers:

| Server                                                                  | Language | Traffic during first 30s | When thermal throttling |
|-------------------------------------------------------------------------|----------|--------------------------|-------------------------|
| [Pixelnuke](https://github.com/defnull/pixelflut/tree/master/pixelnuke) | C        | 1.1 Gbit/s               | 1 Gbit/s                |
| [Pixelwar](https://github.com/defnull/pixelflut/tree/master/pixelwar)   | Java     | 2.1 Gbit/s               | 1.6 Gbit/s              |
| [pixelpwnr-server](https://github.com/timvisee/pixelpwnr-server)        | Rust     | 6.3 Gbit/s               | 4.6 Gbit/s             |
| [Shoreline](https://github.com/TobleMiner/shoreline)                    | C        | 15 Gbit/s                | 12 Gbit/s               |
| [Breakwater](https://github.com/sbernauer/breakwater)                   | Rust     | 30 Gbit/s                | 22 Gbit/s               |

## Server
The server has two `Intel(R) Xeon(R) CPU E5-2660 v2 @ 2.20GHz` processors with 10 cores / 20 threads each.
Another server was used as a Pixelflut-client [Sturmflut](https://github.com/TobleMiner/sturmflut).
The servers were connected with two 40G and one 10G links, through which traffic was generated.

| Server                                                | Language | Sustainable traffic |
|-------------------------------------------------------|----------|---------------------|
| [Shoreline](https://github.com/TobleMiner/shoreline)  | C        | 34 Gbit/s           |
| [Breakwater](https://github.com/sbernauer/breakwater) | Rust     | 52 Gbit/s           |
