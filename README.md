<p align="center">
  <img src="assets/icon.svg" width="128" alt="reconst-prep icon">
</p>

# reconst-prep

TODO: screenshots here

Easy to use tool for preparing video footage for 3D reconstruction as an image dataset.

Works with drone footage, phone footage and anything else that might need clean up.

- Undistort fisheye lens footage
- Mask out people and sky
- Filter out blurry frames
- Downscale footage, for cases in which too high resolution might take more vram than you have.
- Split video into frames based on movement or every Nth frame.

# Usage

# GUI

TODO: showcase the various option panels

# CLI

TODO: showcase common usages


# Installation

# Nix/NixOS

TODO: detail nix run from repo, and using flake as input + overlay and installing package

# Other Linux distros

TODO: detail installing appimage from latest release

# Windows

TODO: detail installing the windows release

## Development

The recommended way is to use the nix dev shell which will provide you with all the dependencies.
It can be enabled by running `nix develop` (or `direnv allow` to apply it automatically).

Have a look at the steps in the justfile.