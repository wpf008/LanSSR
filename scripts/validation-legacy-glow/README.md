# Real-driver rendering validation

This standalone crate exercises the vendored painter using an actual Linux GLX
compatibility context and pixel readback. It is a validation tool, not part of
the installed applications. Normal builds use automatic backend selection:
OpenGL 1.1 selects the fixed-function renderer; OpenGL 2.1 selects the existing
shader renderer. The optional `force-legacy` Cargo feature is only for explicit
compatibility-context experiments.

The existing `lanbridge-kylin-arm64-builder` image contains Rust, Mesa, Xvfb,
X11/GL libraries and Noto CJK fonts. With the repository mounted at `/workspace`
and its existing offline Cargo cache, run from the host:

```sh
docker run --rm --network none \
  --mount type=bind,source="$PWD",target=/workspace \
  lanbridge-kylin-arm64-builder \
  sh /workspace/scripts/validation-legacy-glow/run.sh 1.1

docker run --rm --network none \
  --mount type=bind,source="$PWD",target=/workspace \
  lanbridge-kylin-arm64-builder \
  sh /workspace/scripts/validation-legacy-glow/run.sh 2.1
```

The harness directly imports the production `src/transfer_texture.rs` module,
without duplicating its tiling implementation. It uploads a full 1936×1088
codec image as four nearest-filtered textures with the application texture
limit constrained to 1024, then reads back and compares every GPU pixel and
decodes the reconstructed image. Both watermark settings and DPI scales 1,
1.25, 1.5 and 2 are exercised. A second changing frame must reuse the handles
and update all tiles together; an idle repaint must not upload codec pixels.
This covers seams on both axes, including the non-power-of-two right/bottom
tiles, in both renderer paths above.

The 2.0.20 validation passed all 32 complete encoded-frame readbacks (two
renderer paths × two watermark settings × four DPI scales × two changing
frames) using Mesa 20.3.5 llvmpipe in the cached ARM64 image with container
networking disabled. Every RGB pixel matched its source and every reconstructed
frame decoded successfully. Existing painter checks also passed in both paths.

Other checks include pixel-exact NPOT protocol grids with both watermark settings,
partial texture updates, premultiplied blending, scissor clipping, a 1024-wide
texture, linear `WHITE_UV` panels, top-left partial updates, actual Chinese and
Latin glyphs, a font atlas constrained to 1024 and GL error checks.

The log records the reported GL version, renderer, texture limit and relevant
extensions. Mesa may normalize bordered texture storage, so the rendered pixels
and absence of GL errors determine correctness. The driver in the current
container reports a 16384 texture limit; testing 1024-wide uploads does not
emulate a driver which actually reports a 1024 maximum. Root application tests
separately cover configuration constrained by a 1024 maximum.

This validates the shared renderer on a CPU software GL driver. It does not
replace a Windows 7 / Microsoft GDI or physical graphics-driver test. The Linux
full-application smoke attempt is recorded separately: the current container
aborts in XCB before mapping a window, including with a test-only Xlib thread
initialization preload, so no full-GUI startup claim is made from that attempt.

The default Mesa 1.1 fixture still reports `GL_EXT_texture_edge_clamp` and
`GL_SGIS_texture_edge_clamp`; it exercises automatic GL1.1 rendering with those
optional sampler capabilities. An additional diagnostic disables both:

```sh
docker run --rm --network none \
  --mount type=bind,source="$PWD",target=/workspace \
  -e 'MESA_EXTENSION_OVERRIDE=-GL_ARB_texture_non_power_of_two -GL_EXT_texture_edge_clamp -GL_SGIS_texture_edge_clamp' \
  lanbridge-kylin-arm64-builder \
  sh /workspace/scripts/validation-legacy-glow/run.sh 1.1
```

With Mesa 20.3.5 this diagnostic still strips native texture borders. A sampler
fallback for the combined no-border/no-edge-clamp/linear case now passes the
strict `WHITE_UV`, partial-update and protocol checks. The earlier failure is
retained in `dist/validation-2.0.14/opengl-1.1-no-edge-clamp-before-fallback.log`;
the final passing log is `opengl-1.1-no-edge-clamp.log` in the same directory.

The font panel captures are `font-panel-no-edge-clamp.png`, `font-panel-1.1.png`
and `font-panel-2.1.png`. The standard 1.1 and modern 2.1 captures are pixel
identical with 2020 counted ink pixels. The no-edge-clamp fallback has 2352 ink
pixels: 34 of the 68 glyph vertices touch the actual 1024×32 atlas's top UV
edge, so the half-texel fallback changes their interpolation. Chinese and Latin
remain fully readable with the same visible glyph bounds and no block artifacts;
their pixels are not identical to the reference. Across the 630×120 panel,
1840 pixels differ, maximum channel difference is 108, and mean absolute channel
difference is 0.5538. This fallback does not activate for native GL1.1 border=1.

Mesa's normalized storage in this configuration does not reproduce the native
GL1.1 bordered storage used by Microsoft GDI. Windows 7 hardware remains untested.
