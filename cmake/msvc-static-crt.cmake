# T-117 (decisions #197): CMake toolchain file for x86_64-pc-windows-msvc builds only. cmake-rs
# loads it through CMAKE_TOOLCHAIN_FILE_x86_64_pc_windows_msvc (.cargo/config.toml [env]); no
# build for another target sees it.
#
# libopus (opusic-sys 0.7.5, bundled): opus picks its MSVC runtime from this option and the build
# type (opusic-sys always configures Release), and opusic-sys never sets it, so libopus would be
# /MD. ON selects the static CRT (/MT, libcmt), the CRT of the shell (tauri-build
# static_vcruntime). The help text names no checker token: scripts/ci/opus-cmake-cache.sh refuses
# any cache line other than the owned value line that names the option.
#
# whisper.cpp (whisper-rs-sys) gets this file too, through the same cmake-rs lookup: it reads
# neither variable set here, and its CMP0091 default is already forced by .cargo/config.toml.
# Proven on the Windows CI build: scripts/ci/opus-cmake-cache.sh (cache and dumpbin -directives
# of opus.lib), scripts/ci/whisper-cmake-cache.sh (docs/decisions/ci-toolchain.md).
set(OPUS_STATIC_RUNTIME ON CACHE BOOL "Link the C runtime statically (/MT), set by the voicen toolchain file" FORCE)
set(CMAKE_POLICY_DEFAULT_CMP0091 NEW)
