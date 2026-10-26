from pathlib import Path
import sys,struct
root=Path(__file__).resolve().parents[2]; sys.path.insert(0,str(root/'target/ogg-oracle'));import soundfile as sf
out=root/'tests/fixtures/ogg';out.mkdir(exist_ok=True)
class Bits:
 def __init__(self):self.bits=[]
 def put(self,v,w):self.bits.extend((v>>i)&1 for i in range(w))
 def add(self,pairs):
  for v,w in pairs:self.put(v,w)
 def finish(self):
  r=bytearray((len(self.bits)+7)//8)
  for i,b in enumerate(self.bits):r[i//8]|=b<<(i%8)
  return bytes(r)
def page(p,s,g,f):
 l=bytes([255]*(len(p)//255)+[len(p)%255]);r=bytearray(b'OggS'+bytes([0,f])+struct.pack('<qII',g,17,s)+bytes(4)+bytes([len(l)])+l+p);crc=0
 for b in r:
  crc^=b<<24
  for _ in range(8):crc=((crc<<1)^(0x04c11db7 if crc&0x80000000 else 0))&0xffffffff
 r[22:26]=struct.pack('<I',crc);return r
b=Bits();b.put(1,8)
for minimum in [0x60100000,0xe0100000]:
 b.add([(0x564342,24),(1,16),(1,24),(0,1),(0,1),(0,5),(2,4),(minimum,32),(0x60100000,32),(0,4),(0,1),(0,1)])
b.add([(0,6),(0,16),(0,6),(0,16),(1,8),(8000,16),(32,16),(4,6),(60,8),(0,4),(0,8),
 (0,6),(1,16),(0,24),(32,24),(31,24),(0,6),(0,8),(1,3),(0,1),(1,8),
 (0,6),(0,16),(0,1),(1,1),(1,8),(0,2),(1,2),(1,2),(2,2),(0,2),(0,8),(0,8),(0,8),
 (0,6),(0,1),(0,16),(0,16),(0,8),(1,1)])
setup=b'\x05vorbis'+b.finish();head=b'\x01vorbis'+struct.pack('<IBIiiiBB',0,3,8000,0,0,0,0x66,1);title=b'title=audiobox synthetic fixture';comment=b'\x03vorbis'+struct.pack('<II',0,1)+struct.pack('<I',len(title))+title+b'\x01'
b=Bits();b.add([(0,1),(8,4),(0,1),(0,1),(0,4),(0,4),(0,3),(0,96)]);audio=b.finish()
packets=[head,comment,setup]+[audio]*4
for name,ps in [('vorbis-coupling-forward',packets),('vorbis-residue-eop',packets[:3]+[audio[:-3]]*4)]:
 encoded=out/(name+'.ogg');encoded.write_bytes(b''.join(page(p,i,0 if i<4 else (i-3)*32,2 if i==0 else 4 if i==6 else 0) for i,p in enumerate(ps)))
 with sf.SoundFile(str(encoded)) as f:
  ref=bytes(f.buffer_read(f.frames,dtype='float32'));print(name,f.frames,f.channels,len(ref),max(abs(v[0]) for v in struct.iter_unpack('<f',ref)))
 vals=list(struct.iter_unpack('<f',ref));ref=b''.join(struct.pack('<f',vals[i+c][0]) for i in range(0,len(vals),3) for c in [0,2,1]);(out/(name+'.f32')).write_bytes(ref)
original=(root/'tests/fixtures/ogg/vorbis-floor0.ogg').read_bytes();ps=[];at=0
while at<len(original):
 n=original[at+26];start=at+27+n;end=start+sum(original[at+27:start]);ps.append(original[start:end]);at=end
ps=ps[:3]+[p[:-2] for p in ps[3:]];encoded=out/'vorbis-eop-mono.ogg';encoded.write_bytes(b''.join(page(p,i,0 if i<4 else (i-3)*32,2 if i==0 else 4 if i==6 else 0) for i,p in enumerate(ps)))
with sf.SoundFile(str(encoded)) as f:
 ref=bytes(f.buffer_read(f.frames,dtype='float32'));print('eop-mono',f.frames,f.channels,len(ref),max(abs(v[0]) for v in struct.iter_unpack('<f',ref)))
(out/'vorbis-eop-mono.f32').write_bytes(ref)

# A valid floor-1 packet can end before all floor values: emit silence + overlap.
data=(out/'vorbis-stereo-q4.ogg').read_bytes();ps=[];packet=bytearray();at=0;gp=0
while at<len(data):
 n=data[at+26];body=at+27+n;gp=struct.unpack_from('<q',data,at+6)[0]
 for length in data[at+27:body]:
  packet.extend(data[body:body+length]);body+=length
  if length<255:ps.append(bytes(packet));packet.clear()
 at=body
ps=ps[:3]+[p[:1] for p in ps[3:]]
encoded=out/'vorbis-floor-eop.ogg'
encoded.write_bytes(b''.join(page(p,i,0 if i<3 else gp if i==len(ps)-1 else -1,2 if i==0 else 4 if i==len(ps)-1 else 0) for i,p in enumerate(ps)))
with sf.SoundFile(str(encoded)) as f:
 ref=bytes(f.buffer_read(f.frames,dtype='float32'));print('floor-eop',f.frames,f.channels)
(out/'vorbis-floor-eop.f32').write_bytes(ref)
def oracle(name,ps,channels,frames):
 encoded=out/(name+'.ogg');encoded.write_bytes(b''.join(page(p,i,0 if i<3 else frames if i+1==len(ps) else -1,2 if i==0 else 4 if i+1==len(ps) else 0) for i,p in enumerate(ps)))
 with sf.SoundFile(str(encoded)) as f:
  ref=bytes(f.buffer_read(f.frames,dtype='float32'));print(name,f.frames,f.channels,len(ref),max(abs(v[0]) for v in struct.iter_unpack('<f',ref)))
 (out/(name+'.f32')).write_bytes(ref)
b=Bits();b.put(0,8)
b.add([(0x564342,24),(1,16),(1,24),(0,1),(0,1),(0,5),(2,4),(0x60100000,32),(0x60100000,32),(0,4),(0,1),(0,1)])
b.add([(0,6),(0,16),(0,6),(1,16),(0,5),(0,2),(6,4),
 (0,6),(1,16),(0,24),(64,24),(31,24),(0,6),(0,8),(1,3),(0,1),(0,8),
 (0,6),(0,16),(0,1),(0,1),(0,2),(0,8),(0,8),(0,8),
 (0,6),(0,1),(0,16),(0,16),(0,8),(1,1)])
setup=b'\x05vorbis'+b.finish();head=b'\x01vorbis'+struct.pack('<IBIiiiBB',0,1,8000,0,0,0,0x76,1);title=b'title=audiobox synthetic fixture';comment=b'\x03vorbis'+struct.pack('<II',0,1)+struct.pack('<I',len(title))+title+b'\x01'
b=Bits();b.add([(0,1),(1,1),(100,8),(200,8),(0,1),(0,32)]);audio=b.finish()
oracle('vorbis-floor1-short-shared',[head,comment,setup]+[audio]*4,1,96)
original=(root/'tests/fixtures/ogg/vorbis-stereo-q4.ogg').read_bytes();ps=[];at=0;packet=bytearray()
while at<len(original):
 n=original[at+26];body=at+27+n;end=body+sum(original[at+27:body]);granule=struct.unpack('<q',original[at+6:at+14])[0]
 for size in original[at+27:body]:
  packet.extend(original[body:body+size]);body+=size
  if size<255:ps.append(bytes(packet));packet.clear()
 at=end
print('stereo audio packet sizes',list(map(len,ps[3:])))
ps=ps[:3]+[p[:-3] if len(p)>20 else p for p in ps[3:]]
oracle('vorbis-residue2-eop-stereo',ps,2,granule)

for count in [256]:
 b=Bits();b.put(count-1,8)
 for _ in range(count): b.add([(0x564342,24),(8192,16),(1,24),(0,1),(0,1),(0,5),(1,4),(0,32),(0,32),(0,4),(0,1),(0,1)])
 b.add([(0,6),(0,16),(0,6),(0,16),(1,8),(8000,16),(32,16),(4,6),(60,8),(0,4),(0,8),(0,6),(1,16),(0,24),(32,24),(31,24),(0,6),(0,8),(0,3),(0,1),(0,6),(0,16),(0,1),(0,1),(0,2),(0,8),(0,8),(0,8),(0,6),(0,1),(0,16),(0,16),(0,8),(1,1)])
 setup=b'\x05vorbis'+b.finish();head=b'\x01vorbis'+struct.pack('<IBIiiiBB',0,1,8000,0,0,0,0x66,1);title=b'title=audiobox synthetic fixture';comment=b'\x03vorbis'+struct.pack('<II',0,1)+struct.pack('<I',len(title))+title+b'\x01';ps=[head,comment,setup]+[bytes(1)]*4
 encoded=out/'vorbis-lookup-work.ogg';encoded.write_bytes(b''.join(page(p,i,0 if i<4 else (i-3)*32,2 if i==0 else 4 if i==6 else 0) for i,p in enumerate(ps)))
 print(encoded,len(encoded.read_bytes()))

with sf.SoundFile(str(encoded)) as f:
 ref=bytes(f.buffer_read(f.frames,dtype='float32'));assert len(ref)==96*4
(out/'vorbis-lookup-work.f32').write_bytes(ref)

import hashlib
manifest=out/'vorbis.csv';rows=manifest.read_text().splitlines();names=['vorbis-coupling-forward','vorbis-residue-eop','vorbis-eop-mono','vorbis-floor-eop','vorbis-floor1-short-shared','vorbis-residue2-eop-stereo','vorbis-lookup-work'];rows=[r for r in rows if r.split(',')[0] not in names]
for name in names:
 encoded=out/(name+'.ogg');reference=out/(name+'.f32')
 with sf.SoundFile(str(encoded)) as f:rate,channels,frames=f.samplerate,f.channels,f.frames
 rows.append(f'{name},{rate},{channels},{frames},{frames},{hashlib.sha256(encoded.read_bytes()).hexdigest()},{hashlib.sha256(reference.read_bytes()).hexdigest()}')
manifest.write_text('\n'.join(rows)+'\n',newline='\n')
