"""Development-only synthetic Ogg Opus corpus; FFmpeg/libopus is the oracle."""
from pathlib import Path
import hashlib
import struct
import subprocess
import sys
import wave

root = Path(__file__).resolve().parents[2]
out = root / "tests/fixtures/ogg"
temp = root / "target/opus-fixtures"
temp.mkdir(parents=True,exist_ok=True)
ffmpeg = sys.argv[1]
def packets(data):
    result=[]; current=bytearray(); at=0
    while at<len(data):
        segs=data[at+26]; body=at+27+segs
        for length in data[at+27:body]:
            current.extend(data[body:body+length]); body+=length
            if length<255: result.append(bytes(current)); current.clear()
        at=body
    return result
def page(payload, sequence, granule, flags):
    laces = bytes([255]*(len(payload)//255)+[len(payload)%255])
    result = bytearray(b"OggS"+bytes([0,flags])+struct.pack("<qII",granule,17,sequence)+bytes(4)+bytes([len(laces)])+laces+payload)
    crc=0
    for byte in result:
        crc ^= byte<<24
        for _ in range(8): crc=((crc<<1)^(0x04c11db7 if crc&0x80000000 else 0))&0xffffffff
    result[22:26]=struct.pack("<I",crc)
    return result
def duration(packet):
    config=packet[0]>>3
    frame = [480,960,1920,2880][config%4] if config<12 else [480,960][config%2] if config<16 else [120,240,480,960][config%4]
    code=packet[0]&3
    count=1 if code==0 else 2 if code in (1,2) else packet[1]&63
    return frame*count
def save(name, encoded, expected, channels):
    path=out/(name+".opus"); path.write_bytes(encoded)
    ref=out/(name+".f32")
    subprocess.run([ffmpeg,"-hide_banner","-loglevel","error","-y","-c:a","libopus","-request_sample_fmt","flt","-i",str(path),"-f","f32le","-c:a","pcm_f32le",str(ref)],check=True)
    assert ref.stat().st_size == expected*channels*4,(name,expected,ref.stat().st_size)
    modes=sorted({"silk" if p[0]>>3<12 else "hybrid" if p[0]>>3<16 else "celt" for p in packets(encoded)[2:]})
    rows.append(f"{name},48000,{channels},{expected},{expected},{'+'.join(modes)},{hashlib.sha256(encoded).hexdigest()},{hashlib.sha256(ref.read_bytes()).hexdigest()}")
    return encoded
cases=[]
for rate in [8000,12000,16000]:
    for ch in [1,2]: cases.append((f"opus-silk-{rate}-{ch}ch",rate,ch,rate//2+37,"voip",20,12000*ch,[]))
for ch in [1,2]:
    cases.append((f"opus-hybrid-{ch}ch",48000,ch,24037,"voip",20,18000*ch,[]))
for ms in [2.5,5,10,20,40,60,80,100,120]:
    cases.append((f"opus-celt-{str(ms).replace('.','_')}ms",48000,2,12037,"lowdelay",ms,128000,[]))
for ch in [3,4,5,6,7,8]: cases.append((f"opus-{ch}ch",48000,ch,12037,"audio",20,48000*ch,[]))
for n in [1,31,120,121,312,313,1025]: cases.append((f"opus-short-{n}",48000,2,n,"audio",20,64000,[]))
cases += [("opus-fec",16000,2,8037,"voip",20,28000,["-fec","1","-packet_loss","30"]),("opus-dtx",16000,1,16037,"voip",20,8000,["-dtx","1"])]
rows=["case,rate,channels,program_frames,reference_frames,modes,encoded_sha256,reference_sha256"]
encoded_cases={}
for name,rate,channels,frames,application,ms,bitrate,extra in cases:
    pcm=bytearray(); seed=17
    for i in range(frames):
        for c in range(channels):
            seed=(1664525*seed+1013904223)&0xffffffff
            period=53+18*c
            value=((i*2%(2*period))-period)*7000//period+((seed>>20)-2048)//4
            if i%997<4: value+=9000 if c%2==0 else -9000
            if "dtx" in name and i>rate//10: value=0
            pcm+=struct.pack("<h",value)
    wav=temp/(name+".wav")
    if channels<=2:
        with wave.open(str(wav),"wb") as f: f.setparams((channels,2,rate,frames,"NONE","NONE")); f.writeframes(pcm)
    else:
        mask={3:7,4:0x33,5:0x37,6:0x3f,7:0x70f,8:0x63f}[channels]
        fmt=struct.pack("<HHIIHHHHI16s",0xfffe,channels,rate,rate*channels*2,channels*2,16,22,16,mask,bytes.fromhex("0100000000001000800000aa00389b71"))
        body=b"WAVEfmt "+struct.pack("<I",len(fmt))+fmt+b"data"+struct.pack("<I",len(pcm))+pcm
        wav.write_bytes(b"RIFF"+struct.pack("<I",len(body))+body)
    path=temp/(name+".opus")
    subprocess.run([ffmpeg,"-hide_banner","-loglevel","error","-y","-i",str(wav),"-c:a","libopus","-application",application,"-frame_duration",str(ms),"-b:a",str(bitrate),"-fflags","+bitexact","-flags:a","+bitexact","-metadata","title=audiobox synthetic opus fixture",*extra,str(path)],check=True)
    encoded_cases[name]=save(name,path.read_bytes(),frames*(48000//rate),channels)
# Exercise mode and mono/stereo changes through one persistent stereo decoder.
parts=[]
for name in ["opus-silk-16000-1ch","opus-hybrid-2ch","opus-celt-20ms","opus-silk-8000-2ch","opus-hybrid-1ch","opus-celt-10ms"]:
    parts.extend(packets(encoded_cases[name])[2:12])
headers=packets(encoded_cases["opus-hybrid-2ch"])[:2]
skip=struct.unpack_from("<H",headers[0],10)[0]
total=sum(map(duration,parts))
payloads=headers+parts
encoded=b"".join(page(p,i,0 if i<2 else sum(map(duration,parts[:i-1])),2 if i==0 else 4 if i==len(payloads)-1 else 0) for i,p in enumerate(payloads))
save("opus-transitions",encoded,total-skip,2)
for gain in [12,-18]:
    p=packets(encoded_cases["opus-celt-20ms"])
    h=bytearray(p[0]); h[16:18]=struct.pack("<h",gain*256); p[0]=bytes(h)
    total=0; frames=12037; skip=struct.unpack_from("<H",p[0],10)[0]
    pages=[]
    for i,payload in enumerate(p):
        if i>=2: total+=duration(payload)
        gp=0 if i<2 else frames+skip if i==len(p)-1 else total
        pages.append(page(payload,i,gp,2 if i==0 else 4 if i==len(p)-1 else 0))
    save(f"opus-gain-{gain}",b"".join(pages),frames,2)
(out/"opus.csv").write_text("\n".join(rows)+"\n",encoding="utf-8",newline="\n")
print(subprocess.check_output([ffmpeg,"-version"],text=True).splitlines()[0])
print("\n".join(rows))
