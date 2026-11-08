{
  lib,
  stdenv,
  craneLib,
  reconst-prep,
  runCommand,
  fetchurl,
  ffmpeg-headless,
  mesa,
  vulkan-loader,
  oxipng,
  librsvg,
  python3,
  makeFontsConf,
  xorg-server,
  imagemagick,
}:
let
  demo = ../assets/demo;
  fonts = ../crates/gui/assets/fonts;

  # A still at the real clip's length, so the scrubber shows its per-second ticks. One two-second
  # stretch looped by stream copy: encoding two minutes of 4K takes minutes. A keyframe per stretch
  # keeps the preview's seeks short.
  clip = runCommand "reconst-prep-demo-clip" { nativeBuildInputs = [ ffmpeg-headless ]; } ''
    ffmpeg -loglevel error -loop 1 -framerate 59.94 -t 2 -i ${demo}/frame.jpg \
      -c:v libx264 -preset veryfast -tune stillimage -crf 20 -g 120 -pix_fmt yuv420p stretch.mp4
    mkdir $out
    ffmpeg -loglevel error -stream_loop -1 -i stretch.mp4 -t 115.7 -c copy $out/DJI_0021.mp4
    # The processing scene waits for a half-full progress bar; on the full clip that is minutes of
    # software 4K decoding. Same name, so the dock reads the same.
    mkdir $out/short
    ffmpeg -loglevel error -stream_loop -1 -i stretch.mp4 -t 12 -c copy $out/short/DJI_0021.mp4
  '';

  # The catalogue entry in crates/core/src/models.rs, fetched here because the sandbox has no network.
  model = fetchurl {
    url = "https://huggingface.co/Xenova/segformer-b0-finetuned-ade-512-512/resolve/main/onnx/model.onnx";
    sha256 = "3e5c18a4be395f16646438d54c42377ddc202edfa33d5eced0c9506de75c44c2";
  };

  # Each scene renders the real Blitz window in an isolated Xvfb display.
  shots = craneLib.cargoTest (
    reconst-prep.craneArgs
    // {
      pname = "reconst-prep-screenshots";
      nativeBuildInputs = reconst-prep.craneArgs.nativeBuildInputs ++ [
        ffmpeg-headless
        xorg-server
        imagemagick
      ];
      cargoTestExtraArgs = "-p reconst-prep-gui screenshots::readme -- --ignored --exact";
      preCheck = ''
        export DISPLAY=:99 WINIT_UNIX_BACKEND=x11 WINIT_X11_SCALE_FACTOR=2
        unset WAYLAND_DISPLAY
        Xvfb :99 -screen 0 2560x1440x24 -nolisten tcp &
        xvfb_pid=$!
        trap 'kill "$xvfb_pid"' EXIT
        for attempt in $(seq 1 50); do
          test -S /tmp/.X11-unix/X99 && break
          sleep 0.1
        done
        export SCREENSHOTS_SCENE=main
      '';
      postCheck = ''
        for scene in lens masking selection output processing; do
          export SCREENSHOTS_SCENE=$scene
          if [ "$scene" = processing ]; then export SCREENSHOTS_CLIP="${clip}/short/DJI_0021.mp4"; fi
          cargoWithProfile test --locked -p reconst-prep-gui screenshots::readme -- --ignored --exact
        done
      '';

      # lavapipe, Mesa's software Vulkan, so the shots come out the same on every machine.
      VK_ICD_FILENAMES = "${mesa}/share/vulkan/icd.d/lvp_icd.${stdenv.hostPlatform.uname.processor}.json";
      LD_LIBRARY_PATH = lib.makeLibraryPath (reconst-prep.craneArgs.buildInputs ++ [ vulkan-loader ]);

      FONTCONFIG_FILE = makeFontsConf { fontDirectories = [ fonts ]; };
      SCREENSHOTS_CLIP = "${clip}/DJI_0021.mp4";
      SCREENSHOTS_PROFILE = "${demo}/lens-profile.json";
      SCREENSHOTS_MODEL = model;
      preBuild = ''
        export SCREENSHOTS_OUT=$PWD/screenshots
        export XDG_CONFIG_HOME=$TMPDIR/config XDG_STATE_HOME=$TMPDIR/state XDG_CACHE_HOME=$TMPDIR/cache
      '';

      doInstallCargoArtifacts = false;
      installPhaseCommand = "cp -r $SCREENSHOTS_OUT $out";
    }
  );
in
runCommand "reconst-prep-screenshots"
  {
    nativeBuildInputs = [
      oxipng
      librsvg
      python3
    ];
    FONTCONFIG_FILE = makeFontsConf { fontDirectories = [ fonts ]; };
  }
  ''
    export XDG_CACHE_HOME=$TMPDIR
    mkdir $out
    cp ${shots}/*.png $out/

    cp -r ${shots}/clean clean
    cp ${./collage.svg} collage.svg
    cp ${../assets/icon.svg} icon.svg
    rsvg-convert --zoom 2 collage.svg -o $out/overview.png
    chmod u+w $out/*.png

    # The preview is a photo, so these stay large; this still takes a sixth off.
    oxipng -o 3 --strip safe -q $out/*.png
  ''
