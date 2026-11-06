# Application icons

`lansender.svg` and `lanreceiver.svg` are the shared vector sources. The design
follows the user's reference: a pale ivory rounded background, two overlapping
rounded square frames, a pale sage green front frame, and thin dark green lines.
LanSender uses an arrow pointing to the upper right to represent sending;
LanReceiver uses an arrow pointing to the lower left to represent receiving.
Both applications use the same restrained palette and shape, with no text or
additional badges.

Generated assets:

- `*.png`: transparent 256 × 256 images for application window icons.
- `*-1024.png`: transparent 1024 × 1024 images.
- `*.ico`: Windows resources with 16, 24, 32, 48, 64, 128, and 256 pixel sizes.
- `*.icns`: macOS resources covering 16 through 1024 pixels, including Retina variants.
- `icons-preview.png`: both icons at 256, 128, 64, 48, 32, and 16 pixels on light and dark backgrounds.
- `packaging/linux/*.svg`: copies of the same vector sources for Linux desktop integration.

Regenerate with Python 3 and Pillow, plus either CairoSVG or Node.js with `sharp`.
The generator never downloads dependencies. ICNS resources use the standard
PNG-backed format with explicit small and Retina sizes, and can be regenerated
on any platform without macOS-specific tools.

```sh
python3 scripts/generate-icons.py
```

For an explicitly configured Node.js installation:

```sh
python3 scripts/generate-icons.py --renderer sharp \
  --node /path/to/node --node-modules /path/to/node_modules
```

The generator also detects Codex's bundled local `sharp` runtime when available.
All generated resources are committed assets; building the applications does
not require Python, Node.js, or an SVG renderer.
