#!/bin/sh
# Inside the existing offline Linux validation/build image.
set -eu
cd /workspace
export CARGO_HOME=/workspace/target/linux-cargo
export CARGO_TARGET_DIR=/workspace/target/legacy-glow-linux
export CARGO_NET_OFFLINE=true
export LIBGL_ALWAYS_SOFTWARE=1
export MESA_GL_VERSION_OVERRIDE="${1:-1.1}"
export DISPLAY=:98
if [ "$MESA_GL_VERSION_OVERRIDE" = 1.1 ]; then
    export MESA_EXTENSION_OVERRIDE="${MESA_EXTENSION_OVERRIDE:--GL_ARB_texture_non_power_of_two}"
fi
cargo build --manifest-path scripts/validation-legacy-glow/Cargo.toml --offline
Xvfb :98 -screen 0 1984x1200x24 -nolisten tcp -extension MIT-SHM >/tmp/legacy-glow-xvfb.log 2>&1 &
xvfb_pid=$!
trap 'kill "$xvfb_pid" 2>/dev/null || true' EXIT HUP INT TERM
attempt=0
while [ ! -S /tmp/.X11-unix/X98 ]; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 50 ] || { cat /tmp/legacy-glow-xvfb.log; exit 1; }
    sleep 0.1
done
/workspace/target/legacy-glow-linux/debug/validate-legacy-glow
