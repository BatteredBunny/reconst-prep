{
  lib,
  writeShellApplication,
  reconst-prep,
  coreutils,
  ffmpeg-headless,
  imagemagick,
  librsvg,
  oxipng,
  xvfb,
  mesa,
  lexend,
}:
let
  # The app renders its 1280x720 point layout at 2x, and every size below is in points.
  # Layers are only ever scaled down from the 2x shots, so text stays sharp.
  zoom = 2;
  px = n: toString (n * zoom);
  window = {
    w = 1280;
    h = 720;
  };

  demo = ../assets/demo;

  # Software GL from the store, so the shots do not depend on the host's graphics driver.
  environment = {
    XDG_CONFIG_HOME = "$work/config";
    XDG_STATE_HOME = "$work/state";
    XDG_DATA_HOME = "$work/data";
    LIBGL_ALWAYS_SOFTWARE = "1";
    __EGL_VENDOR_LIBRARY_FILENAMES = "${mesa}/share/glvnd/egl_vendor.d/50_mesa.json";
    __GLX_VENDOR_LIBRARY_NAME = "mesa";
    LD_LIBRARY_PATH = "${mesa}/lib";
    LIBGL_DRIVERS_PATH = "${mesa}/lib/dri";
    # Vulkan would pick the host GPU, which cannot present to Xvfb.
    WGPU_BACKEND = "gl";
    LC_ALL = "C.UTF-8";
  };

  font = "${lexend}/share/fonts/truetype/lexend/lexend/Lexend-Bold.ttf";

  # Each panel shot is cropped to the column that tells it apart: every window shows the same preview.
  collage = {
    canvas = {
      w = 2100;
      h = 1150;
    };
    width = 1600;
    layers = [
      {
        shot = "lens";
        crop = {
          w = 388;
          h = 385;
        };
        rotate = -5;
        x = 150;
        y = 150;
      }
      {
        shot = "masking";
        crop = {
          w = 388;
          h = 275;
          y = 95;
        };
        rotate = 4;
        x = 170;
        y = 600;
      }
      {
        shot = "main";
        scale = 80;
        rotate = -1;
        x = 560;
        y = 220;
      }
      {
        shot = "selection";
        crop = {
          w = 388;
          h = 410;
          y = 125;
        };
        rotate = 5;
        x = 1500;
        y = 190;
      }
      {
        shot = "processing";
        crop = {
          w = 1280;
          h = 52;
          y = 668;
        };
        scale = 82;
        rotate = -2;
        x = 560;
        y = 790;
      }
    ];

    logo = {
      icon = 120;
      gap = 24;
      pointsize = 80;
      colour = "#ffffff";
      outline = "#000000";
      outlineWidth = 6;
      centreX = 1070;
      centreY = 110;
    };
  };

  layer =
    {
      shot,
      x,
      y,
      rotate,
      scale ? 100,
      crop ? window,
    }:
    let
      c = {
        x = 0;
        y = 0;
      }
      // crop;
    in
    assert lib.assertMsg (scale <= 100) "${shot}: scaling a shot up would blur it";
    ''\( "$work/shots/${shot}.png" -crop ${px c.w}x${px c.h}+${px c.x}+${px c.y} +repage -resize ${toString (c.w * scale * zoom / 100)}x -bordercolor "#2a2d32" -border ${px 1} -background none -rotate ${toString rotate} +repage \) -geometry +${px x}+${px y} -composite'';

  inherit (collage) logo;

  callout_style = {
    colour = "#ff4d4d";
    pointsize = 13;
    padding = 6;
    stroke = 2;
    head = 10;
    radius = 7;
  };

  # Red labels with arrows on the README's shots; the collage keeps the clean ones.
  # A feature shot is cropped to what its text talks about, so it is clear where to look; the overview stays whole.
  # `at` is a label's top-left and `tip` where its arrow points, both in points of the full window.
  callouts = {
    main = {
      crop = window;
      marks = [
        {
          text = "Drop videos here";
          at = [ 470 130 ];
          tip = [ 382 152 ];
        }
        {
          text = "Drag to compare before and after";
          at = [ 830 380 ];
          tip = [ 771 345 ];
        }
        {
          text = "Step one frame";
          at = [ 880 500 ];
          tip = [ 1060 566 ];
        }
        {
          text = "Scrub through the clip";
          at = [ 680 652 ];
          tip = [ 622 640 ];
        }
        {
          text = "Start";
          at = [ 215 676 ];
          tip = [ 168 690 ];
        }
      ];
    };
    lens = {
      crop = {
        x = 0;
        y = 170;
        w = 780;
        h = 215;
      };
      marks = [
        {
          text = "Profile from a file or Gyroflow's database";
          at = [ 470 245 ];
          tip = [ 281 259 ];
        }
        {
          text = "Undistort on the graphics card";
          at = [ 470 325 ];
          tip = [ 181 339 ];
        }
      ];
    };
    masking = {
      crop = {
        x = 0;
        y = 95;
        w = 820;
        h = 285;
      };
      marks = [
        {
          text = "Tune sky detection";
          at = [ 470 195 ];
          tip = [ 312 210 ];
        }
        {
          text = "People masking uses a segmentation model";
          at = [ 470 280 ];
          tip = [ 236 295 ];
        }
        {
          text = "Masked sky, tinted";
          at = [ 640 105 ];
          tip = [ 760 165 ];
        }
      ];
    };
    selection = {
      crop = {
        x = 0;
        y = 125;
        w = 810;
        h = 410;
      };
      marks = [
        {
          text = "Keep a frame once the camera moved enough";
          at = [ 470 185 ];
          tip = [ 364 200 ];
        }
        {
          text = "Drop blurry frames";
          at = [ 470 305 ];
          tip = [ 340 320 ];
        }
        {
          text = "Output size and format";
          at = [ 470 455 ];
          tip = [ 180 481 ];
        }
      ];
    };
    processing = {
      crop = {
        x = 0;
        y = 580;
        w = 1280;
        h = 140;
      };
      marks = [
        {
          text = "Cancel any time, written frames stay";
          at = [ 440 590 ];
          tip = [ 325 642 ];
        }
        {
          text = "Frames kept, decoded and time left";
          at = [ 760 600 ];
          tip = [ 560 676 ];
        }
      ];
    };
  };

  annotate =
    shot:
    { crop, marks }:
    let
      c = {
        x = 0;
        y = 0;
      }
      // crop;
      mark =
        {
          text,
          at,
          tip,
        }:
        ''callout "$out/${shot}.png" ${px (builtins.elemAt at 0)} ${px (builtins.elemAt at 1)} ${px (builtins.elemAt tip 0)} ${px (builtins.elemAt tip 1)} ${lib.escapeShellArg text}'';
    in
    map mark marks
    ++ [
      ''magick "$out/${shot}.png" -crop ${px c.w}x${px c.h}+${px c.x}+${px c.y} +repage "$out/${shot}.png"''
    ];
in
writeShellApplication {
  name = "reconst-prep-screenshots";

  runtimeInputs = [
    coreutils
    ffmpeg-headless
    imagemagick
    librsvg
    oxipng
    xvfb
  ];

  text = ''
    usage() {
      echo "usage: reconst-prep-screenshots [--out DIR]" >&2
      exit 2
    }

    out="$PWD/assets/screenshots"
    while [ $# -gt 0 ]; do
      case "$1" in
        --out) out=''${2:?}; shift 2 ;;
        *) usage ;;
      esac
    done

    work=$(mktemp -d)
    xvfb_pid=""
    cleanup() {
      if [ -n "$xvfb_pid" ]; then
        kill "$xvfb_pid" 2>/dev/null || true
      fi
      rm -rf "$work"
    }
    trap cleanup EXIT

    mkdir -p "$out" "$work"/{shots,config,state,data,dataset}
    ${lib.concatStringsSep "\n    " (
      lib.mapAttrsToList (name: value: ''export ${name}="${value}"'') environment
    )}

    # Xvfb rather than a Wayland compositor: one less moving part for an X11-capable app.
    exec 3>"$work/display"
    Xvfb -displayfd 3 -screen 0 ${px window.w}x${px window.h}x24 -nolisten tcp >"$work/xvfb.log" 2>&1 &
    xvfb_pid=$!
    exec 3>&-
    for _ in $(seq 100); do
      [ -s "$work/display" ] && break
      sleep 0.1
    done
    if [ ! -s "$work/display" ]; then
      echo "screenshots: Xvfb did not start:" >&2
      cat "$work/xvfb.log" >&2
      exit 1
    fi
    DISPLAY=":$(head -n1 "$work/display")"
    export DISPLAY
    unset WAYLAND_DISPLAY

    # As long as the real clip, so the scrubber shows its per-second ticks; a short clip lands them all on the labelled ones.
    # One encoded second, looped by stream copy: encoding the whole length at 4K takes minutes.
    ffmpeg -loglevel error -loop 1 -framerate 59.94 -t 1 -i ${demo}/frame.jpg \
      -c:v libx264 -preset ultrafast -crf 12 -pix_fmt yuv420p "$work/second.mp4"
    ffmpeg -loglevel error -stream_loop -1 -i "$work/second.mp4" -t 115.7 -c copy "$work/DJI_0021.mp4"

    RECONST_PREP_SCREENSHOTS="$work/shots" ${lib.getExe reconst-prep} gui "$work/DJI_0021.mp4" \
      --profile ${demo}/lens-profile.json --out "$work/dataset"

    cp "$work"/shots/*.png "$out"/

    # The arrow leaves the middle of the label's side that faces its tip, and stops at the arrowhead's base so the line does not poke through the point.
    callout() {
      local img=$1 x=$2 y=$3 tx=$4 ty=$5 text=$6 w h sx sy
      magick -background none -fill "${callout_style.colour}" -font "${font}" \
        -pointsize ${px callout_style.pointsize} label:"$text" "$work/callout-text.png"
      read -r w h < <(magick identify -format '%w %h\n' "$work/callout-text.png")
      w=$(( w + 2 * ${px callout_style.padding} )); h=$(( h + 2 * ${px callout_style.padding} ))
      magick -size "''${w}x''${h}" xc:none -fill "rgba(16,17,20,0.9)" \
        -stroke "${callout_style.colour}" -strokewidth ${px 1} \
        -draw "roundrectangle 1,1 $(( w - 2 )),$(( h - 2 )) ${px callout_style.radius},${px callout_style.radius}" \
        "$work/callout-text.png" -gravity center -composite "$work/label.png"
      if [ "$tx" -lt "$x" ]; then sx=$x; sy=$(( y + h / 2 ))
      elif [ "$tx" -gt $(( x + w )) ]; then sx=$(( x + w )); sy=$(( y + h / 2 ))
      elif [ "$ty" -lt "$y" ]; then sx=$(( x + w / 2 )); sy=$y
      else sx=$(( x + w / 2 )); sy=$(( y + h )); fi
      read -r base head < <(awk -v sx="$sx" -v sy="$sy" -v tx="$tx" -v ty="$ty" -v l=${px callout_style.head} 'BEGIN {
        a = atan2(ty - sy, tx - sx)
        printf "%d,%d %d,%d %d,%d %d,%d\n", tx - l * cos(a), ty - l * sin(a),
          tx, ty, tx - l * cos(a + 0.45), ty - l * sin(a + 0.45), tx - l * cos(a - 0.45), ty - l * sin(a - 0.45)
      }' | awk '{ print $1, $2" "$3" "$4 }')
      magick "$img" -stroke "${callout_style.colour}" -strokewidth ${px callout_style.stroke} -fill none \
        -draw "line $sx,$sy $base" -stroke none -fill "${callout_style.colour}" -draw "polygon $head" \
        "$work/label.png" -geometry "+$x+$y" -composite "$img"
    }
    ${lib.concatStringsSep "\n    " (
      lib.concatLists (lib.mapAttrsToList annotate callouts)
    )}

    rsvg-convert -w ${px logo.icon} -h ${px logo.icon} \
      ${reconst-prep}/share/icons/hicolor/scalable/apps/reconst-prep.svg -o "$work/icon.png"

    magick -background none -font "${font}" \
      -pointsize ${px logo.pointsize} \
      -fill "${logo.colour}" label:"reconst-prep" "$work/text-fill.png"
    magick "$work/text-fill.png" \
      -bordercolor none -border ${px (2 * logo.outlineWidth)} \
      \( +clone -alpha extract \
         -morphology Dilate Disk:${px logo.outlineWidth} \
         -background "${logo.outline}" -alpha shape \) \
      +swap -composite "$work/text.png"
    magick -background none "$work/icon.png" \( -size ${px logo.gap}x1 xc:none \) "$work/text.png" \
      -gravity center +append "$work/logo.png"

    logo_x=$(( ${px logo.centreX} - $(magick identify -format %w "$work/logo.png") / 2 ))
    logo_y=$(( ${px logo.centreY} - $(magick identify -format %h "$work/logo.png") / 2 ))

    magick -background none -size ${px collage.canvas.w}x${px collage.canvas.h} xc:none \
      ${lib.concatMapStringsSep " \\\n      " layer collage.layers} \
      "$work/logo.png" -geometry "+$logo_x+$logo_y" -composite \
      -trim +repage \
      -resize ${px collage.width}x \
      -depth 8 -strip \
      "$out/overview.png"

    # The preview is a photo, so these stay large; this still takes a sixth off.
    oxipng -o 3 --strip safe -q "$out"/*.png

    echo "screenshots: wrote $out"
  '';

  meta.mainProgram = "reconst-prep-screenshots";
}
