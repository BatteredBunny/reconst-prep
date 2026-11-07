# Building from source

reconst-prep is a Rust project with one binary which includes both the CLI and the GUI.

- [What you need](#what-you-need)
- [Ubuntu](#ubuntu)
- [Windows](#windows)
- [NixOS / Nix](#nixos--nix)

## Dependencies

| | Build  |
|---|---|
| Rust | stable, pinned in `rust-toolchain.toml` |
| C toolchain | gcc/clang on Linux, MSVC build tools on Windows |
| Python 3 | Required by Blitz/Stylo |
| GTK 3 headers | Linux only, for the file dialogs |

## Ubuntu


```bash
# Install build dependencies: build-essential, pkg-config, curl, libgtk-3-dev, rustup, ffmpeg
./scripts/install-deps-ubuntu.sh --with-ffmpeg

# Build the appimage
./packaging/appimage/build.sh

# Ta-da!
# You will find your appimage at packaging/appimage/reconst-prep-<version>-<arch>.AppImage
```

## Windows

You need the MSVC toolchain (`x86_64-pc-windows-msvc`), not the GNU one. The build links the C runtime
statically (`.cargo/config.toml`), so the binary runs without the Visual C++ redistributable.

In PowerShell, from the repository root:

```powershell
# Install dependencies: rustup, MSVC build tools, Python 3, .NET SDK
powershell -ExecutionPolicy Bypass -File scripts\install-deps-windows.ps1 -InstallPrerequisites

# Build the installer
.\packaging\windows\build-velopack.ps1

# You will find the installer at packaging\windows\releases\ReconstPrep-win-Setup.exe
```

## NixOS / Nix

```bash
nix build                    # or: just nix-build; result/bin/reconst-prep, wrapped with ffmpeg on PATH
nix run . -- gui             # build and start the GUI
```