default:
  @just --list

build-release:
    cargo ndk -t arm64-v8a -t armeabi-v7a --platform=29 build --release

package-release: build-release
    [[ -d out ]] || mkdir -p out
    [[ ! -d out/release ]] || rm -rf out/release
    cp -r module out/release
    mkdir out/release/zygisk
    cp target/aarch64-linux-android/release/libmetadump.so out/release/zygisk/arm64-v8a.so
    cp target/armv7-linux-androideabi/release/libmetadump.so out/release/zygisk/armeabi-v7a.so
    cd out/release/ && zip -r ../metadump-release.zip .
