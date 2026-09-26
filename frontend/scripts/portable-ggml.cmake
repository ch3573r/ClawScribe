# Adapted from Meetily v0.4.1's portable Windows build guard (MIT).
# Apply after project() so whisper-rs-sys cannot restore host-native defaults.
set(GGML_NATIVE OFF CACHE BOOL "Disable host CPU specialization" FORCE)
foreach(flag GGML_AVX512 GGML_AVX512_VBMI GGML_AVX512_VNNI GGML_AVX512_BF16)
  set(${flag} OFF CACHE BOOL "Portable Windows CPU baseline" FORCE)
endforeach()
