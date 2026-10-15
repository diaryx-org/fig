{
  description = "fig — a format-preserving config-file parser/editor CLI and library";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # The org's shared Rust toolchain and the dev shell that carries it.
    diaryx-nix.url = "github:diaryx-org/nix";
    diaryx-nix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, diaryx-nix }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        # Every fig artifact, the CLI this flake ships included, carries the one
        # version decided in the workspace's Cargo.toml (see docs/VERSIONING.md),
        # so the flake reads it from there and reports what `fig version` does.
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
      in {
        packages = rec {
          default = fig;

          fig = pkgs.rustPlatform.buildRustPackage {
            pname = "fig";
            inherit version;
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            # The CLI crate only (`fig` and `fig-lsp`); the core and the
            # languages are its dependencies.
            cargoBuildFlags = [ "-p" "fig-cli" ];
            cargoTestFlags = [ "-p" "fig-cli" ];

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

        # git-cliff comes with the shared shell: `ship changelog` and `ship`
        # drive it to regenerate the generated region of docs/CHANGELOG.md. Not
        # needed to build or test fig — only to cut a release — so `ship` still
        # says how to get it rather than assuming this shell.
        devShells.default = diaryx-nix.devShells.${system}.rust;
      });
}
