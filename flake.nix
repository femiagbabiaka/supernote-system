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
            liberation_ttf
            # The packaged renderer puts `supernote-render` on PATH with
            # supernotelib bundled.
            (pkgs.callPackage ./nix/renderer.nix { })
          ];
          env = {
            SUPERNOTE_FONT_DIR = "${pkgs.liberation_ttf}/share/fonts/truetype";
            SUPERNOTE_FONT_NAME = "LiberationSans";
          };
        };
      });
    };
}
