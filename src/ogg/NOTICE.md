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

## Opus

The decoder-only Opus modules adapt the same pinned TPT Cadence revision,
`tpt-av-cadence-opus/src`, under its retained MIT license. These modules port
the Opus 1.5.2 reference decoder; the reference copyright/license is retained
in `LICENSE-OPUS-BSD`. The production encoder, upstream containers, test harness
and environment-controlled tracing are excluded. The audiobox wrapper owns
Ogg parsing, resource limits, packet framing validation, trim/gain and seeking.

Original source SHA-256 digests:

```text
d3031bc9a2d81e6d50644ef4f74fc28c059f6154a0e6dd83cf21d6810137bcd9  src/packet.rs
fb06cb53f8e9f91a7ca0c806bbd2f7e54d6fde0efa9ee4474f898da2d2a57962  src/range.rs
f415a38272a026c74866ca13ac317b67164b6b09a7cb789b28017cff9fe20246  src/decoder.rs
52280b17129ee6257b770bef45bb04d95c8a5c36f345b14f51be17ec982489c1  src/celt/bands.rs
e85242659077390e7db191b7f60f7fab63ed4041a0e1b51e932e0d9403e699a5  src/celt/celt_lpc.rs
064da2343bcda222245765588272122dee155d4aa15886913e62e8690cee3ef7  src/celt/cwrs.rs
5db10b08fa987b4d88a861cfcb0ac1c74069c51d0f5b0b9d7c37eee25d6286bc  src/celt/decoder.rs
eb4b043b5d012733ddce9c7617dc241341e5e42c4b8f0540229dfc277c0476f2  src/celt/fft.rs
d2bfe6cfcd2b688b9fc309402bbc839b104b960d5b5d996c94011665a7fbeb20  src/celt/laplace.rs
586a2f068a2a7891d0c4aef3d814ee24fe73dfa5a24ec74ec2c27af693252729  src/celt/math.rs
c1e5c3b39f30c6d001eae86bb4bd6d321a1b7eb3b45f312d9efb93a23ea559b4  src/celt/mdct.rs
5db44d4afa7f64b2c8cf68a7b4ad599b2714013ac8b814421420192396462c9c  src/celt/pitch.rs
bacc53ccf594fb33441eb6995fc51091180eabd19bcdce8edd808cfff9100d49  src/celt/quant_bands.rs
734c82eb3b3e8c48fc929512f2218169169e446c974e6e41e3bfe178cb9e50a5  src/celt/rate.rs
207e8c76ceeeea3d319e424b6d528b33de205718d4f28ff3ce379480afd01b73  src/celt/tables.rs
5b4af43dc9986ce68b04eaab5210a1e626d8a33ee528df8caf1565f78641b0ca  src/celt/vq.rs
1a36a76f3c7f168c68dedac5c42abd8effbabe0eded987ee0d906b0d8b88677d  src/silk/decode_indices.rs
9a04f45708817ee06a136ffb8bfa33ccfcf869629f63d360d0d3911ea4352948  src/silk/decoder.rs
3777fccefea461acb0c632b06dea6b415757abc1fe6b38ca91d57528645e2002  src/silk/excitation.rs
905b8c53a2057e68f3361340ceac979fd9fe2a8f21f46cfbb1b977c61b16a98c  src/silk/gains.rs
f9e00612a4204e8078a4b38c24fd2f1591603592d6bbbb88d4f7175b0428888d  src/silk/nlsf.rs
4127cfdbc9990569dc99de13b7464d266abf273b16b478c5573dd510374553c1  src/silk/pitch.rs
607596a9750d49c139f942bc56e76efad7aa3f84e1b19ded04b7cfa10fbdff6e  src/silk/plc.rs
e2ab405d0d1e6e9db2b73e3bd9cec9dfabccf7d9e31885ec70c1bcd7e38b9c66  src/silk/resampler.rs
2a81da819b15ee0b9513e2c2b764f5f884e531ddc93136b1b7f43e6662aa5547  src/silk/sigproc.rs
66c2b0b2b37cd8b4cc24032cd3f476a518b33cd37b80a8a591247f37c9a8d52a  src/silk/stereo.rs
bb8e80bf14fac6abfa72576d70cab3c1fb95b4aec8ca3b777f25ca69870692e2  src/silk/synthesis.rs
fe39c7aac3fd52e2279381bef5a1585e34bbfa0dedd8e1f8f7a1e3b3ff34d311  src/silk/tables.rs
```

## Additional audiobox review corrections

Nonzero coupling propagation follows forward pair order; inverse coupling keeps
reverse order. Packet exhaustion has a dedicated nonallocating signal: floor
exhaustion marks that floor unused and residue exhaustion retains partial output,
including type-2 deinterleaving. Floor-1 line slopes retain the authored endpoint
when a long-block floor is used by a short block; only writes are clipped. Lookup
1 divisors advance incrementally, bounding setup work to O(decoded table cells).
Seven independent libvorbisfile controls retain these review regressions.
