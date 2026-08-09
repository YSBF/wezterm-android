# Source this to configure a cross-compile environment for aarch64-linux-android.
#
#   . ci/android-env.sh
#   cargo build --target aarch64-linux-android -p wezterm-gui \
#         --no-default-features --features vendored-fonts
#
# --no-default-features matters: the default feature set enables `wayland`,
# which turns on window/wayland.
#
# Two traps worth remembering:
#  - Building in a fresh git worktree needs deps/freetype/{freetype2,libpng,zlib}
#    and deps/harfbuzz/harfbuzz seeded, otherwise deps/freetype/build.rs shells
#    out to `git submodule update --init` and hangs on credentials with no
#    output. Set GIT_TERMINAL_PROMPT=0 to at least fail fast.
#  - Do not build under a tmpfs /tmp. The target dir reaches ~7GB.

: "${ANDROID_NDK_HOME:=$HOME/Application/Android_SDK/ndk/28.0.12674087}"
: "${ANDROID_API:=28}"

NDK_BIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"

if [ ! -d "$NDK_BIN" ]; then
    echo "android-env.sh: no NDK toolchain at $NDK_BIN" >&2
    echo "set ANDROID_NDK_HOME to an installed NDK" >&2
    return 1 2>/dev/null || exit 1
fi

export ANDROID_NDK_HOME ANDROID_API
export GIT_TERMINAL_PROMPT=0

export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_BIN/aarch64-linux-android$ANDROID_API-clang"
export CC_aarch64_linux_android="$NDK_BIN/aarch64-linux-android$ANDROID_API-clang"
export CXX_aarch64_linux_android="$NDK_BIN/aarch64-linux-android$ANDROID_API-clang++"
export AR_aarch64_linux_android="$NDK_BIN/llvm-ar"
export RANLIB_aarch64_linux_android="$NDK_BIN/llvm-ranlib"
export CFLAGS_aarch64_linux_android="--target=aarch64-linux-android$ANDROID_API -fPIC"

export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$NDK_BIN/armv7a-linux-androideabi$ANDROID_API-clang"
export CC_armv7_linux_androideabi="$NDK_BIN/armv7a-linux-androideabi$ANDROID_API-clang"
export CXX_armv7_linux_androideabi="$NDK_BIN/armv7a-linux-androideabi$ANDROID_API-clang++"
export AR_armv7_linux_androideabi="$NDK_BIN/llvm-ar"
export RANLIB_armv7_linux_androideabi="$NDK_BIN/llvm-ranlib"
export CFLAGS_armv7_linux_androideabi="--target=armv7a-linux-androideabi$ANDROID_API -fPIC"

export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$NDK_BIN/x86_64-linux-android$ANDROID_API-clang"
export CC_x86_64_linux_android="$NDK_BIN/x86_64-linux-android$ANDROID_API-clang"
export CXX_x86_64_linux_android="$NDK_BIN/x86_64-linux-android$ANDROID_API-clang++"
export AR_x86_64_linux_android="$NDK_BIN/llvm-ar"
export RANLIB_x86_64_linux_android="$NDK_BIN/llvm-ranlib"
export CFLAGS_x86_64_linux_android="--target=x86_64-linux-android$ANDROID_API -fPIC"
