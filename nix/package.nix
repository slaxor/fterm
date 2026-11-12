{ lib, rustPlatform }:
let
  manifest = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = "fterm";
  version = manifest.workspace.package.version;
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [ ../Cargo.toml ../Cargo.lock ../lib ../cli ];
  };
  cargoLock.lockFile = ../Cargo.lock;
  meta = {
    description = "Serial terminal for Forth targets with flow-controlled uploads and Forth macros";
    license = lib.licenses.gpl3Only;
    mainProgram = "fterm";
  };
}
