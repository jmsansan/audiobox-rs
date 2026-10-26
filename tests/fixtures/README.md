These six files were generated from the original audiobox TypeScript implementation
(https://github.com/jmsansan/audiobox, commit 6f89fca7eefb7367738454d64b329c96f4fd81bc, src/index.ts) during the Rust migration. They contain 4097 stereo frames at
44100 Hz: a 440 Hz sine at amplitude 0.6 on the left and an 880 Hz sine at
amplitude 0.4 on the right. PCM/FLAC files use 16-bit samples. MP3 uses VBR,
quality 2 and a 192 kbit/s ceiling. The fixtures are synthetic, MIT-licensed
repository test data and require no runtime JavaScript dependency.

The `lame-*.mp3` and `ima-stereo.wav` files are synthetic sines encoded with FFmpeg 8
and libmp3lame, for the incremental decoder's tests. `lame-stereo.mp3` is 3 s of
44.1 kHz joint stereo (VBR `-q:a 6`, with a LAME gapless tag), `lame-mpeg2.mp3` 2 s
of 22.05 kHz mono at 32 kbit/s (MPEG-2), and `lame-mpeg25.mp3` 2 s of 8 kHz mono at
16 kbit/s (MPEG-2.5). Most of their frames use the bit reservoir. `ima-stereo.wav` is
2 s of 11.025 kHz stereo IMA ADPCM.

`cover.png` and `cover.jpg` are synthetic 16x16 flat violet images generated with
FFmpeg 7.1 for front-cover metadata tests. No outside artwork is used. For each
extension: `ffmpeg -f lavfi -i color=c=0x6841cb:s=16x16:d=0.04 -frames:v 1 -update 1 cover.EXT`.

`mp3-contract.csv` pins 246 original rational/seeded synthetic fixtures generated
by `tests/mp3_contract.rs`, initially with Rust 1.98.1 release on Windows x86_64.
Its stage hashes and encoded lengths are regression evidence, not reference
quality measurements or a general arbitrary-input MP3 identity guarantee.
See `docs/mp3-export-contract.md` for the matrix, hash layout, comparison artifacts
and individual-case/boundary reproduction helpers. The test and data are MIT-licensed.

The #95 gapless correction updates only the MP3 encoded lengths/hashes in this
246-case CSV. Every source and decoded-PCM hash was compared and retained. Its
separate 32..=48-frame tail compatibility control is in `tests/mp3_gapless.rs`.
