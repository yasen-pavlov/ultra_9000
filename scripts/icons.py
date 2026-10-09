#!/usr/bin/env python3
"""Rebuild Ultra 9000 app/tray assets from the SVG masters in assets/icons.
Requires Python 3, rsvg-convert and ImageMagick (magick); run from any directory.
ICNS is the standard chunk container of PNG entries, so no macOS tools are needed.
"""
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "assets/icons"
OUT = ROOT / "src-tauri/icons"


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


def render(source, dest, size, color=None, dark=False, canvas=None):
    svg = source.read_text()
    if color:
        svg = svg.replace("currentColor", color)
    if dark:
        svg = svg.replace("#ff9800", "#e65100")
    with tempfile.TemporaryDirectory(prefix="u9k-icons-") as tmp:
        master = Path(tmp) / "master.svg"
        image = Path(tmp) / "image.png"
        master.write_text(svg)
        run("rsvg-convert", "-w", size, "-h", size, "-o", image, master)
        dest.parent.mkdir(parents=True, exist_ok=True)
        args = ["magick", image]
        if canvas:
            args += ["-background", "none", "-gravity", "center", "-extent", f"{canvas}x{canvas}"]
        run(*args, "-strip", f"PNG32:{dest}")


def main():
    for tool in ("rsvg-convert", "magick"):
        if not shutil.which(tool):
            raise SystemExit(f"Required tool missing: {tool}")
    for variant, color in (("light", "#eeffff"), ("dark", "#212121")):
        for attention in (False, True):
            suffix = "-attention" if attention else ""
            for size in (16, 20, 22, 24, 32, 40):
                master = 16 if size in (16, 32) else 20
                render(SOURCE / f"tray/tray-{master}{suffix}.svg",
                       OUT / f"tray/{variant}{suffix}-{size}.png",
                       master if size in (22, 24) else size,
                       color, variant == "dark", size if size in (22, 24) else None)
        # tray-icon forces 18 pt logical size; 36 px keeps the image sharp on Retina.
        render(SOURCE / "tray/tray-16-attention.svg",
               OUT / f"tray/macos-{variant}-attention-36.png",
               32, color, variant == "dark", 36)
    render(SOURCE / "tray/macos-template-18.svg", OUT / "tray/macos-template-36.png", 36)
    for size in (16, 32, 128, 256, 512):
        master = 16 if size == 16 else 32
        dest = OUT / f"hicolor/{size}x{size}/apps/ultra-9000.png"
        render(SOURCE / f"app/ultra-9000-{master}.svg", dest, size)
        if size in (32, 128):
            shutil.copyfile(dest, OUT / f"{size}x{size}.png")
        elif size == 256:
            shutil.copyfile(dest, OUT / "128x128@2x.png")
        elif size == 512:
            shutil.copyfile(dest, OUT / "icon.png")
    scalable = OUT / "hicolor/scalable/apps/ultra-9000.svg"
    scalable.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(SOURCE / "app/ultra-9000-32.svg", scalable)
    # PNG-backed ICNS types cover both standard and Retina representations.
    entries = ((b"icp4", 16, 16), (b"icp5", 32, 32), (b"icp6", 64, 1024),
               (b"ic07", 128, 1024), (b"ic08", 256, 1024), (b"ic09", 512, 1024),
               (b"ic10", 1024, 1024), (b"ic11", 32, 32), (b"ic12", 64, 1024),
               (b"ic13", 256, 1024), (b"ic14", 512, 1024))
    chunks = []
    with tempfile.TemporaryDirectory(prefix="u9k-icns-") as tmp:
        for kind, size, master in entries:
            source = "ultra-9000-macos-1024.svg" if master == 1024 else f"ultra-9000-{master}.svg"
            image = Path(tmp) / f"{kind.decode()}.png"
            render(SOURCE / "app" / source, image, size)
            data = image.read_bytes()
            chunks.append(kind + struct.pack(">I", len(data) + 8) + data)
    data = b"".join(chunks)
    (OUT / "icon.icns").write_bytes(b"icns" + struct.pack(">I", len(data) + 8) + data)
    # Obsolete 36 px >_ tray assets are intentionally removed by this cutover.
    for name in ("tray.png", "tray-attention.png", "tray-light.png", "tray-light-attention.png"):
        (OUT / name).unlink(missing_ok=True)
    # The 36 px bitmaps render at 18 pt on macOS; unused 1x images no longer ship.
    for name in ("macos-template-18.png", "macos-light-attention-18.png", "macos-dark-attention-18.png"):
        (OUT / "tray" / name).unlink(missing_ok=True)


if __name__ == "__main__":
    main()
