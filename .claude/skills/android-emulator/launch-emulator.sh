#!/usr/bin/env bash
# Launch the Android emulator for wezterm android-port work.
#
# Two things have to be got right or the emulator is useless here:
#
#  - A display. Claude Code and most ssh/tmux shells start with no DISPLAY and
#    XDG_SESSION_TYPE=tty. The emulator's bundled Qt has no wayland plugin
#    (vnc, offscreen, xcb, minimal, linuxfb only), so it needs Xwayland, and
#    mutter's Xwayland only accepts a private cookie whose filename is
#    regenerated on every login.
#
#  - A real GPU. Left to itself the emulator decides "your GPU drivers may have
#    a bug" and falls back to swANGLE, which exposes GLSL ES 1.00. wezterm's
#    lowest shader is 300 es, so it panics with "No OpenGL" before drawing.
#
# Usage: launch-emulator.sh [-avd NAME] [extra emulator args...]

set -euo pipefail

AVD="${WEZTERM_AVD:-reverse_eng}"
SDK="${ANDROID_SDK_ROOT:-$HOME/Application/Android_SDK}"
SERIAL="${WEZTERM_EMU_SERIAL:-emulator-5554}"

if [ "${1:-}" = "-avd" ]; then
    AVD="$2"
    shift 2
fi

EMULATOR="$SDK/emulator/emulator"
[ -x "$EMULATOR" ] || { echo "no emulator at $EMULATOR" >&2; exit 1; }

# --- display -------------------------------------------------------------
#
# Prefer whatever the caller already has, but only if it actually answers.
usable_display() {
    DISPLAY="$1" XAUTHORITY="${2:-${XAUTHORITY:-}}" timeout 5 xdpyinfo >/dev/null 2>&1
}

if [ -n "${DISPLAY:-}" ] && usable_display "$DISPLAY"; then
    : # inherited environment is fine
else
    # Recover the display number and cookie from the running Xwayland.
    xwl=$(pgrep -a Xwayland | head -1 || true)
    [ -n "$xwl" ] || {
        echo "no Xwayland running; is a graphical session logged in?" >&2
        exit 1
    }

    disp=$(printf '%s\n' "$xwl" | grep -oE ' :[0-9]+' | head -1 | tr -d ' ')
    auth=$(printf '%s\n' "$xwl" | grep -oP '(?<=-auth )\S+' | head -1)

    [ -n "$disp" ] || { echo "could not parse a display from: $xwl" >&2; exit 1; }

    export DISPLAY="$disp"
    [ -n "$auth" ] && export XAUTHORITY="$auth"

    usable_display "$DISPLAY" "${XAUTHORITY:-}" || {
        echo "cannot open $DISPLAY with XAUTHORITY=${XAUTHORITY:-unset}" >&2
        exit 1
    }
fi

echo "display: $DISPLAY (XAUTHORITY=${XAUTHORITY:-unset})"

# --- launch --------------------------------------------------------------
#
# -gpu host is not optional; see the header.
"$EMULATOR" -avd "$AVD" -no-snapshot-save -gpu host "$@" &
emu_pid=$!

echo "emulator pid $emu_pid, waiting for boot..."

adb wait-for-device
until [ "$(adb -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ]; do
    kill -0 "$emu_pid" 2>/dev/null || { echo "emulator died during boot" >&2; exit 1; }
    sleep 3
done

echo "booted: $SERIAL"
adb -s "$SERIAL" shell 'getprop ro.product.cpu.abilist; getprop ro.build.version.sdk'
