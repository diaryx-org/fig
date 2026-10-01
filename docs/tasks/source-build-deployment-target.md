```fig
title = A source build of fig-sys on a Mac targets the build machine's macOS
description = With any non-default language feature (`zon`, `plist`) fig-sys builds libfig from Zig source, passes no `-Dtarget` when target equals host, and the archive comes out with minos set to the build machine's OS — so whatever links it refuses to launch on an older macOS than that machine
status = open
created = 2026-09-30
updated = 2026-09-30
part_of = [Tasks](/docs/tasks/tasks.md)
```

# A source build of fig-sys on a Mac targets the build machine's macOS

**Repro.** On an Apple Silicon Mac running macOS 27, in a crate depending on
`fig = { version = "5", features = ["zon"] }`:

```sh
cargo build --target aarch64-apple-darwin
ar -x target/aarch64-apple-darwin/debug/lib<crate>.a libfig_zcu.o
otool -l libfig_zcu.o | grep minos     # minos 27.0
```

The default feature set links the prebuilt `fig-sys-macos-arm64` archive
instead, and that says `minos 13.0`. Setting `MACOSX_DEPLOYMENT_TARGET=13.0`
changes nothing, because Zig does not read it.

**Why it matters.** flower's Mac app declares macOS 13 and its Homebrew
binaries are built on a CI runner. Turning on `zon` would make the app
require the build machine's macOS and the CLI require the runner's. The
linker only warns about it (`was built for newer 'macOS' version (27.0) than
being linked (13.0)`). flower has left ZON out until this is fixed.

**Where.** `bindings/rust/fig-sys/build.rs`, `zig_target_for_cargo_target`,
returns `None` when `target == host`, so `zig build` compiles for the native
OS version.

**Done when** a source build for an Apple target produces the same minimum OS
as the prebuilt archive. Either of these would do it:

- pass `-Dtarget=aarch64-macos.13.0` (and the matching floor for x86_64 and
  iOS) on Apple targets even when the target is the host, reading
  `MACOSX_DEPLOYMENT_TARGET` / `IPHONEOS_DEPLOYMENT_TARGET` where they are set;
- or put `zon` in the prebuilt archive's feature set, so that flower's
  feature set gets the prebuilt archive. This fixes flower's case and no other.
