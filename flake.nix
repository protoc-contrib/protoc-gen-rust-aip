{
  description = "protoc-gen-rust-aip - A protoc plugin that emits Rust helpers for Google AIP resource names, List-RPC query handling and field behavior";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    nix-release-bin = {
      url = "github:nixos-contrib/nix-release-bin";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      rust-overlay,
      nix-release-bin,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
        rust-toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

        source = pkgs.rustPlatform.buildRustPackage {
          pname = manifest.name;
          inherit (manifest) version;
          src = pkgs.lib.cleanSource ./.;
          doCheck = false;

          cargoLock.lockFile = ./Cargo.lock;

          # `build.rs` compiles the vendored google/api annotations, so protoc
          # is needed to build and not only to develop. Named outright rather
          # than left to a PATH search, which the build sandbox would lose.
          nativeBuildInputs = [ pkgs.protobuf ];
          env.PROTOC = "${pkgs.protobuf}/bin/protoc";

          # Only the plugin. The other workspace member is the fixture, which
          # exists to regenerate and type-check the test schema and has no place
          # in the binary being shipped.
          cargoBuildFlags = [
            "--package"
            "protoc-gen-rust-aip"
          ];

          meta = with pkgs.lib; {
            inherit (manifest) description homepage;
            license = licenses.mit;
            mainProgram = manifest.name;
          };
        };
      in
      {
        packages = {
          # The latest release binary, where it has one for the system: CI pins
          # them in the manifest once the release has published them.
          default = nix-release-bin.lib.mkReleaseBin {
            inherit pkgs;
            manifest = ./.github/config/nix-release-bin-manifest.json;
            pname = manifest.name;
            fallback = source;
          };
          inherit source;
        };

        devShells.default = pkgs.mkShell {
          inherit (manifest) name;
          packages = [
            rust-toolchain
            pkgs.protobuf
          ];
        };
      }
    );
}
