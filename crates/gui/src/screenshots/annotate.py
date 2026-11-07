"""Restore the README callouts/crops on a clean native capture.

Invoked by screenshots.rs with laid-out DOM bounds; also usable on saved clean/ PNG+JSON
pairs. All positions are logical pixels in the controlled 1280x720 window. ImageMagick
paints at the capture's native scale, so HiDPI text/arrows stay sharp. No image upscaling.
"""
import json
import math
import subprocess
import sys
from pathlib import Path

scene, source, output, metadata, font = sys.argv[1:]
bounds = json.loads(Path(metadata).read_text())
width, height = map(int, subprocess.check_output(
    ["magick", "identify", "-format", "%w %h", source], text=True).split())
scale = width / 1280
assert scale in (1, 2) and height == 720 * scale, "capture must be a 1280x720 logical window"


def target(name, fraction=None):
    assert name in bounds, f"{scene}: missing or hidden screenshot target {name!r}"
    x, y, w, h = bounds[name]
    assert 0 <= x < 1280 and 0 <= y < 720, f"{scene}: target {name!r} is outside the window"
    if fraction:
        return (x + w * fraction[0], y + h * fraction[1], 0, 0)
    return (x, y, w, h)


# Original explanatory text and directions; the current DOM supplies every arrow's target.
recipes = {
    "main": ([], [
        ("Switch videos; drop more in anywhere", "videos", "below", 70, None),
        ("Drag to pan, scroll to zoom", "preview", "right", 60, (.5, .5)),
        ("Step one frame", "previous", "above", 50, None),
        ("Scrub through the clip", "timeline", "above", 50, (.3, 0)),
        ("Flip to the untouched frame", "compare", "below", 110, None),
        ("Start", "process", "above", 40, None),
    ]),
    "lens": (["tabs", "lens"], [
        ("Profile from Gyroflow's database or a file", "profile", "left", 40, None),
        ("Choose the resampling quality", "interpolation-row", "left", 40, None),
    ]),
    "masking": (["tabs", "masking"], [
        ("Sky detection, tuned in place", "sky-row", "left", 40, None),
        ("People masking uses a segmentation model", "model", "left", 40, None),
        ("Masked sky, tinted", "preview", "below", 60, (.6, .1)),
    ]),
    "selection": (["tabs", "selection", "blur-group"], [
        ("Keep a frame once the camera moved enough", "movement", "left", 40, (0, .3)),
        ("Pick the sharpest nearby frame", "window", "left", 40, (0, .3)),
        ("Drop blurry frames", "blur-group", "left", 40, (0, .12)),
    ]),
    "processing": (["dock"], [
        ("Cancel any time, written frames stay", "cancel", "above", 30, None),
        ("Processing progress and time remaining", "status", "above", 30, None),
    ]),
}
if scene not in recipes:
    # Collage-only scenes (the overview's Output window) have no README image of their own.
    print(f"{scene}: no callouts; clean capture only")
    sys.exit(0)
keep, callouts = recipes[scene]
areas = [target(name) for name in keep]
draw = [f"scale {scale},{scale}"]
red = "#ff4d4d"


def quote(value):
    # MVG accepts apostrophes in double-quoted text (Gyroflow's database).
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'



for text, name, side, gap, fraction in callouts:
    rect = target(name, fraction)
    x, y, w, h = rect
    tw, th = map(int, subprocess.check_output(
        ["magick", "-font", font, "-pointsize", "13", f"label:{text}",
         "-format", "%w %h", "info:"], text=True).split())
    lw, lh = tw + 14, th + 10
    if side == "right":
        tip = (x + w, y + h / 2)
        left, top = tip[0] + gap, tip[1] - lh / 2
    elif side == "left":
        # Labels for inspector controls sit over the viewer so they never cover other settings.
        tip = (x, y + h / 2)
        edge = min(tip[0] - gap, bounds["inspector"][0] - 24) if "inspector" in bounds else tip[0] - gap
        left, top = edge - lw, tip[1] - lh / 2
    elif side == "above":
        tip = (x + w / 2, y)
        left, top = tip[0] - lw / 2, tip[1] - gap - lh
    else:
        tip = (x + w / 2, y + h)
        left, top = tip[0] - lw / 2, tip[1] + gap
    # Keep labels inside the capture even when a target sits near the window edge.
    left, top = max(14, min(1266 - lw, left)), max(14, min(706 - lh, top))
    start = ((left, top + lh / 2) if side == "right" else
             (left + lw, top + lh / 2) if side == "left" else
             (left + lw / 2, top + lh) if side == "above" else (left + lw / 2, top))
    dx, dy = tip[0] - start[0], tip[1] - start[1]
    length = math.hypot(dx, dy)
    assert length > 9, f"{scene}: label overlaps its arrow target"
    ux, uy = dx / length, dy / length
    base = (tip[0] - ux * 9, tip[1] - uy * 9)
    a = (base[0] - uy * 4.05, base[1] + ux * 4.05)
    b = (base[0] + uy * 4.05, base[1] - ux * 4.05)
    draw += [
        f"stroke '{red}' stroke-width 2 line {start[0]},{start[1]} {base[0]},{base[1]}",
        f"stroke none fill '{red}' polygon {tip[0]},{tip[1]} {a[0]},{a[1]} {b[0]},{b[1]}",
        f"stroke '{red}' stroke-width 1 fill 'rgba(16,17,20,0.9)' roundrectangle {left},{top} {left+lw},{top+lh} 7,7",
        f"stroke none fill '{red}' font {quote(font)} font-size 13 text {left+7},{top+5+th-4} {quote(text)}",
    ]
    areas += [rect, (left, top, lw, lh)]

command = ["magick", source, "-draw", " ".join(draw)]
if scene != "main":
    x0 = max(0, math.floor(min(r[0] for r in areas) - 14))
    y0 = max(0, math.floor(min(r[1] for r in areas) - 14))
    x1 = min(1280, math.ceil(max(r[0] + r[2] for r in areas) + 14))
    y1 = min(720, math.ceil(max(r[1] + r[3] for r in areas) + 14))
    command += ["-crop", f"{round((x1-x0)*scale)}x{round((y1-y0)*scale)}+{round(x0*scale)}+{round(y0*scale)}", "+repage"]
command += [output]
subprocess.run(command, check=True)
print(f"{scene}: annotated {len(callouts)} DOM targets → {output}")
