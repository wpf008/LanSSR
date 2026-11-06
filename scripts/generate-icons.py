#!/usr/bin/env python3
"""Render the shared SVG application icons without network or build-time tools.

Requires Python 3 + Pillow and either CairoSVG, or Node.js + sharp. ICNS uses
the standard PNG-backed resource format. All paths can be configured explicitly,
so asset regeneration is independent of Codex.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

from PIL import Image, ImageDraw, ImageFont


ROOT = Path(__file__).resolve().parent.parent
ICON_DIR = ROOT / "assets" / "icons"
APPLICATIONS = ("lansender", "lanreceiver")
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)
PNG_SIZES = (16, 24, 32, 48, 64, 128, 256, 512, 1024)
BUNDLED_RUNTIME = Path.home() / ".cache/codex-runtimes/codex-primary-runtime/dependencies"


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--node", help="Node executable for the sharp renderer")
    parser.add_argument("--node-modules", help="Directory containing the sharp module")
    parser.add_argument("--renderer", choices=("auto", "cairosvg", "sharp"), default="auto")
    return parser.parse_args()


def renderer(args: argparse.Namespace):
    if args.renderer != "sharp" and importlib.util.find_spec("cairosvg"):
        import cairosvg

        def render_cairo(source: Path, target: Path, size: int) -> None:
            cairosvg.svg2png(url=str(source), write_to=str(target), output_width=size, output_height=size)

        return render_cairo
    if args.renderer == "cairosvg":
        raise SystemExit("CairoSVG is not installed; select --renderer sharp or install CairoSVG.")

    bundled_node = BUNDLED_RUNTIME / "node/bin/node"
    node = args.node or shutil.which("node") or (str(bundled_node) if bundled_node.is_file() else None)
    bundled_modules = BUNDLED_RUNTIME / "node/node_modules"
    module_dir = args.node_modules or os.environ.get("NODE_PATH")
    if not module_dir and (bundled_modules / "sharp").is_dir():
        module_dir = str(bundled_modules)
    if not node:
        raise SystemExit("An SVG renderer is required: CairoSVG, or Node.js + sharp.")

    sharp_script = """
const sharp = require(process.argv[1]);
sharp(process.argv[2], { density: 96 })
  .resize(Number(process.argv[4]), Number(process.argv[4]))
  .png({ compressionLevel: 9, palette: false })
  .toFile(process.argv[3])
  .catch(error => { console.error(error); process.exit(1); });
"""
    sharp_module = str(Path(module_dir) / "sharp") if module_dir else "sharp"

    def render_sharp(source: Path, target: Path, size: int) -> None:
        subprocess.run([node, "-e", sharp_script, sharp_module, str(source), str(target), str(size)], check=True)

    return render_sharp


def write_icns(name: str, rendered: dict[int, Path]) -> None:
    # The resource type identifies the size and scale, including Retina entries.
    # PNG-backed ICNS is supported on the application's minimum macOS version.
    resources = ((b"icp4", 16), (b"icp5", 32), (b"icp6", 64),
                 (b"ic07", 128), (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024),
                 (b"ic11", 32), (b"ic12", 64), (b"ic13", 256), (b"ic14", 512))
    chunks = []
    for resource_type, size in resources:
        png = rendered[size].read_bytes()
        chunks.append(resource_type + struct.pack(">I", len(png) + 8) + png)
    payload = b"".join(chunks)
    (ICON_DIR / f"{name}.icns").write_bytes(b"icns" + struct.pack(">I", len(payload) + 8) + payload)


def font(size: int, bold: bool = False):
    choices = (
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf" if bold else "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf" if bold else "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    )
    for choice in choices:
        if Path(choice).is_file():
            return ImageFont.truetype(choice, size=size)
    return ImageFont.load_default(size=size)


def preview(all_images: dict[str, dict[int, Path]]) -> None:
    canvas = Image.new("RGB", (1400, 850), "#EEF1F6")
    draw = ImageDraw.Draw(canvas)
    draw.text((36, 24), "LanBridge application icons", fill="#182235", font=font(30, True))
    draw.text((36, 65), "Two overlapping frames, pale sage green, and a fine directional arrow", fill="#5F6C80", font=font(18))
    for index, name in enumerate(APPLICATIONS):
        origin = 28 + index * 684
        label = "LanSender" if name == "lansender" else "LanReceiver"
        draw.text((origin + 14, 111), label, fill="#182235", font=font(26, True))
        for row, background in enumerate(("#FFFFFF", "#172232")):
            top = 155 + row * 338
            draw.rounded_rectangle((origin, top, origin + 660, top + 320), radius=18, fill=background)
            foreground = "#5F6C80" if row == 0 else "#A9B5C8"
            draw.text((origin + 18, top + 14), "Light" if row == 0 else "Dark", fill=foreground, font=font(14))
            for size, offset in zip((256, 128, 64, 48, 32, 16), (155, 357, 463, 529, 579, 613)):
                center = origin + offset
                with Image.open(all_images[name][size]) as image:
                    canvas.paste(image, (center - size // 2, top + 156 - size // 2), image)
                draw.text((center, top + 302), f"{size}", anchor="mm", fill=foreground, font=font(15))
    canvas.save(ICON_DIR / "icons-preview.png", optimize=True)


def main() -> None:
    render = renderer(arguments())
    manifest = {}
    with tempfile.TemporaryDirectory(prefix="lanbridge-icons-") as temp:
        temporary = Path(temp)
        all_images = {}
        for name in APPLICATIONS:
            source = ICON_DIR / f"{name}.svg"
            rendered = {}
            for size in PNG_SIZES:
                rendered[size] = temporary / f"{name}-{size}.png"
                render(source, rendered[size], size)
            shutil.copyfile(rendered[256], ICON_DIR / f"{name}.png")
            shutil.copyfile(rendered[1024], ICON_DIR / f"{name}-1024.png")
            ico_images = []
            for size in ICO_SIZES[:-1]:
                with Image.open(rendered[size]) as image:
                    ico_images.append(image.copy())
            with Image.open(rendered[256]) as image:
                image.save(ICON_DIR / f"{name}.ico", format="ICO", sizes=[(size, size) for size in ICO_SIZES],
                           append_images=ico_images)
            write_icns(name, rendered)
            shutil.copyfile(source, ROOT / "packaging/linux" / f"{name}.svg")
            all_images[name] = rendered
            manifest[name] = {"png": [256, 1024], "ico": list(ICO_SIZES), "icns": [16, 32, 64, 128, 256, 512, 1024]}
        preview(all_images)
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
