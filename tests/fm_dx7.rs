#![cfg(feature = "synth")]
//! DX7 voice data. Every fixture here is generated; no third-party patch is used.
use audiobox::ErrorCode;
use audiobox::synth::*;

struct Lcg(u64);
impl Lcg {
    fn below(&mut self, limit: u8) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // Both ends of each range come up often.
        match (self.0 >> 40) % 6 {
            0 => 0,
            1 => limit,
            _ => ((self.0 >> 33) % (limit as u64 + 1)) as u8,
        }
    }
}

/// Maximum of each of the 155 single-voice parameter bytes, from the format's table.
fn limits() -> [u8; 155] {
    let mut max = [99u8; 155];
    for op in 0..6 {
        let o = op * 21;
        (max[o + 11], max[o + 12], max[o + 13]) = (3, 3, 7);
        (max[o + 14], max[o + 15]) = (3, 7);
        (max[o + 17], max[o + 18], max[o + 20]) = (1, 31, 14);
    }
    (max[134], max[135], max[136]) = (31, 7, 1);
    (max[141], max[142], max[143], max[144]) = (1, 5, 7, 48);
    max
}
/// A voice with a distinct, in-range value in every field.
fn parameters(seed: u64, name: &str) -> [u8; 155] {
    let mut r = Lcg(seed);
    let max = limits();
    let mut v: [u8; 155] = std::array::from_fn(|i| r.below(max[i]));
    for op in 0..6 {
        // Fixed mode keeps only the two low bits of the coarse value.
        if v[op * 21 + 17] == 1 {
            v[op * 21 + 18] &= 3;
        }
    }
    v[145..155].copy_from_slice(format!("{name:<10}").as_bytes());
    v
}
fn checksum(data: &[u8]) -> u8 {
    (128 - data.iter().map(|&b| b as u32).sum::<u32>() % 128) as u8 & 127
}
fn single(v: &[u8; 155]) -> Vec<u8> {
    let mut out = vec![0xf0, 0x43, 0x00, 0x00, 0x01, 0x1b];
    out.extend(v);
    out.push(checksum(v));
    out.push(0xf7);
    out
}
/// Packs a voice into the bank's 128 bytes, written from the format's table.
fn pack(v: &[u8; 155]) -> Vec<u8> {
    let mut out = Vec::new();
    for op in 0..6 {
        let o = &v[op * 21..];
        out.extend(&o[..11]);
        out.push(o[11] + o[12] * 4);
        out.push(o[13] + o[20] * 8);
        out.push(o[14] + o[15] * 4);
        out.push(o[16]);
        out.push(o[17] + o[18] * 2);
        out.push(o[19]);
    }
    out.extend(&v[126..135]);
    out.push(v[135] + v[136] * 8);
    out.extend(&v[137..141]);
    out.push(v[141] + v[142] * 2 + v[143] * 16);
    out.extend(&v[144..155]);
    assert_eq!(out.len(), 128);
    out
}
fn bank(voices: &[[u8; 155]]) -> Vec<u8> {
    let data: Vec<u8> = voices.iter().flat_map(pack).collect();
    let mut out = vec![0xf0, 0x43, 0x00, 0x09, 0x20, 0x00];
    out.extend(&data);
    out.push(checksum(&data));
    out.push(0xf7);
    out
}
fn generated_bank() -> Vec<[u8; 155]> {
    (0..32)
        .map(|i| parameters(1000 + i, &format!("GEN {i:02}-x")))
        .collect()
}

/// Checks a patch against the parameter bytes it came from, field by field.
fn check(patch: &FmPatch, v: &[u8; 155]) {
    let voice = &patch.voice;
    for number in 1..=6usize {
        let b = &v[(6 - number) * 21..];
        let o = voice.operators[number - 1].unwrap();
        assert_eq!(o.envelope.rates, b[0..4]);
        assert_eq!(o.envelope.levels, b[4..8]);
        let s = o.scaling;
        assert_eq!(
            (s.break_point, s.left_depth, s.right_depth),
            (b[8], b[9], b[10])
        );
        assert_eq!((s.left_curve as u8, s.right_curve as u8), (b[11], b[12]));
        assert_eq!(
            (o.rate_scaling, o.amplitude_sensitivity, o.velocity),
            (b[13], b[14], b[15])
        );
        assert_eq!(o.level, b[16]);
        let expected = if b[17] == 0 {
            FmFrequency::Ratio {
                coarse: b[18],
                fine: b[19],
            }
        } else {
            FmFrequency::Fixed {
                decade: b[18] & 3,
                fine: b[19],
            }
        };
        assert_eq!(o.frequency, expected);
        assert_eq!(o.detune, b[20] as i8 - 7);
    }
    assert_eq!(voice.pitch_envelope.rates, v[126..130]);
    assert_eq!(voice.pitch_envelope.levels, v[130..134]);
    assert_eq!((voice.algorithm, voice.feedback), (v[134] + 1, v[135]));
    assert_eq!(voice.key_sync, v[136] == 1);
    let l = voice.lfo;
    assert_eq!(
        (l.speed, l.delay, l.pitch_depth, l.amplitude_depth),
        (v[137], v[138], v[139], v[140])
    );
    assert_eq!(
        (l.key_sync, l.wave as u8, l.pitch_sensitivity),
        (v[141] == 1, v[142], v[143])
    );
    assert_eq!(voice.transpose, v[144] as i8 - 24);
    // Nothing else is set by an import.
    let d = Fm::default();
    assert_eq!(
        (voice.voices, voice.unison, voice.pan),
        (d.voices, d.unison, d.pan)
    );
    assert_eq!(
        (voice.glide_ms, voice.legato, voice.oversample),
        (0, false, 1)
    );
    assert_eq!((l.beats, l.seed, patch.volume), (None, 1, 15));
}

#[test]
fn a_generated_bank_reads_field_by_field_and_writes_back_identically() {
    let voices = generated_bank();
    let bytes = bank(&voices);
    assert_eq!(bytes.len(), DX7_BANK_BYTES);
    let read = read_dx7(&bytes).unwrap();
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    assert_eq!(read.voices.len(), DX7_BANK_VOICES);
    for (index, (voice, v)) in read.voices.iter().zip(&voices).enumerate() {
        check(&voice.patch, v);
        assert_eq!(voice.original_name, format!("GEN {index:02}-x  "));
        assert_eq!(voice.patch.name, format!("GEN_{index:02}-x"));
    }
    // Through text and back to the same bytes.
    let patches: Vec<FmPatch> = read
        .voices
        .iter()
        .map(|v| {
            let text = v.patch.to_string();
            let parsed = FmPatch::parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
            assert_eq!(parsed, v.patch);
            parsed
        })
        .collect();
    assert!(write_dx7_bank(&patches).unwrap() == bytes);
    // A voice on its own.
    for (voice, v) in read.voices.iter().zip(&voices).step_by(5) {
        let message = single(v);
        assert_eq!(message.len(), DX7_VOICE_BYTES);
        let one = read_dx7(&message).unwrap();
        assert!(one.warnings.is_empty());
        assert_eq!(one.voices.len(), 1);
        assert_eq!(one.voices[0], *voice);
        assert!(write_dx7_voice(&voice.patch).unwrap() == message);
    }
}

#[test]
fn structural_faults_are_errors_and_minor_faults_are_warnings() {
    let voices = generated_bank();
    let good = bank(&voices);
    let code = |bytes: &[u8]| read_dx7(bytes).unwrap_err().code;
    // A message cut short, or empty data, is an error.
    assert_eq!(code(&good[..good.len() - 1]), ErrorCode::DecodeError);
    assert_eq!(code(&good[..163]), ErrorCode::DecodeError);
    assert_eq!(code(&[]), ErrorCode::DecodeError);
    // A stray byte after the bank is ignored with a warning.
    let read = read_dx7(&[good.clone(), vec![0xf7]].concat()).unwrap();
    assert_eq!(read.voices, read_dx7(&good).unwrap().voices);
    assert_eq!(read.warnings.len(), 1);
    assert!(
        read.warnings[0].message.contains("1 byte outside"),
        "{:?}",
        read.warnings
    );
    // A message that is not DX7 voice data leaves nothing to read.
    for (index, value) in [(0, 0xf1), (1, 0x42), (2, 0x10), (3, 0x01)] {
        let mut bad = good.clone();
        bad[index] = value;
        assert_eq!(
            code(&bad),
            ErrorCode::UnsupportedFormat,
            "header byte {index}"
        );
    }
    // A bank whose format byte says single voice is a voice of the wrong length.
    let mut as_voice = good.clone();
    as_voice[3] = 0x00;
    assert_eq!(code(&as_voice), ErrorCode::DecodeError);
    // A bank whose byte count disagrees with its 4096 data bytes reads, with a warning.
    for (index, value) in [(4, 0x21), (5, 0x01)] {
        let mut odd = good.clone();
        odd[index] = value;
        let read = read_dx7(&odd).unwrap();
        assert_eq!(read.voices, read_dx7(&good).unwrap().voices, "byte {index}");
        assert!(read.warnings[0].message.contains("data bytes"));
    }
    let mut bad = good.clone();
    *bad.last_mut().unwrap() = 0x00;
    assert_eq!(code(&bad), ErrorCode::DecodeError);
    // Any channel in the low half of the third byte is accepted.
    let mut channel = good.clone();
    channel[2] = 0x0f;
    assert!(read_dx7(&channel).unwrap().warnings.is_empty());

    // A wrong checksum still reads, with one warning for the file.
    let mut damaged = good.clone();
    let at = damaged.len() - 2;
    damaged[at] ^= 0x15;
    let read = read_dx7(&damaged).unwrap();
    assert_eq!(read.voices, read_dx7(&good).unwrap().voices);
    assert_eq!(read.warnings.len(), 1);
    assert_eq!(read.warnings[0].voice, None);
    assert!(read.warnings[0].message.contains("Checksum"));

    // Bytes above their range are clamped and reported against their voice.
    let mut v = parameters(7, "CLAMPED");
    (v[16], v[20], v[134], v[144], v[142]) = (120, 15, 40, 60, 7);
    v[2 * 21] = 127;
    let read = read_dx7(&single(&v)).unwrap();
    let patch = &read.voices[0].patch;
    assert_eq!(patch.voice.operators[5].unwrap().level, 99);
    assert_eq!(patch.voice.operators[5].unwrap().detune, 7);
    assert_eq!(patch.voice.operators[3].unwrap().envelope.rates[0], 99);
    assert_eq!((patch.voice.algorithm, patch.voice.transpose), (32, 24));
    assert_eq!(patch.voice.lfo.wave, FmLfoWave::SampleHold);
    assert_eq!(read.warnings.len(), 6);
    assert!(read.warnings.iter().all(|w| w.voice == Some(0)));
    let has = |text: &str| read.warnings.iter().any(|w| w.message.contains(text));
    assert!(has("operator 4 envelope rate 127 is above 99"));
    assert!(has("operator 6 output level 120") && has("algorithm 40") && has("LFO wave 7"));
    assert!(patch.validate().is_ok());

    // Fixed mode stores the reduced decade, so this one field does not round-trip.
    let mut v = parameters(8, "FIXED");
    (v[17], v[18]) = (1, 30);
    let read = read_dx7(&single(&v)).unwrap();
    assert!(read.warnings.is_empty());
    let op6 = read.voices[0].patch.voice.operators[5].unwrap();
    assert!(matches!(
        op6.frequency,
        FmFrequency::Fixed { decade: 2, .. }
    ));
    assert_eq!(write_dx7_voice(&read.voices[0].patch).unwrap()[6 + 18], 2);
}

#[test]
fn names_are_sanitized_and_unique() {
    let mut voices = generated_bank();
    for (index, name) in [
        "E.PIANO 1",
        "E.PIANO 1",
        "E.PIANO 1",
        "",
        "  BASS",
        "caf\u{e9}",
    ]
    .iter()
    .enumerate()
    {
        let mut bytes = [b' '; 10];
        for (slot, c) in bytes.iter_mut().zip(name.chars()) {
            *slot = if c.is_ascii() { c as u8 } else { 0xe9 };
        }
        voices[index][145..155].copy_from_slice(&bytes);
    }
    let read = read_dx7(&bank(&voices)).unwrap();
    let names: Vec<&str> = read.voices[..6]
        .iter()
        .map(|v| v.patch.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "E-PIANO_1",
            "E-PIANO_1-2",
            "E-PIANO_1-3",
            "voice",
            "BASS",
            "caf-"
        ]
    );
    assert_eq!(read.voices[0].original_name, "E.PIANO 1 ");
    assert_eq!(read.voices[5].original_name, "caf?      ");
    for voice in &read.voices {
        assert!(voice.patch.validate().is_ok());
        assert_eq!(
            FmPatch::parse(&voice.patch.to_string()).unwrap(),
            voice.patch
        );
    }
}

#[test]
fn writing_refuses_what_the_format_cannot_hold() {
    let patch = FmPatch::parse("fm tone\nop 1\nend").unwrap();
    let bytes = write_dx7_voice(&patch).unwrap();
    let back = &read_dx7(&bytes).unwrap().voices[0];
    assert_eq!(back.original_name, "tone      ");
    // Operators that were off come back at level 0, which sounds the same.
    assert_eq!(back.patch.voice.operators[0], patch.voice.operators[0]);
    assert_eq!(back.patch.voice.operators[3].unwrap().level, 0);
    assert_eq!(
        FmPatch::parse("fm my_tone\nop 1\nend")
            .map(|p| write_dx7_voice(&p).unwrap()[151..161].to_vec())
            .unwrap(),
        b"my tone   "
    );

    for (header, field) in [
        ("unison=2", "unison"),
        ("pan=10", "pan"),
        ("voices=1 glide=20", "glide"),
        ("voices=1 legato=on", "legato"),
        ("oversample=2", "oversample"),
        ("volume=9", "volume"),
        (
            "spread=20 unison=2 unison-detune=5",
            "unison, unison-detune, spread",
        ),
    ] {
        let patch = FmPatch::parse(&format!("fm a {header}\nop 1\nend")).unwrap();
        let e = write_dx7_voice(&patch).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidArgument);
        assert!(e.message.contains(field), "{}", e.message);
    }
    let synced = FmPatch::parse("fm a\nlfo beats=1,4 seed=3\nop 1\nend").unwrap();
    let message = write_dx7_voice(&synced).unwrap_err().message;
    assert!(message.contains("lfo beats") && message.contains("lfo seed"));
    let long = FmPatch::parse("fm elevenchars\nop 1\nend").unwrap();
    assert!(write_dx7_voice(&long).is_err());
    // Voice count is not part of the format and is simply not stored.
    let poly = FmPatch::parse("fm a voices=3\nop 1\nend").unwrap();
    assert!(write_dx7_voice(&poly).is_ok());

    assert_eq!(
        write_dx7_bank(std::slice::from_ref(&patch))
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    let full = vec![patch; 32];
    let bank = write_dx7_bank(&full).unwrap();
    assert_eq!(bank.len(), DX7_BANK_BYTES);
    // Equal names in a bank are told apart on reading.
    let read = read_dx7(&bank).unwrap();
    assert_eq!(read.voices[31].patch.name, "tone-32");
}

#[test]
fn arbitrary_bytes_never_panic_and_imports_render() {
    let mut r = Lcg(99);
    for round in 0..400 {
        let length = match round % 4 {
            0 => DX7_VOICE_BYTES,
            1 => DX7_BANK_BYTES,
            _ => (r.below(255) as usize) * (1 + r.below(20) as usize),
        };
        let mut bytes: Vec<u8> = (0..length).map(|_| r.below(255)).collect();
        if round % 2 == 0 && length > 8 {
            // A valid frame around random content reaches the field decoder.
            let header: &[u8] = if length == DX7_BANK_BYTES {
                &[0xf0, 0x43, 0, 9, 0x20, 0]
            } else {
                &[0xf0, 0x43, 0, 0, 1, 0x1b]
            };
            bytes[..6].copy_from_slice(header);
            *bytes.last_mut().unwrap() = 0xf7;
        }
        if let Ok(read) = read_dx7(&bytes) {
            for voice in &read.voices {
                voice.patch.validate().unwrap();
            }
        }
    }
    // Every generated voice plays through the score path.
    let read = read_dx7(&bank(&generated_bank())).unwrap();
    for voice in read.voices.iter().step_by(4) {
        let text = format!(
            "tempo 240\n{}channel a=@{} o4 l8 c!40 g!120",
            voice.patch, voice.patch.name
        );
        let audio = Score::parse(&text).unwrap().render_stereo(24000).unwrap();
        assert!(audio.channel_data(0).unwrap().iter().all(|x| x.is_finite()));
    }
}

#[test]
fn a_bank_with_a_wrong_byte_count_reads_with_a_warning() {
    // Old editors wrote `10 00` (2048) although all 4096 data bytes are there.
    let voices = generated_bank();
    let mut odd = bank(&voices);
    (odd[4], odd[5]) = (0x10, 0x00);
    let read = read_dx7(&odd).unwrap();
    assert_eq!(read.voices.len(), DX7_BANK_VOICES);
    for (voice, v) in read.voices.iter().zip(&voices) {
        check(&voice.patch, v);
    }
    assert_eq!(read.warnings.len(), 1);
    assert_eq!(read.warnings[0].voice, None);
    assert!(
        read.warnings[0].message.contains("2048"),
        "{:?}",
        read.warnings
    );
}

#[test]
fn raw_banks_read_with_their_own_function() {
    let voices = generated_bank();
    let wrapped = bank(&voices);
    let raw = &wrapped[6..6 + DX7_RAW_BANK_BYTES];
    let read = read_dx7_raw_bank(raw).unwrap();
    assert!(read.warnings.is_empty());
    assert_eq!(read.voices, read_dx7(&wrapped).unwrap().voices);
    // read_dx7 cannot recognise one, and says where to look.
    let err = read_dx7(raw).unwrap_err();
    assert_eq!(err.code, ErrorCode::UnsupportedFormat);
    assert!(err.message.contains("read_dx7_raw_bank"));
    // Wrong lengths and eight-bit bytes are not raw banks.
    assert_eq!(
        read_dx7_raw_bank(&raw[1..]).unwrap_err().code,
        ErrorCode::DecodeError
    );
    let mut high = raw.to_vec();
    high[100] = 0x80;
    assert_eq!(
        read_dx7_raw_bank(&high).unwrap_err().code,
        ErrorCode::DecodeError
    );
}

#[test]
fn every_voice_message_of_a_file_is_read() {
    let first = generated_bank();
    let second: Vec<[u8; 155]> = (0..32)
        .map(|i| parameters(2000 + i, &format!("TWO {i:02}")))
        .collect();
    // Two banks saved together: 64 voices, in order.
    let both = [bank(&first), bank(&second)].concat();
    assert_eq!(both.len(), 2 * DX7_BANK_BYTES);
    let read = read_dx7(&both).unwrap();
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    assert_eq!(read.voices.len(), 64);
    for (voice, v) in read.voices.iter().zip(first.iter().chain(&second)) {
        check(&voice.patch, v);
    }
    // The same bank twice: names stay unique.
    let twice = read_dx7(&[bank(&first), bank(&first)].concat()).unwrap();
    assert_eq!(
        twice.voices[32].patch.name,
        format!("{}-2", twice.voices[0].patch.name)
    );
    // A clamp warning names the voice by its place in the whole file.
    let mut v = parameters(9, "LATE");
    v[16] = 120;
    let read = read_dx7(&[single(&first[0]), single(&v)].concat()).unwrap();
    assert_eq!(read.voices.len(), 2);
    assert_eq!(read.warnings[0].voice, Some(1));
}

#[test]
fn other_messages_and_stray_bytes_are_skipped_with_warnings() {
    let voices = generated_bank();
    let good = bank(&voices);
    let expected = read_dx7(&good).unwrap().voices;
    // A bank followed by trailing data that is not a message.
    let trailing = [good.clone(), vec![0x00; 1128]].concat();
    let read = read_dx7(&trailing).unwrap();
    assert_eq!(read.voices, expected);
    assert!(read.warnings[0].message.contains("1128 bytes outside"));
    // A bank and a performance message of another instrument: the bank is read.
    let mut performance = vec![0xf0, 0x43, 0x00, 0x7e, 0x01, 0x28];
    performance.extend(std::iter::repeat_n(0x11, 168));
    performance.extend([0x00, 0xf7]);
    let read = read_dx7(&[performance.clone(), good.clone()].concat()).unwrap();
    assert_eq!(read.voices, expected);
    assert_eq!(read.warnings.len(), 1);
    assert!(
        read.warnings[0]
            .message
            .contains("not DX7 voice data (176 bytes)")
    );
    // A single voice with two extra bytes.
    let one = single(&voices[3]);
    let read = read_dx7(&[one.clone(), vec![0x00, 0x00]].concat()).unwrap();
    assert_eq!(read.voices, read_dx7(&one).unwrap().voices);
    // A file of other messages only is not voice data.
    assert_eq!(
        read_dx7(&performance).unwrap_err().code,
        ErrorCode::UnsupportedFormat
    );
    // A message cut short by the start of the next one is an error.
    let cut = [good[..2000].to_vec(), good.clone()].concat();
    assert_eq!(read_dx7(&cut).unwrap_err().code, ErrorCode::DecodeError);
    // A voice or bank message of the wrong length is an error.
    let short = [good[..4000].to_vec(), vec![0x00, 0xf7]].concat();
    assert_eq!(read_dx7(&short).unwrap_err().code, ErrorCode::DecodeError);
}

#[test]
fn review_cases() {
    let voices = generated_bank();
    // A damaged checksum byte that reads F7 is a bad checksum, not the end.
    let mut one = single(&voices[0]);
    let at = one.len() - 2;
    one[at] = 0xf7;
    let read = read_dx7(&one).unwrap();
    assert_eq!(read.voices.len(), 1);
    assert!(read.warnings[0].message.contains("Checksum"));
    // A bank header in a frame too short for a bank is a structural fault.
    let short = [
        bank(&voices),
        vec![0xf0, 0x43, 0x00, 0x09, 0x20, 0x00, 0xf7],
    ]
    .concat();
    assert_eq!(read_dx7(&short).unwrap_err().code, ErrorCode::DecodeError);
    // Many voices of one name are named quickly and uniquely.
    let same: Vec<[u8; 155]> = (0..32).map(|i| parameters(3000 + i, "SAME")).collect();
    let many: Vec<u8> = (0..128).flat_map(|_| bank(&same)).collect();
    let start = std::time::Instant::now();
    let read = read_dx7(&many).unwrap();
    assert_eq!(read.voices.len(), 4096);
    let names: std::collections::HashSet<&str> =
        read.voices.iter().map(|v| v.patch.name.as_str()).collect();
    assert_eq!(names.len(), 4096);
    assert!(start.elapsed() < std::time::Duration::from_secs(30));
}

#[test]
fn review_cases_two() {
    let voices = generated_bank();
    // A short bank followed by more data and F7 is a short bank, not 32 voices.
    let mut short = vec![0xf0, 0x43, 0x00, 0x09, 0x10, 0x00];
    short.extend(std::iter::repeat_n(0, 2048));
    short.extend([0x00, 0xf7]);
    short.extend(std::iter::repeat_n(0, 2047));
    short.push(0xf7);
    assert_eq!(short.len(), 4104);
    assert_eq!(read_dx7(&short).unwrap_err().code, ErrorCode::DecodeError);
    // A message that is not DX7 data and holds the start of another is cut short.
    let cut = [vec![0xf0, 0x7d, 0xf0, 0xf7], single(&voices[0])].concat();
    assert_eq!(read_dx7(&cut).unwrap_err().code, ErrorCode::DecodeError);
    // A file that is one standard message reads as it always has, whatever its
    // data bytes hold: a level of F0 is clamped.
    let mut one = single(&voices[0]);
    one[6 + 16] = 0xf0;
    let at = one.len() - 2;
    one[at] = checksum(&one[6..at]);
    let read = read_dx7(&one).unwrap();
    assert_eq!(read.voices.len(), 1);
    assert!(!read.warnings.is_empty());
}

#[test]
fn a_truncated_dx7_header_is_a_fault() {
    let voices = generated_bank();
    for head in [
        vec![0xf0, 0x43, 0xf7],
        vec![0xf0, 0x43, 0x00, 0xf7],
        vec![0xf0, 0x43, 0x00, 0x09, 0xf7],
        vec![0xf0, 0x43, 0x00, 0x09, 0x20, 0xf7],
        vec![0xf0, 0x43, 0x00, 0x00, 0x01, 0xf7],
    ] {
        let file = [head, bank(&voices)].concat();
        assert_eq!(read_dx7(&file).unwrap_err().code, ErrorCode::DecodeError);
    }
    // A short message of another Yamaha format is skipped.
    let other = [vec![0xf0, 0x43, 0x00, 0x7e, 0xf7], bank(&voices)].concat();
    assert_eq!(read_dx7(&other).unwrap().voices.len(), 32);
}

#[test]
fn a_voice_with_a_wrong_count_is_still_a_voice() {
    let voices = generated_bank();
    // Short: a fault, not another message.
    let mut short = vec![0xf0, 0x43, 0x00, 0x00, 0x01, 0x1a];
    short.extend(&voices[0][..154]);
    short.push(checksum(&voices[0][..154]));
    short.push(0xf7);
    let file = [short, bank(&voices)].concat();
    assert_eq!(read_dx7(&file).unwrap_err().code, ErrorCode::DecodeError);
    // Of the right length: read with a warning.
    let mut one = single(&voices[0]);
    one[5] = 0x1a;
    let read = read_dx7(&one).unwrap();
    assert_eq!(read.voices.len(), 1);
    assert!(
        read.warnings
            .iter()
            .any(|w| w.message.contains("says it holds"))
    );
}
