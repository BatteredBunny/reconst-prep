{
  lib,
  stdenv,
  rustPlatform,
  pkg-config,
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
in
rustPlatform.buildRustPackage {
  pname = "reconst-prep";
  version = "0.1.0";

  src = ./.;

  cargoDeps = rustPlatform.importCargoLock {
    lockFile = ../Cargo.lock;
    allowBuiltinFetchGit = true;
  };

  __structuredAttrs = true;
  strictDeps = true;

  nativeBuildInputs = [
    pkg-config
    makeWrapper
    installShellFiles
  ];

  buildInputs = guiLibs ++ [ gtk3 ];

  postPatch = ''
    for crate in "$cargoDepsCopy"/gyroflow-core-*; do
      echo 'fn main() {}' > "$crate/build.rs"
    done
  '';

  doCheck = true;

  postInstall =
    ''
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

  meta = {
    description = "Easy to use tool for preparing video footage for 3D reconstruction as an image dataset.";
    homepage = "https://github.com/BatteredBunny/reconst-prep";
    license = lib.licenses.gpl3Plus;
    mainProgram = "reconst-prep";
    maintainers = with lib.maintainers; [ BatteredBunny ];
    platforms = lib.platforms.linux;
  };
}
