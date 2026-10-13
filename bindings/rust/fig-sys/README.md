# fig-sys

Low-level FFI bindings and the native static library for
[fig](https://github.com/diaryx-org/fig) — the comment-preserving
JSON / YAML / TOML / … configuration engine.

This crate builds and links `libfig.a` and exposes the raw C ABI (`extern "C"`
declarations and `#[repr(C)]` types). You almost certainly want the safe,
ergonomic wrapper instead:

```toml
fig = "…"
```

## Building the native library

`fig-sys` links a prebuilt static library from a per-target
`fig-sys-<target>` payload crate, so most consumers need **no** Zig toolchain.
The library carries no language — each is a crate of its own (`fig-yaml`, …)
linking its library beside this one — so there is one build of it. A source
build (via `zig`) is used only when:

- the target has no prebuilt payload crate, or
- `FIG_SYS_FORCE_SOURCE=1` is set.

## License

MIT OR Apache-2.0
