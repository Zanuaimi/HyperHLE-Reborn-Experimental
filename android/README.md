# Android renderer builds

ANGLE remains default. Renderer mode `zink` uses Mesa 26.2.5's Zink Gallium driver over the device Vulkan loader. Zink is experimental and requires Android API 26+ and a Vulkan-capable driver. It exposes GLES 2.0, so HyperHLE translates guest GLES 1.1 through `GLES1OnGLES2` for this mode. Runtime failure does not silently switch to ANGLE or native GLES.

The Android release workflow builds Mesa for arm64 with NDK r25c and packages `libEGL_zink.so`, `libGLESv2_zink.so`, `libgallium_dri.so`, and `libdrm.so`. It pins the Mesa source archive by SHA-256 in `dev-scripts/build-mesa-zink-android.sh` and caches generated libraries. Local Android arm64 builds offer the experimental choice; selecting it requires the generated Mesa libraries. Run `dev-scripts/build-mesa-zink-android.sh <NDK-r25c-path> <jniLibs/arm64-v8a-output>` to build them locally. If the device linker blocks a required platform library, HyperHLE reports the loader error instead of silently switching renderers.

Mesa's Android EGL platform also depends on gralloc/HAL libraries. Some stock Android linker namespaces may block `libhardware.so`; this APK experiment cannot guarantee Zink will initialize on every device. The generated APK assets include Mesa's license guide and license texts. Mesa source version and archive checksum are recorded in the build script.
