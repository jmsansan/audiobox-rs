These six files were generated from the original audiobox TypeScript implementation
(https://github.com/jmsansan/audiobox, commit 6f89fca7eefb7367738454d64b329c96f4fd81bc, src/index.ts) during the Rust migration. They contain 4097 stereo frames at
44100 Hz: a 440 Hz sine at amplitude 0.6 on the left and an 880 Hz sine at
amplitude 0.4 on the right. PCM/FLAC files use 16-bit samples. MP3 uses VBR,
quality 2 and a 192 kbit/s ceiling. The fixtures are synthetic, MIT-licensed
repository test data and require no runtime JavaScript dependency.
