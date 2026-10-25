#!/usr/bin/env bash
set -euo pipefail

MESA_VERSION=26.2.5
MESA_SHA256=794b3d1729075dae7ebd47c7283b7ca0cdb5dec668f7f11cc0a5d82f6138a0a5
ANDROID_API=26
NDK_ROOT=${1:-${ANDROID_NDK_ROOT:-}}
OUTPUT_DIR=${2:-}
if [[ -z "$NDK_ROOT" || -z "$OUTPUT_DIR" ]]; then
    echo "Usage: $0 <android-ndk-r25c> <jniLibs/arm64-v8a output directory> [asset output directory]" >&2
    exit 2
fi
GENERATED_DIR=$(dirname "$(dirname "$OUTPUT_DIR")")
ASSET_DIR=${3:-"$GENERATED_DIR/assets/mesa-zink"}
BUILD_INFO="$GENERATED_DIR/mesa-zink-build-info.txt"
EXPECTED_BUILD_INFO="Mesa=$MESA_VERSION sha256=$MESA_SHA256 api=$ANDROID_API ndk=25.2.9519653"

TOOLCHAIN="$NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64"
CLANG="$TOOLCHAIN/bin/aarch64-linux-android${ANDROID_API}-clang"
CLANGXX="$TOOLCHAIN/bin/aarch64-linux-android${ANDROID_API}-clang++"
NDK_REVISION=$(sed -n 's/^Pkg.Revision = //p' "$NDK_ROOT/source.properties")
if [[ "$NDK_REVISION" != "25.2.9519653" ]]; then
    echo "Expected NDK r25c (25.2.9519653), got ${NDK_REVISION:-unknown}" >&2
    exit 1
fi

for tool in "$CLANG" "$CLANGXX" "$TOOLCHAIN/bin/llvm-ar" "$TOOLCHAIN/bin/llvm-strip"; do
    if [[ ! -x "$tool" ]]; then
        echo "Missing Android NDK tool: $tool" >&2
        exit 1
    fi
done

mkdir -p "$OUTPUT_DIR"
CACHED_LIBRARIES_VALID=false
if [[ -f "$BUILD_INFO" && "$(<"$BUILD_INFO")" == "$EXPECTED_BUILD_INFO" && \
      -f "$ASSET_DIR/Mesa-license.rst" && \
      -f "$ASSET_DIR/licenses/MIT" && -f "$ASSET_DIR/BUILD-INFO.txt" ]]; then
    CACHED_LIBRARIES_VALID=true
fi
for library in libEGL_zink.so libGLESv2_zink.so libgallium_dri.so libdrm.so; do
    if [[ ! -f "$OUTPUT_DIR/$library" ]] || \
       ! "$TOOLCHAIN/bin/llvm-readelf" -h "$OUTPUT_DIR/$library" | grep -q 'Machine:.*AArch64'; then
        CACHED_LIBRARIES_VALID=false
        break
    fi
done
if [[ "$CACHED_LIBRARIES_VALID" == true ]]; then
    echo "Using cached Mesa Zink libraries in $OUTPUT_DIR"
    exit 0
fi
rm -f "$BUILD_INFO"

WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT
ARCHIVE="$WORK_DIR/mesa-$MESA_VERSION.tar.xz"
SOURCE="$WORK_DIR/mesa"
BUILD="$WORK_DIR/build"
PREFIX="$WORK_DIR/install"
VENV="$WORK_DIR/venv"

curl --fail --location --retry 2 \
    "https://archive.mesa3d.org/mesa-$MESA_VERSION.tar.xz" \
    --output "$ARCHIVE"
printf '%s  %s\n' "$MESA_SHA256" "$ARCHIVE" | sha256sum --check
mkdir "$SOURCE"
tar -xJf "$ARCHIVE" --strip-components=1 -C "$SOURCE"

python3 -m venv "$VENV"
"$VENV/bin/pip" install --disable-pip-version-check \
    'meson==1.7.0' 'ninja==1.13.2' 'Mako==1.3.10' 'PyYAML==6.0.2' \
    'Jinja2==3.1.6' 'MarkupSafe==3.0.4' 'packaging==25.0'

CROSS_FILE="$WORK_DIR/android-aarch64.ini"
cat > "$CROSS_FILE" <<EOF
[binaries]
c = ['$CLANG', '-fno-exceptions', '-fno-emulated-tls', '-fno-unwind-tables', '-fno-asynchronous-unwind-tables']
cpp = ['$CLANGXX', '-fno-exceptions', '-fno-emulated-tls', '-fno-unwind-tables', '-fno-asynchronous-unwind-tables', '-static-libstdc++']
ar = '$TOOLCHAIN/bin/llvm-ar'
strip = '$TOOLCHAIN/bin/llvm-strip'
pkg-config = '/usr/bin/pkg-config'

[host_machine]
system = 'android'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'

[properties]
needs_exe_wrapper = true
pkg_config_libdir = ['$TOOLCHAIN/sysroot/usr/lib/aarch64-linux-android/$ANDROID_API/pkgconfig']
EOF

# Native TLS keeps Mesa's exported GL dispatch symbols visible across DSOs.
# NDK exposes sys/shm.h without the SysV SHM functions; Zink does not need them.
PATH="$VENV/bin:$PATH" "$VENV/bin/meson" setup "$BUILD" "$SOURCE" \
    --cross-file "$CROSS_FILE" \
    --prefix "$PREFIX" --libdir lib --buildtype release \
    -Dplatforms=android -Dplatform-sdk-version="$ANDROID_API" \
    -Dandroid-stub=true -Dandroid-strict=true \
    -Degl=enabled -Degl-native-platform=android \
    -Degl-lib-suffix=_zink -Dgles-lib-suffix=_zink \
    -Dgles1=disabled -Dgles2=enabled \
    -Dgallium-drivers=zink -Dvulkan-drivers= \
    -Dglvnd=disabled -Dllvm=disabled -Dglx=disabled -Dgbm=disabled \
    -Dxmlconfig=disabled -Dexpat=disabled -Dzstd=disabled \
    -Dcpp_rtti=false -Dtools= -Dbuild-tests=false -Dvideo-codecs=[] \
    -Dallow-fallback-for=libdrm \
    -Dlibdrm:tests=false -Dlibdrm:install-test-programs=false \
    -Dlibdrm:intel=disabled -Dlibdrm:radeon=disabled \
    -Dlibdrm:amdgpu=disabled -Dlibdrm:nouveau=disabled \
    -Dlibdrm:vmwgfx=disabled -Dlibdrm:freedreno=disabled \
    -Dlibdrm:vc4=disabled -Dlibdrm:etnaviv=disabled \
    -Dlibdrm:freedreno-kgsl=false \
    -Dc_args=-UHAVE_SYS_SHM_H -Dcpp_args=-UHAVE_SYS_SHM_H \
    --wrap-mode=default
"$VENV/bin/meson" compile -C "$BUILD" -j2
"$VENV/bin/meson" install -C "$BUILD" --strip

for library in libEGL_zink.so libGLESv2_zink.so libgallium_dri.so libdrm.so; do
    source="$PREFIX/lib/$library"
    if [[ ! -f "$source" ]]; then
        echo "Mesa Zink build did not produce $source" >&2
        exit 1
    fi
    "$TOOLCHAIN/bin/llvm-strip" --strip-unneeded "$source"
    "$TOOLCHAIN/bin/llvm-readelf" -h "$source" | grep -q 'Machine:.*AArch64'
    cp "$source" "$OUTPUT_DIR/$library"
done

mkdir -p "$ASSET_DIR"
cp "$SOURCE/docs/license.rst" "$ASSET_DIR/Mesa-license.rst"
rm -rf "$ASSET_DIR/licenses"
mkdir -p "$ASSET_DIR/licenses"
cp -R "$SOURCE/licenses/." "$ASSET_DIR/licenses/"
printf 'Mesa %s Zink source archive SHA-256: %s\n' \
    "$MESA_VERSION" "$MESA_SHA256" > "$ASSET_DIR/BUILD-INFO.txt"
printf '%s' "$EXPECTED_BUILD_INFO" > "$BUILD_INFO"

echo "Mesa $MESA_VERSION Zink libraries packaged in $OUTPUT_DIR"
