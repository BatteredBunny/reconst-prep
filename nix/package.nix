{
  lib,
  stdenv,
  craneLib,
  pkg-config,
  python3,
  makeWrapper,
  installShellFiles,
  versionCheckHook,
  ffmpeg-headless,
  gtk3,
  gsettings-desktop-schemas,
  libGL,
  libxkbcommon,
  wayland,
  libx11,
  libxcursor,
  libxi,
  libxrandr,
  vulkan-loader,
}:

let
  ffmpeg = ffmpeg-headless;

  guiLibs = [
    libGL
    libxkbcommon
    wayland
    libx11
    libxcursor
    libxi
    libxrandr
    vulkan-loader
  ];

  schemaDirs = lib.concatStringsSep ":" [
    "${gtk3}/share/gsettings-schemas/${gtk3.name}"
    "${gsettings-desktop-schemas}/share/gsettings-schemas/${gsettings-desktop-schemas.name}"
  ];

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      ../assets/icon.svg
      ../assets/icon-256.png
      ../packaging/appimage/reconst-prep.desktop
    ];
  };

  common = {
    inherit src;
    pname = "reconst-prep";
    version = "0.1.0";
    strictDeps = true;
    GYROFLOW_SKIP_LENS_PROFILE_DOWNLOAD = "1";
    # Stylo generates Rust CSS property definitions with its vendored Python modules.
    nativeBuildInputs = [
      pkg-config
      python3
    ];
    buildInputs = guiLibs ++ [ gtk3 ];
  };

  # Depends only on Cargo.lock, so the package and clippy share it and CI's Nix store cache keeps it between commits.
  cargoArtifacts = craneLib.buildDepsOnly common;
  craneArgs = common // {
    inherit cargoArtifacts;
  };

  clippy = craneLib.cargoClippy (
    craneArgs
    // {
      cargoClippyExtraArgs = "--workspace --all-targets -- -D warnings";
    }
  );
in
craneLib.buildPackage (
  craneArgs
  // {
    nativeBuildInputs = common.nativeBuildInputs ++ [
      makeWrapper
      installShellFiles
    ];

    cargoTestExtraArgs = "--workspace";

    postInstall = ''
      rm -rf $out/lib

      install -Dm644 packaging/appimage/reconst-prep.desktop \
        $out/share/applications/reconst-prep.desktop
      install -Dm644 assets/icon.svg \
        $out/share/icons/hicolor/scalable/apps/reconst-prep.svg
      install -Dm644 assets/icon-256.png \
        $out/share/icons/hicolor/256x256/apps/reconst-prep.png
    ''
    + lib.optionalString (stdenv.buildPlatform.canExecute stdenv.hostPlatform) ''
      installShellCompletion --cmd reconst-prep \
        --bash <($out/bin/reconst-prep completions bash) \
        --fish <($out/bin/reconst-prep completions fish) \
        --zsh  <($out/bin/reconst-prep completions zsh)
    '';

    postFixup = ''
      wrapProgram $out/bin/reconst-prep \
        --prefix PATH : ${lib.makeBinPath [ ffmpeg ]} \
        --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath guiLibs} \
        --prefix XDG_DATA_DIRS : ${schemaDirs}
    '';

    nativeInstallCheckInputs = [ versionCheckHook ];
    versionCheckProgramArg = "--version";
    doInstallCheck = true;

    passthru = { inherit clippy craneArgs; };

    meta = {
      description = "Easy to use tool for preparing video footage for 3D reconstruction as an image dataset.";
      homepage = "https://github.com/BatteredBunny/reconst-prep";
      license = lib.licenses.gpl3Plus;
      mainProgram = "reconst-prep";
      maintainers = with lib.maintainers; [ BatteredBunny ];
      platforms = lib.platforms.linux;
    };
  }
)
