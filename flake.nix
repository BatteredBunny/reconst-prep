{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
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
        reconst-prep = final.callPackage ./nix/package.nix { };
      };

      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor.${system};
          scope = lib.makeScope pkgs.newScope (final: self.overlays.default final pkgs);
        in
        {
          inherit (scope) reconst-prep;
          default = scope.reconst-prep;
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = pkgsFor.${system};
        in
        {
          package = self.packages.${system}.reconst-prep;

          clippy = self.packages.${system}.reconst-prep.overrideAttrs (old: {
            pname = "${old.pname}-clippy";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
            buildPhase = ''
              runHook preBuild
              cargo clippy --offline --release --workspace --all-targets -- -D warnings
              runHook postBuild
            '';
            installPhase = "touch $out";
            dontFixup = true;
            doCheck = false;
            doInstallCheck = false;
          });

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
        }
      );

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
