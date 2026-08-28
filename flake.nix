{
  description = "fig — a format-preserving config-file parser/editor CLI and library";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # The Zig toolchain fig is built with, and the dev shell that carries it.
    # The version itself lives in diaryx-org/nix, because prov builds fig
    # through its build script and so has to agree with this repo about it —
    # `nix eval --raw github:diaryx-org/nix#versions.zig` is what CI reads.
    diaryx-nix.url = "github:diaryx-org/nix";
    diaryx-nix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, diaryx-nix }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        zig = diaryx-nix.lib.${system}.zig;

        # This flake ships the `fig` CLI binary, which carries its OWN SemVer
        # track (`cli_version` in build.zig), independent of the core library
        # version in build.zig.zon. Parse that so the flake reports the same
        # version as `fig version` rather than the core number.
        cliVersion =
          let m = builtins.match ''.*cli_version = std\.SemanticVersion\.parse\("([^"]+)"\).*''
                    (builtins.readFile ./build.zig);
          in if m == null
             then throw "fig flake: could not find `cli_version` in build.zig"
             else builtins.head m;
      in {
        packages = rec {
          default = fig;

          fig = pkgs.stdenv.mkDerivation {
            pname = "fig";
            version = cliVersion;
            src = ./.;

            nativeBuildInputs = [ zig ];

            # fig has no build.zig.zon dependencies, so the build needs no
            # network access — but Zig still wants a writable cache dir, which
            # the read-only Nix store won't provide.
            dontConfigure = true;
            dontInstall = true; # `zig build --prefix $out` installs directly.

            buildPhase = ''
              runHook preBuild
              export HOME="$TMPDIR"
              export ZIG_GLOBAL_CACHE_DIR="$TMPDIR/zig-global-cache"
              export ZIG_LOCAL_CACHE_DIR="$TMPDIR/zig-local-cache"
              zig build --prefix "$out" -Doptimize=ReleaseFast -Dstrip=true
              runHook postBuild
            '';

            meta = {
              description = "Format-preserving config-file parser/editor (YAML, JSON, TOML, ZON, INI, ...)";
              homepage = "https://github.com/diaryx-org/fig";
              license = with pkgs.lib.licenses; [ mit asl20 ];
              mainProgram = "fig";
              platforms = pkgs.lib.platforms.unix ++ pkgs.lib.platforms.windows;
            };
          };
        };

        apps.default = {
          type = "app";
          program = "${self.packages.${system}.fig}/bin/fig";
        };

        # git-cliff comes with the shared shell: it drives tools/changelog.sh,
        # which regenerates the generated region of docs/CHANGELOG.md. Not
        # needed to build or test fig — only to cut a release — so `zig build
        # changelog` still says how to get it rather than assuming this shell.
        devShells.default = diaryx-nix.devShells.${system}.zig;
      });
}
