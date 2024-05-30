# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Fixed

- Raise `ffmpeg` errors as early as possible, e.g. when the `ffmpeg` command is not found

## [0.13.0] - 2024-05-15

## Added

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
