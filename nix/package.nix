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
  # False builds the command line alone: no `gui` subcommand and no windowing libraries.
  withGui ? true,
}:

let
  ffmpeg = ffmpeg-headless;

  # GPU undistortion lives in the core, so the command line needs these too.
  gpuLibs = [
    libGL
    vulkan-loader
  ];

  guiLibs = gpuLibs ++ [
    libxkbcommon
    wayland
    libx11
    libxcursor
    libxi
    libxrandr
  ];
  runtimeLibs = if withGui then guiLibs else gpuLibs;

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
    pname = if withGui then "reconst-prep" else "reconst-prep-cli";
    version = "0.2.0";
    strictDeps = true;
    GYROFLOW_SKIP_LENS_PROFILE_DOWNLOAD = "1";
    # Stylo generates Rust CSS property definitions with its vendored Python modules.
    nativeBuildInputs = [
      pkg-config
      python3
    ];
    buildInputs = runtimeLibs ++ lib.optional withGui gtk3;
  }
  // lib.optionalAttrs (!withGui) {
    cargoExtraArgs = "--locked -p reconst-prep --no-default-features";
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

    # The GUI crate's tests need its libraries, which the command line build leaves out.
    cargoTestExtraArgs = if withGui then "--workspace" else "-p reconst-prep-core";

    postInstall = ''
      rm -rf $out/lib
    ''
    + lib.optionalString withGui ''
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
        --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath runtimeLibs} \
        ${lib.optionalString withGui "--prefix XDG_DATA_DIRS : ${schemaDirs}"}
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
