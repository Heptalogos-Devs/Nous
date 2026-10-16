#!/usr/bin/env bash
# Copyright 2026 Aravine Zhu
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail
# Explicit release build; no privilege or system installation is required.
# Windows (unchanged): source archive, verified LLVM-MinGW archive, output, [build root].
# Linux host: --linux source archive output [build root].
mode=windows-x64
if test "${1:-}" = --linux; then
  test "$(uname -s)" = Linux || { echo "Linux host build requires Linux" >&2; exit 1; }
  mode=linux-x64
  source_archive=$(realpath "$2")
  output=$(realpath -m "$3")
  build=${4:-}
else
  source_archive=$(realpath "$1")
  compiler_archive=$(realpath "$2")
  output=$(realpath -m "$3")
  test "$(sha256sum "$compiler_archive" | cut -d' ' -f1)" = bb7bb7654b33d5aa8712acb837c963b2e0c56352560c76105270a3268c665c21
  build=${4:-}
fi
notices_script=$(realpath "$(dirname "${BASH_SOURCE[0]}")/build-notices.sh")
test "$(sha256sum "$source_archive" | cut -d' ' -f1)" = 8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
if test -z "$build"; then build="$(dirname "${BASH_SOURCE[0]}")/../../data/cache/runtime-build/ffmpeg/$mode"; fi
build=$(realpath -m "$build")
data_root=$(realpath -m "$(dirname "${BASH_SOURCE[0]}")/../../data")
case "$output/" in "$data_root/"*) ;; *) echo "Output must be under repository data/" >&2; exit 1;; esac
case "$build/" in "$data_root/"*) ;; *) echo "Build root must be under repository data/" >&2; exit 1;; esac

mkdir -p "$build"
tar -xf "$source_archive" -C "$build"
if test "$mode" = windows-x64; then
  tar -xf "$compiler_archive" -C "$build"
  toolchain="$build/llvm-mingw-20260922-ucrt-ubuntu-22.04-x86_64"
  bash "$notices_script" "$toolchain" "$output"
  export PATH="$toolchain/bin:/usr/bin:/bin"
fi
export LC_ALL=C TZ=UTC SOURCE_DATE_EPOCH=1789709400
mkdir -p "$output/bin" "$output/licenses/ffmpeg"
cd "$build/ffmpeg-9.0.2"
flags=()
if test "$mode" = windows-x64; then
  flags+=(
    --target-os=mingw32 --arch=x86_64 --enable-cross-compile
    --cross-prefix=x86_64-w64-mingw32-
    --cc=x86_64-w64-mingw32-clang --cxx=x86_64-w64-mingw32-clang++
    --ar=x86_64-w64-mingw32-ar --ranlib=x86_64-w64-mingw32-ranlib
    --nm=x86_64-w64-mingw32-nm --strip=x86_64-w64-mingw32-strip
  )
else
  flags+=(--target-os=linux --arch=x86_64 --cc=gcc --cxx=g++ --enable-zlib)
fi
flags+=(
  --disable-gpl --disable-nonfree --disable-version3 --disable-autodetect
  --disable-network --disable-doc --disable-debug --disable-x86asm
  --disable-everything --disable-ffplay --enable-ffmpeg --enable-ffprobe
  --enable-protocol=file,pipe
  --enable-demuxer=mov,matroska,ogg,avi,mpegts,mpegvideo,wav,mp3,gif,image2,png_pipe
  --enable-decoder=h264,hevc,vp8,vp9,aac,mp3,vorbis,opus,mjpeg,png,pcm_s16le,gif
  --enable-encoder=mjpeg,pcm_s16le
  --enable-muxer=image2,wav,mov,mp4,null
  --enable-filter=scale,showinfo,format,aformat,aresample,anull
  --extra-cflags=-ffile-prefix-map=.=./
)
printf '%s\n' "${flags[@]}" > "$output/licenses/ffmpeg/build-config.txt"
./configure "${flags[@]}" > "$output/licenses/ffmpeg/configure.log" 2>&1
make -j8 > "$output/licenses/ffmpeg/build.log" 2>&1
if test "$mode" = windows-x64; then cp ffmpeg.exe ffprobe.exe "$output/bin/"; else
  cp ffmpeg ffprobe "$output/bin/"
  { gcc --version | head -n1; uname -srm; ldd --version | head -n1; } > "$output/licenses/ffmpeg/host-toolchain.txt"
  (cd "$output" && sha256sum bin/ffmpeg bin/ffprobe > licenses/ffmpeg/executables.sha256)
fi
cp COPYING.LGPLv2.1 LICENSE.md "$output/licenses/ffmpeg/"
cp "$source_archive" "$output/licenses/ffmpeg/ffmpeg-9.0.2.tar.xz"
printf '%s\n' 'https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz' > "$output/licenses/ffmpeg/source-url.txt"
printf '%s\n' '8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e' > "$output/licenses/ffmpeg/source.sha256"
printf '%s\n' 'No source patches. Private executable invocation, no application linkage.' > "$output/licenses/ffmpeg/changes.txt"
