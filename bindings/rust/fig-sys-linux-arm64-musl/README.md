# fig-sys-linux-arm64-musl

Prebuilt fig core (`libfig.a`) and the library of every language fig ships
(`libfig_json.a`, …) for `aarch64-unknown-linux-musl`, letting `fig-sys` and the language
crates (`fig-json`, `fig-yaml`, …) link on this target **without a Zig
toolchain**.

This is an implementation detail of [`fig-sys`](https://crates.io/crates/fig-sys)
and [`fig`](https://crates.io/crates/fig). You should not depend on it directly;
`fig-sys` pulls in the right payload crate automatically for your target.

## License

MIT OR Apache-2.0
