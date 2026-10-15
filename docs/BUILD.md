```fig
title = fig build instructions
author = adammharris
date = 2026-07-04
updated = 2026-07-04T15:48:19-06:00
part_of = [fig docs](/docs/docs.md)
```

# Building `fig`

You will need:
- [Access to a terminal](https://share.google/CzgpcLKGi8PKS4q7G)
- [Rust](https://rustup.rs), 1.88 or newer
- [Git](https://git-scm.com/install)

You will:
1. Use Git to download the code for `fig`
2. Go into the downloaded code folder
3. Use Cargo, Rust's build tool, to compile the code into an executable

Put these commands into the terminal and run them:

```bash
git clone https://github.com/diaryx-org/fig
cd fig
cargo build --release -p fig-cli
```

Now the code is compiled! Nothing else is needed: fig is Rust throughout, with
no C compiler or other toolchain behind it.

<details>
  <summary>Extra details about compilation</summary>
  <p><code>--release</code> compiles with optimizations, which takes longer but gives a much faster binary; leave it off for a quicker "debug" build while you are changing the code, which lands in <code>target/debug</code> instead. The binaries distributed through Homebrew and GitHub Releases are release builds.</p>
</details>

<details>
  <summary>The formats in the build</summary>

The `fig` command line always carries all ten built-in formats. The C library
carries none: a program registers the formats it uses, and each language
builds as a library of its own (see [Languages](../languages/README.md)), so a
program that wants a smaller audit surface links only those. One format is
opt-**in** on the CLI: `canonical`, fig's oracle format for testing
(`cargo build --release -p fig-cli --features canonical`).
</details>

In the `fig` folder, a new folder called `target` has been created.
- The command-line tool is at `target/release/fig`.
- `fig-lsp`, the language server for figl, is beside it.

The C library is built separately, from the `fig-capi` crate:

```bash
cargo rustc -p fig-capi --release --crate-type staticlib
```

Cargo names it after the crate, `target/release/libfig_capi.a`; it is the
library [`fig.h`](../bindings/c/include/fig.h) describes, `libfig.a`, under
another name, and `cargo xtask abi-check` copies it to that name.

You can run the command-line tool where it is:

```bash
./target/release/fig
```

It should print helpful instructions on how to use it.

# Installing `fig`

If you want easy access to the command-line tool without having to navigate to the proper folder every time, you will have to add it to your `PATH` shell variable. The standard way to do this is to move the compiled binary to a `bin` folder.

To install for just your user:

```bash
mv target/release/fig ~/.local/bin/
```

To install for for all users in the computer (this requires an admin password):

```bash
sudo mv target/release/fig /usr/local/bin
```

`fig` does not update itself to new versions automatically. To get the latest version, you will have to download the latest changes and repeat the above steps again:

```bash
cd fig
git pull
cargo build --release -p fig-cli
mv target/release/fig ~/.local/bin
```

You can use a **package manager** to make updating easier. Right now, `fig` only supports Homebrew for macOS:

- [Install Homebrew](https://brew.sh)
- Run `brew tap diaryx-org/tap && brew install diaryx-org/tap/fig`
- To update, run `brew upgrade diaryx-org/tap/fig`

## nix-darwin

I personally use Nix on my MacBook to manage packages.
I integrate it with Homebrew by adding this to my `configuration.nix`:

```nix
homebrew = {
  enable = true;
  taps = [
    "diaryx-org/tap"
  ];
  brews = [
    "fig"
  ];
};
```

`fig` isn't an official Nix package, but I'm sure Linux users are smart enough to figure it out!

# Changing Code

If you want,
you can make changes to the code (the core is in `rust/fig-core`, the command
line in `rust/fig-cli`, each format in `languages/<name>/rust`)
and then run `cargo build --release -p fig-cli` again to see how your changes affected the `fig` binary.

If you make changes and want to share them with me,
be sure to follow the instructions [here](CONTRIBUTING.md).
