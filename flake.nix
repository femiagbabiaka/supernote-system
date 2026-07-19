{
  description = "Supernote → Google Workspace meeting/action automation";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        supernote-system = pkgs.callPackage ./nix/package.nix { };
        supernote-renderer = pkgs.callPackage ./nix/renderer.nix { };
        default = supernote-system;
      });

      nixosModules.default = import ./nix/module.nix self;

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            sqlx-cli
            sqlite
            rclone
            dejavu_fonts
            (python3.withPackages (
              ps: with ps; [
                # supernotelib pulled from PyPI via renderer.nix at build time;
                # for dev, `pip install supernotelib` into this interpreter's venv
                # or use `nix build .#supernote-renderer`.
                pillow
                numpy
              ]
            ))
          ];
          env.SUPERNOTE_FONT_DIR = "${pkgs.dejavu_fonts}/share/fonts/truetype";
        };
      });
    };
}
