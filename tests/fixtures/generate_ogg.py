"""Generate synthetic codec inputs and independent FFmpeg f32 references.

Development only. python generate_ogg.py PATH_TO_FFMPEG
PCM is an integer triangle/noise/transient mixture; no third-party media.
"""
from pathlib import Path
import hashlib
import struct
import subprocess
import sys
import wave

root = Path(__file__).resolve().parents[2]
out = root / "tests/fixtures/ogg"
out.mkdir(parents=True, exist_ok=True)
temp = root / "target/ogg-fixtures"
temp.mkdir(parents=True, exist_ok=True)
ffmpeg = sys.argv[1]
sys.path.insert(0, str(root / "target/ogg-oracle"))
import soundfile as sf  # development oracle only; never a crate dependency
cases = [(f"vorbis-mono-{r}", r, 1, r // 5 + 37, 4) for r in [8000, 16000, 22050, 32000, 44100, 48000, 96000]]
cases += [(f"vorbis-stereo-q{q}", 48000, 2, 9637, q) for q in [-1, 0, 4, 10]]
cases += [(f"vorbis-{c}ch", 44100, c, 8857, 4) for c in [3, 4, 5, 6, 7, 8]]
cases += [(f"vorbis-short-{n}", 44100, 2, n, 4) for n in [1, 31, 64, 127, 128, 129, 255, 1025]]
rows = ["case,rate,channels,input_frames,reference_frames,encoded_sha256,reference_sha256"]
for name, rate, channels, frames, quality in cases:
    seed = 7
    pcm = bytearray()
    for i in range(frames):
        for c in range(channels):
            seed = (1664525 * seed + 1013904223) & 0xFFFFFFFF
            period = 53 + 18 * c
            triangle = ((i * 2 % (2 * period)) - period)
            value = triangle * 7000 // period + ((seed >> 20) - 2048) // 4
            if i % 997 < 4:
                value += 9000 if c % 2 == 0 else -9000
            pcm += struct.pack("<h", value)
    wav = temp / f"{name}.wav"
    if channels <= 2:
        with wave.open(str(wav), "wb") as w:
            w.setnchannels(channels)
            w.setsampwidth(2)
            w.setframerate(rate)
            w.writeframes(pcm)
    else:
        mask = {3: 7, 4: 0x33, 5: 0x37, 6: 0x3f, 7: 0x70f, 8: 0x63f}[channels]
        guid = bytes.fromhex("0100000000001000800000aa00389b71")
        fmt = struct.pack("<HHIIHHHHI16s", 0xfffe, channels, rate, rate*channels*2, channels*2, 16, 22, 16, mask, guid)
        body = b"WAVEfmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(pcm)) + pcm
        wav.write_bytes(b"RIFF" + struct.pack("<I", len(body)) + body)
    encoded = out / f"{name}.ogg"
    ref = out / f"{name}.f32"
    subprocess.run([ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-i", str(wav), "-c:a", "libvorbis", "-q:a", str(quality), "-fflags", "+bitexact", "-flags:a", "+bitexact", "-metadata", "title=audiobox synthetic fixture", str(encoded)], check=True)
    independent = temp / f"{name}-ffmpeg.f32"
    subprocess.run([ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-i", str(encoded), "-f", "f32le", "-c:a", "pcm_f32le", str(independent)], check=True)
    with sf.SoundFile(str(encoded)) as f:
        reference = bytes(f.buffer_read(f.frames, dtype="float32"))
    # libvorbisfile supplies Vorbis stream order; audiobox/FFmpeg expose WAV order.
    permutation = {3: [0,2,1], 4: [0,1,2,3], 5: [0,2,1,3,4], 6: [0,2,1,5,3,4], 7: [0,2,1,6,5,3,4], 8: [0,2,1,7,5,6,3,4]}.get(channels)
    if permutation:
        values = memoryview(reference).cast("f")
        reference = b"".join(struct.pack("<f", values[i*channels+c]) for i in range(frames) for c in permutation)
    assert len(reference) == frames * channels * 4, (name, len(reference))
    # FFmpeg's Ogg timestamps can expose padding/drop a very short packet. Its
    # aligned decoded samples must still agree; libvorbisfile owns exact trim.
    a = struct.iter_unpack("<f", reference)
    b = struct.iter_unpack("<f", independent.read_bytes())
    difference = max((abs(x[0]-y[0]) for x,y in zip(a,b)), default=0.0)
    assert difference < 0.00002, (name, difference, frames, len(reference), independent.stat().st_size)
    ref.write_bytes(reference)
    rows.append(f"{name},{rate},{channels},{frames},{ref.stat().st_size//4//channels},{hashlib.sha256(encoded.read_bytes()).hexdigest()},{hashlib.sha256(ref.read_bytes()).hexdigest()}")
# A minimal, independently decoded floor-0 stream. Modern libvorbis encoders
# choose floor 1, so an authored setup exercises floor-0 preparation/synthesis.
class Bits:
    def __init__(self): self.bits = []
    def put(self, value, width): self.bits.extend((value >> i) & 1 for i in range(width))
    def finish(self):
        result = bytearray((len(self.bits)+7)//8)
        for i,b in enumerate(self.bits): result[i//8] |= b << (i%8)
        return bytes(result)
def page(payload, sequence, granule, flags):
    laces = bytes([255]*(len(payload)//255)+[len(payload)%255])
    result = bytearray(b"OggS"+bytes([0,flags])+struct.pack("<qII", granule, 17, sequence)+bytes(4)+bytes([len(laces)])+laces+payload)
    crc = 0
    for b in result:
        crc ^= b << 24
        for _ in range(8): crc = ((crc << 1) ^ (0x04c11db7 if crc & 0x80000000 else 0)) & 0xffffffff
    result[22:26] = struct.pack("<I", crc)
    return result
for name,order,dims,amplitude in [("vorbis-floor0",1,1,8),("vorbis-floor0-even",2,2,2),("vorbis-floor0-vector",3,2,2)]:
    b = Bits()
    for value,width in [(0,8),(0x564342,24),(dims,16),(1,24),(0,1),(0,1),(0,5),(2,4),(0x60100000 if dims==1 else 0x5ff00000,32),(0x60100000,32),(0,4),(0,1)]: b.put(value,width)
    for i in range(dims): b.put(i,1)
    for value,width in [(0,6),(0,16),(0,6),(0,16),(order,8),(8000,16),(32,16),(4,6),(60,8),(0,4),(0,8),
                        (0,6),(1,16),(0,24),(32,24),(31,24),(0,6),(0,8),(1,3),(0,1),(0,8),
                        (0,6),(0,16),(0,1),(0,1),(0,2),(0,8),(0,8),(0,8),
                        (0,6),(0,1),(0,16),(0,16),(0,8),(1,1)]: b.put(value,width)
    setup = b"\x05vorbis"+b.finish()
    id_header = b"\x01vorbis"+struct.pack("<IBIiiiBB",0,1,8000,0,0,0,0x66,1)
    title = b"title=audiobox synthetic fixture"
    comment = b"\x03vorbis"+struct.pack("<II",0,1)+struct.pack("<I",len(title))+title+b"\x01"
    b = Bits()
    for value,width in [(0,1),(amplitude,4),(0,1),(0,(order+dims-1)//dims),(0,1),(0,32//dims)]: b.put(value,width)
    audio_packet = b.finish()
    encoded = out / f"{name}.ogg"
    encoded.write_bytes(b"".join(page(p,i,0 if i<4 else (i-3)*32,2 if i==0 else 4 if i==6 else 0) for i,p in enumerate([id_header,comment,setup]+[audio_packet]*4)))
    with sf.SoundFile(str(encoded)) as f:
        reference = bytes(f.buffer_read(f.frames,dtype="float32"))
    assert len(reference) == 96*4
    (out / f"{name}.f32").write_bytes(reference)
    rows.append(f"{name},8000,1,96,96,{hashlib.sha256(encoded.read_bytes()).hexdigest()},{hashlib.sha256(reference).hexdigest()}")
(out / "vorbis.csv").write_text("\n".join(rows) + "\n", encoding="utf-8", newline="\n")
print(subprocess.check_output([ffmpeg, "-version"], text=True).splitlines()[0])
print("SoundFile", sf.__version__, "libsndfile", sf.__libsndfile_version__)
print("\n".join(rows))
