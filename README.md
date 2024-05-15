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

If you prefer the manual way (the best performance - as e.g. can use native SIMD instructions) you need to have [Rust installed](https://www.rust-lang.org/tools/install).
You may need to install some additional packages with `sudo apt install pkg-config libvncserver-dev`
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

If you want to permanently save statistics (to keep them between restarts) you can use the following command:

```bash
mkdir -p pixelflut && docker run --rm -u 1000:1000 --init -t -p 1234:1234 -p 5900:5900 -p 9100:9100 -v "$(pwd)/pixelflut:/pixelflut" sbernauer/breakwater --statistics-save-file /pixelflut/statistics.json
```

# Ready to use Docker compose setup
The ready to use Docker compose setup contains the Pixelflut server, a Prometheus server and a Grafana for monitoring.
Use the following command to start the whole setup

```bash
cd docker && docker-compose up
```

You should now have access to the following services

| Port | Description                 |
|------|-----------------------------|
| 1234 | Pixelflut server            |
| 5900 | VNC server                  |
| 9100 | Prometheus metrics exporter |
| 9090 | Prometheus server           |
| 3000 | Grafana                     |

If you visit the Grafana server (user=admin, password=admin) you will have access to dashboards like the dashboard below.

![Grafana screenshot](docs/images/Screenshot_20220210_215752.png)

## Live streaming via Webinterface (owncast)

The docker-compose setup also contains an owncast server, which breakwater pushes an RTMP stream into.
owncast than exposes a Web UI where people can watch the game in a web-browser.
Please note that the stream has a much higer delay compared to VNC and the fmmpeg command used internally consumes much CPU.
Because of this the components are commented out by default, you need to comment them in!

## Live streaming to internet services (e.g. Youtube, Twich)

This should work the same way as streaming to owncast.
Simply uncomment the `breakwater` command and adopt `--rtmp-address` accordingly.
I never used this for a longer time period, so happy about feedback!

# Performance

## Laptop

:warning: The figures for breakwater below are outdated. The performance of breakwater has increased significant in the meantime, but I don't have access to the Laptop any more.
See the [Server section](#server) below for up-to-date performance numbers.

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
As I don't have access to a dedicated server any more I did run the following benchmark on a Hetzner sever.

Server type: [`CCX62`](https://www.hetzner.com/cloud) with `48` dedicated AMD EPYC cores and `192` GB RAM.
[Sturmflut](https://github.com/TobleMiner/sturmflut) was used as a client using the loopback interface.
The whole test setup did cost less than 1€ for one hour, so please feel free to submit performance numbers for other pixelflut servers or validate the results!

| Server                                                                                         | Language | Sustainable traffic |
|------------------------------------------------------------------------------------------------|----------|---------------------|
| [Shoreline](https://github.com/TobleMiner/shoreline)@05a2bbfb4559090727c51673e1fb47d20eac5672  | C        | 55 Gbit/s           |
| [Breakwater](https://github.com/sbernauer/breakwater)@4cc8e2a4c7fd03886ede3061d6359c8063665755 | Rust     | 110 Gbit/s          |

<details>
  <summary>lscpu command output</summary>

```
Architecture:            x86_64
  CPU op-mode(s):        32-bit, 64-bit
  Address sizes:         40 bits physical, 48 bits virtual
  Byte Order:            Little Endian
CPU(s):                  48
  On-line CPU(s) list:   0-47
Vendor ID:               AuthenticAMD
  Model name:            AMD EPYC Processor
    CPU family:          25
    Model:               1
    Thread(s) per core:  2
    Core(s) per socket:  24
    Socket(s):           1
    Stepping:            1
    BogoMIPS:            4792.80
    Flags:               fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ht syscall nx mmxext fxsr_opt pdpe1gb rdts
                         cp lm rep_good nopl cpuid extd_apicid tsc_known_freq pni pclmulqdq ssse3 fma cx16 pcid sse4_1 sse4_2 x2apic movbe popcnt aes xsave avx f16c
                         rdrand hypervisor lahf_lm cmp_legacy cr8_legacy abm sse4a misalignsse 3dnowprefetch osvw topoext perfctr_core invpcid_single ssbd ibrs ibpb
                         stibp vmmcall fsgsbase bmi1 avx2 smep bmi2 erms invpcid rdseed adx smap clflushopt clwb sha_ni xsaveopt xsavec xgetbv1 xsaves clzero xsaveer
                         ptr wbnoinvd arat umip pku ospke rdpid fsrm
Virtualization features:
  Hypervisor vendor:     KVM
  Virtualization type:   full
Caches (sum of all):
  L1d:                   768 KiB (24 instances)
  L1i:                   768 KiB (24 instances)
  L2:                    12 MiB (24 instances)
  L3:                    32 MiB (1 instance)
NUMA:
  NUMA node(s):          1
  NUMA node0 CPU(s):     0-47
Vulnerabilities:
  Itlb multihit:         Not affected
  L1tf:                  Not affected
  Mds:                   Not affected
  Meltdown:              Not affected
  Mmio stale data:       Not affected
  Retbleed:              Not affected
  Spec store bypass:     Mitigation; Speculative Store Bypass disabled via prctl and seccomp
  Spectre v1:            Mitigation; usercopy/swapgs barriers and __user pointer sanitization
  Spectre v2:            Mitigation; Retpolines, IBPB conditional, IBRS_FW, STIBP conditional, RSB filling, PBRSB-eIBRS Not affected
  Srbds:                 Not affected
  Tsx async abort:       Not affected
```

</details>

## 80 core ARM server

As I got access to an server with a single Ampere(R) Altra(R) Processor with 80 cores, here are the results:

[Sturmflut](https://github.com/TobleMiner/sturmflut) was used as a client using the loopback interface.

| Server                                                                                         | Language | Sustainable traffic |
|------------------------------------------------------------------------------------------------|----------|---------------------|
| [Shoreline](https://github.com/TobleMiner/shoreline)@05a2bbfb4559090727c51673e1fb47d20eac5672  | C        | 185 Gbit/s          |
| [Breakwater](https://github.com/sbernauer/breakwater)@135d2b795858a896a73470fd152407c22b0a0d26 | Rust     | 415 Gbit/s          |

<details>
  <summary>lscpu command output</summary>

```
Architecture:           aarch64
  CPU op-mode(s):       32-bit, 64-bit
  Byte Order:           Little Endian
CPU(s):                 80
  On-line CPU(s) list:  0-79
Vendor ID:              ARM
  Model name:           Neoverse-N1
    Model:              1
    Thread(s) per core: 1
    Core(s) per socket: 80
    Socket(s):          1
    Stepping:           r3p1
    Frequency boost:    disabled
    CPU max MHz:        3000.0000
    CPU min MHz:        1000.0000
    BogoMIPS:           50.00
    Flags:              fp asimd evtstrm aes pmull sha1 sha2 crc32 atomics fphp asimdhp cpuid asimdrdm lrcpc dcpop asimddp ssbs
Caches (sum of all):
  L1d:                  5 MiB (80 instances)
  L1i:                  5 MiB (80 instances)
  L2:                   80 MiB (80 instances)
NUMA:
  NUMA node(s):         1
  NUMA node0 CPU(s):    0-79
Vulnerabilities:
  Gather data sampling: Not affected
  Itlb multihit:        Not affected
  L1tf:                 Not affected
  Mds:                  Not affected
  Meltdown:             Not affected
  Mmio stale data:      Not affected
  Retbleed:             Not affected
  Spec rstack overflow: Not affected
  Spec store bypass:    Mitigation; Speculative Store Bypass disabled via prctl
  Spectre v1:           Mitigation; __user pointer sanitization
  Spectre v2:           Mitigation; CSV2, BHB
  Srbds:                Not affected
  Tsx async abort:      Not affected
```

</details>
