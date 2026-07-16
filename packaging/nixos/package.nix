{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "portway";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../Cargo.lock
      ../../Cargo.toml
      ../../src
      ../../web
    ];
  };
  cargoLock.lockFile = ../../Cargo.lock;

  meta = {
    description = "Self-hosted browser remote mouse and keyboard controller";
    homepage = "https://github.com/heptanal/portway";
    license = lib.licenses.mit;
    mainProgram = "portway";
    platforms = lib.platforms.linux;
  };
}
