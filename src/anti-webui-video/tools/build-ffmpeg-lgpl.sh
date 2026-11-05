#!/bin/sh
# Builds the smallest FFmpeg that plays what `video()` is meant to play, as LGPL shared libraries to
# put next to an application, in `<exe>/ffmpeg/`. See docs/design/24-video.md.
#
#   tools/build-ffmpeg-lgpl.sh <unpacked FFmpeg source> <where the libraries go>
#
# Whoever hands these out has to follow the LGPL: see https://ffmpeg.org/legal.html. It also isn't
# for this script to say whether patents allow the decoders of H.264 and HEVC where you are: take
# them out of DECODERS to leave them out.
set -eu

src=$(realpath "$1")
mkdir -p "$2"
out=$(realpath "$2")

DECODERS=h264,hevc,vp8,vp9,av1,aac,aac_latm,mp3float,opus,vorbis,flac,pcm_s16le,pcm_s24le,pcm_f32le
DEMUXERS=mov,matroska,ogg,mp3,aac,flac,wav
PARSERS=h264,hevc,vp8,vp9,av1,aac,aac_latm,mpegaudio,opus,vorbis,flac
HWACCELS=h264_vaapi,hevc_vaapi,vp8_vaapi,vp9_vaapi,av1_vaapi

extra=""
# Software AV1 needs dav1d: FFmpeg's own decoder only drives hardware.
pkg-config --exists dav1d && extra="$extra --enable-libdav1d --enable-decoder=libdav1d"
pkg-config --exists libva libva-drm && extra="$extra --enable-vaapi --enable-hwaccel=$HWACCELS"
# Without an assembler it still works, but decoding on the CPU is several times slower.
command -v nasm >/dev/null || extra="$extra --disable-x86asm"

cd "$src"
# shellcheck disable=SC2086
./configure \
    --prefix="$out" \
    --disable-everything --disable-programs --disable-doc --disable-static --enable-shared \
    --disable-avdevice --disable-avfilter --disable-network --disable-autodetect \
    --enable-protocol=file \
    --enable-decoder="$DECODERS" --enable-demuxer="$DEMUXERS" --enable-parser="$PARSERS" \
    --enable-swscale --enable-swresample \
    $extra
make -j"$(nproc)"
make install
cp ffbuild/config.log "$out/configure-line.txt" 2>/dev/null || true
for f in "$out"/lib/*.so.*; do
    [ -L "$f" ] || strip "$f"
done
ls -lL "$out"/lib/*.so.*.*.* 
