{
  description = "bettertree — an interactive terminal file tree driven like Helix";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      perSystem =
        {
          pkgs,
          system,
          ...
        }:
        let
          rustToolchain = pkgs.rust-bin.stable.latest.default;
          craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;

          src = craneLib.cleanCargoSource ./.;

          commonArgs = {
            inherit src;
            strictDeps = true;

            buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.libiconv
              pkgs.apple-sdk
            ];

            nativeCheckInputs = [ pkgs.git ];
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          bettertree = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              doCheck = true;
            }
          );
        in
        {
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ inputs.rust-overlay.overlays.default ];
          };

          packages = {
            default = bettertree;
            inherit bettertree;
          };

          apps.default = {
            type = "app";
            program = "${bettertree}/bin/bt";
          };

          checks.bettertree = bettertree;

          devShells.default = craneLib.devShell {
            inputsFrom = [ bettertree ];
            packages = [
              rustToolchain
              pkgs.rust-analyzer
            ];
          };

          formatter = pkgs.nixfmt;
        };
    };
}
