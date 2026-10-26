# Decoder source provenance

The Vorbis packet DSP in `vorbis/` is adapted from
https://github.com/tpt-solutions/tpt-cadence/tree/c8c99890606687addd79601e8212592d2a9d41eb/tpt-av-cadence-vorbis
(commit `c8c99890606687addd79601e8212592d2a9d41eb`), under its MIT license reproduced in `LICENSE-TPT-MIT`.
The upstream source files' SHA-256 digests before adaptation are below.
No Cargo dependency, FFI or build-time download is introduced. audiobox owns
the Ogg framing, API integration, bounded preparation and seek policy.

Adaptations: private module paths, decoder-only code, caller-supplied preparation
budget, packet/reset interface and independent integration tests. Upstream's
standalone container, encoder and test harness are excluded.

The independent corpus required additional corrections: residue submap bundle
sizes, 6.1 channel order, floor-0 polynomial/gain evaluation and vector offsets,
and floor-0 scratch preparation. Header allocation budgets are charged before
allocation. Huffman assignment caches available depths to avoid repeated scans
of full subtrees; codeword assignment is retained. The original standalone
facade's packet synthesis/lapping helpers are also adapted in `vorbis/mod.rs`.

```text
0da3f1dcea89e1c1432ae21c8dbefad19fb3938eb1b053cdfaee7a0a73026a2c  src/bitreader.rs
ac30f08b2558d929dc27220cf6e1f8bfc7a61019a67e2179e254275e9ad4f848  src/codebook.rs
678bb02d13fce7ee34128699f90e7ca12e82edf38ce86933f42d605cc092609c  src/fft.rs
b4216e94b714a92afc50e31f1af2c877289d2ca02e70bb63391b79052790a261  src/floor.rs
43aab56ad2f662ce27f40f58d02c359e973ab24476db253b3cc208d8841aceb9  src/header.rs
63f94bd1f471c3f89e3ffa49ffe95cf210b4390a7a984a1294c421a67f6478a0  src/mdct.rs
a3a76802080e46fc23e6d4ba7eb6caee3a425569d81458606ca8bd7bf6f9e277  src/residue.rs
e3afcced6bbb2fc8497020c518ece9e56a9159fcbb893506b2c70448df499e65  src/lib.rs
```
