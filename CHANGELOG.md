# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added

- Support using shared memory for the framebuffer. This e.g. allows  on-Rust applications to join
  the shared memory and read/write screen contents ([#55])

### Changed

- Bump to `2024` Rust edition ([#52])
- Replace `snafu` with `eyre` and `thiserror` for error handling ([#53])
  - Use `color-eyre` for prettier error reports
- Use `tracing` for logging ([#54])

[#52]: https://github.com/sbernauer/breakwater/pull/52
[#53]: https://github.com/sbernauer/breakwater/pull/53
[#54]: https://github.com/sbernauer/breakwater/pull/54
[#55]: https://github.com/sbernauer/breakwater/pull/55

## [0.17.0] - 2025-02-14

### Added

- Add a customizable multi-window native display using egui ([#48])

### Fixed

- BREAKING: Count IPv4 and IPv6 statistics individually to avoid confusion.
  Because of this, the Prometheus metrics `breakwater_ips` and `breakwater_legacy_ips` have been removed,
  use `breakwater_ips_v4` and `breakwater_ips_v6` instead ([#50])
- Ensure statistic information updates are send periodically ([#49])

[#48]: https://github.com/sbernauer/breakwater/pull/48
[#49]: https://github.com/sbernauer/breakwater/pull/49
[#50]: https://github.com/sbernauer/breakwater/pull/50

## [0.16.3] - 2025-01-11

### Fixed

- Receive buffers for pixelflut connections that were leaked before will now be properly deallocated ([#46])
- Errors while allocating receive buffers for pixelflut connections are now handled properly ([#46])

[#46]: https://github.com/sbernauer/breakwater/pull/46

## [0.16.2] - 2024-12-30

### Fixed

- Ignore spurious zero-sized window buffers in native display sink ([#42])
- Use physical size for winit window in native display sink ([#42])

[#42]: https://github.com/sbernauer/breakwater/pull/42

## [0.16.1] - 2024-09-02

### Fixed

- Fixed problem with docker build, which resulted in 0.16.0 to be missing on Docker Hub ([#40])
- Only offer the CLI arguments `--vnc` and `--native-display` when the corresponding features are enabled ([#41])

[#40]: https://github.com/sbernauer/breakwater/pull/40
[#41]: https://github.com/sbernauer/breakwater/pull/41

## [0.16.0] - 2024-08-19

### Added

- Added support for binary sync protocol behind the `binary-sync-pixels` feature ([#34])
- Added support for native display output ([#38])

### Changed

- BREAKING: Feature `binary-commands` has been renamed to `binary-set-pixel` ([#34])
- BREAKING: Remove the `breakwater-core` crate ([#37])
- Add a `FrameBuffer` trait, rename the existing implementation one to `SimpleFrameBuffer` ([#37])
- Add a `DisplaySink`´trait, so that new sinks can be added more easily ([#38])
- BREAKING: No display sink is now started by default. To start the VNC server add the `--vnc` CLI argument ([#38])

### Fixed

- Performance improvement due to addition of missing set of `last_byte_parsed` when sending binary pixel commands ([#36])
- Minor performance improvement when parsing `HELP` and `SIZE` requests ([#36])

[#34]: https://github.com/sbernauer/breakwater/pull/34
[#36]: https://github.com/sbernauer/breakwater/pull/36
[#37]: https://github.com/sbernauer/breakwater/pull/37
[#38]: https://github.com/sbernauer/breakwater/pull/38

## [0.15.0] - 2024-06-12

### Added

- Support binary protocol ([#33])
- Try to improve performance by calling `madvise` to inform Kernel we are reading sequentially ([#24])
- Expose metric on denied connection counts ([#26])
- Print nicer error messages ([#32])

### Changed

- Ignore repeated `HELP` requests ([#25])
  - Only the first 2 requests of any `parse` patch are answered
  - Answers `Stop spamming HELP!` on the third request
  - Doesn't respond to any further requests

[#24]: https://github.com/sbernauer/breakwater/pull/24
[#25]: https://github.com/sbernauer/breakwater/pull/25
[#26]: https://github.com/sbernauer/breakwater/pull/26
[#32]: https://github.com/sbernauer/breakwater/pull/32
[#33]: https://github.com/sbernauer/breakwater/pull/33

## [0.14.0] - 2024-05-30 at GPN 22 :)

### Added

- Command line option `--connections-per-ip` that allows limiting the number of connections per ip address. Default is unlimited ([#22])

### Fixed

- Raise `ffmpeg` errors as early as possible, e.g. when the `ffmpeg` command is not found

[#22]: https://github.com/sbernauer/breakwater/pull/22

## [0.13.0] - 2024-05-15

### Added

- Also release binary for `aarch64-apple-darwin`

### Changed

- Second rewrite with the following improvements: ([#21])
  * Put `Parser` behind a trait, so that we can have multiple implementations in parallel
  * Use cargo workspaces
  * Better error handling using snafu
- BREAKING: Build release binaries without support for VNC, as this
  * Has a dependecy on a dynamically linked library on the host executing the binary
  * Needs a cross-compilation (which didn't work), as the macOS GitHub runners all run on arm and we try to build an x86 binary

[#21]: https://github.com/sbernauer/breakwater/pull/21
