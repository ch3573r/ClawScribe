# Decoder fixture

`he_aac_48k_5s.m4a` is the synthetic HE-AAC regression fixture from
[Meetily v0.4.1](https://github.com/Zackriya-Solutions/meetily/tree/v0.4.1/frontend/src-tauri/tests/fixtures),
distributed under the upstream MIT license (see the repository LICENSE.md).
It exercises a container sample rate that differs from Symphonia's decoded
rate, including duration preservation after conversion to 16 kHz.
