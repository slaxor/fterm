{
  description = "fterm";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };
  outputs = { nixpkgs, rust-overlay, ... }:
    let
      pkgs = import nixpkgs {
        system = "x86_64-linux";
        overlays = [ rust-overlay.overlays.default ];
      };
      rust = pkgs.rust-bin.stable.latest.default.override {
        extensions = [ "rust-src" "rust-analyzer" ];
      };
    in {
      packages.x86_64-linux.default = pkgs.rustPlatform.buildRustPackage {
        pname = "fterm";
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        meta.mainProgram = "fterm";
      };
      devShells.x86_64-linux.default = pkgs.mkShell {
        packages = [ rust pkgs.just pkgs.cargo-nextest ];
      };
    };
}
