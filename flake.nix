{
  inputs = {
    nixpkgs.url = "https://channels.nixos.org/nixos-unstable/nixexprs.tar.zst";

    crane.url = "github:ipetkov/crane";

    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      git-hooks,
      ...
    }:
    let
      inherit (nixpkgs) lib;

      # TODO: support darwin in the future
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      forAllSystems = lib.genAttrs systems;

      pkgsFor = forAllSystems (system: nixpkgs.legacyPackages.${system});
    in
    {
      overlays.default = final: _prev: {
        reconst-prep = final.callPackage ./nix/package.nix { craneLib = crane.mkLib final; };
        reconst-prep-cli = final.reconst-prep.override { withGui = false; };
      };

      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor.${system};
          scope = lib.makeScope pkgs.newScope (final: self.overlays.default final pkgs);
        in
        {
          inherit (scope) reconst-prep reconst-prep-cli;
          default = scope.reconst-prep;
          screenshots = pkgs.callPackage ./nix/screenshots.nix {
            inherit (scope) reconst-prep;
            craneLib = crane.mkLib pkgs;
          };
        }
      );

      checks = forAllSystems (system: {
        package = self.packages.${system}.reconst-prep;

        inherit (self.packages.${system}.reconst-prep.passthru) clippy;

        pre-commit = git-hooks.lib.${system}.run {
          src = ./.;
          hooks = {
            rustfmt.enable = true;
            clippy = {
              enable = true;
              stages = [ "pre-push" ];
            };
          };
        };
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor.${system};
          guiLibs = with pkgs; [
            libGL
            libxkbcommon
            wayland
            libx11
            libxcursor
            libxi
            libxrandr
            vulkan-loader
          ];
        in
        {
          default = pkgs.mkShell {
            packages =
              (with pkgs; [
                cargo
                rustc
                rust-analyzer
                clippy
                rustfmt
                just
                pkg-config
                python3
                ffmpeg
                gtk3
                glib
              ])
              ++ guiLibs
              ++ self.checks.${system}.pre-commit.enabledPackages;

            LD_LIBRARY_PATH = lib.makeLibraryPath guiLibs;

            shellHook = ''
              ${self.checks.${system}.pre-commit.shellHook}

              export XDG_DATA_DIRS="${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
            '';
          };
        }
      );
    };
}
