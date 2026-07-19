{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "supernote-system";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: type:
      let
        rel = lib.removePrefix (toString ../. + "/") (toString path);
      in
      lib.any (p: lib.hasPrefix p rel) [
        "Cargo.toml"
        "Cargo.lock"
        "crates"
        "migrations"
      ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  meta = {
    description = "Supernote → Google Workspace meeting/action automation (webapp, templater, ingest agent)";
    license = lib.licenses.mit;
    mainProgram = "supernote-webapp";
  };
}
