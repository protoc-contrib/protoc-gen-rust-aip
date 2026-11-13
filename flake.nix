{
  description = "protoc-gen-rust-aip - A protoc plugin that emits Rust helpers for Google AIP resource names, List-RPC query handling and field behavior";

  nixConfig = {
    extra-substituters = [ "https://nixos-contrib.cachix.org" ];
    extra-trusted-public-keys = [ "nixos-contrib.cachix.org-1:WRyXCyMQGjYEEC87RdlW4ZbK/R3Ia4/WGYEoIP9X/o0=" ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
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

        package = pkgs.rustPlatform.buildRustPackage {
          pname = manifest.name;
          inherit (manifest) version;
          src = pkgs.lib.cleanSource ./.;
          doCheck = false;

          cargoLock = {
            lockFile = ./Cargo.lock;
            # The fixture's aip-rs is a git dependency (its releases are git
            # tags); Cargo.lock pins its revision, so it is fetched as it is
            # rather than by a hash kept here.
            allowBuiltinFetchGit = true;
          };

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
        packages.default = package;

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
