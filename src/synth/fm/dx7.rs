//! DX7 voice data: single voices and 32-voice banks, to and from [`FmPatch`].
//!
//! This is file conversion only. Byte layouts are in `docs/fm-voice.md`.
use super::{
    Fm, FmCurve, FmEnvelope, FmFrequency, FmLfo, FmLfoWave, FmOperator, FmPatch, FmScaling,
};
use std::collections::HashSet;

use crate::{Error, ErrorCode, Result};

/// Bytes of a single-voice message: 155 parameters in a 6-byte header, a checksum
/// and a terminator.
pub const DX7_VOICE_BYTES: usize = 163;
/// Bytes of a 32-voice bank message: 32 packed voices of 128 bytes each.
pub const DX7_BANK_BYTES: usize = 4104;
/// Voices in a bank.
pub const DX7_BANK_VOICES: usize = 32;

const VOICE_HEADER: [u8; 4] = [0x00, 0x00, 0x01, 0x1b];
const BANK_HEADER: [u8; 4] = [0x00, 0x09, 0x20, 0x00];

/// Something the reader accepted but changed or could not verify.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dx7Warning {
    /// Zero-based voice in the file, or `None` for the file as a whole.
    pub voice: Option<usize>,
    pub message: String,
}

/// One voice read from DX7 data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dx7Voice {
    /// The voice, named by a sanitized, unique form of `original_name`.
    pub patch: FmPatch,
    /// The ten name characters as stored, with other than printable ASCII as `?`.
    pub original_name: String,
}

/// The voices of a DX7 file, in file order, and what the reader had to tolerate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dx7Voices {
    pub voices: Vec<Dx7Voice>,
    pub warnings: Vec<Dx7Warning>,
}

fn checksum(data: &[u8]) -> u8 {
    let sum = data.iter().fold(0u8, |s, &b| s.wrapping_add(b & 0x7f));
    sum.wrapping_neg() & 0x7f
}

/// A field reader that clamps out-of-range bytes and records that it did.
struct Fields<'a> {
    voice: usize,
    warnings: &'a mut Vec<Dx7Warning>,
}
impl Fields<'_> {
    fn get(&mut self, value: u8, max: u8, what: &str) -> u8 {
        if value > max {
            self.warnings.push(Dx7Warning {
                voice: Some(self.voice),
                message: format!("{what} {value} is above {max} and was clamped"),
            });
            max
        } else {
            value
        }
    }
    fn four(&mut self, bytes: &[u8], what: &str) -> [u8; 4] {
        std::array::from_fn(|i| self.get(bytes[i], 99, what))
    }
}

fn curve(index: u8) -> FmCurve {
    FmCurve::NAMES[index as usize].1
}

/// Reads the 155 parameter bytes of an unpacked voice.
fn voice(data: &[u8], fields: &mut Fields) -> (Fm, String) {
    let mut operators = [None; 6];
    for (block, bytes) in data[..126].chunks_exact(21).enumerate() {
        // The data stores operator 6 first.
        let number = 6 - block;
        let op = |what: &str| format!("operator {number} {what}");
        let fine = fields.get(bytes[19], 99, &op("fine frequency"));
        let coarse = fields.get(bytes[18], 31, &op("coarse frequency"));
        let frequency = match fields.get(bytes[17], 1, &op("mode")) {
            0 => FmFrequency::Ratio { coarse, fine },
            _ => FmFrequency::Fixed {
                decade: coarse & 3,
                fine,
            },
        };
        operators[number - 1] = Some(FmOperator {
            frequency,
            detune: fields.get(bytes[20], 14, &op("detune")) as i8 - 7,
            level: fields.get(bytes[16], 99, &op("output level")),
            envelope: FmEnvelope {
                rates: fields.four(&bytes[0..4], &op("envelope rate")),
                levels: fields.four(&bytes[4..8], &op("envelope level")),
            },
            scaling: FmScaling {
                break_point: fields.get(bytes[8], 99, &op("break point")),
                left_depth: fields.get(bytes[9], 99, &op("left depth")),
                right_depth: fields.get(bytes[10], 99, &op("right depth")),
                left_curve: curve(fields.get(bytes[11], 3, &op("left curve"))),
                right_curve: curve(fields.get(bytes[12], 3, &op("right curve"))),
            },
            rate_scaling: fields.get(bytes[13], 7, &op("rate scaling")),
            amplitude_sensitivity: fields.get(bytes[14], 3, &op("amplitude sensitivity")),
            velocity: fields.get(bytes[15], 7, &op("velocity sensitivity")),
        });
    }
    let g = &data[126..];
    let fm = Fm {
        pitch_envelope: FmEnvelope {
            rates: fields.four(&g[0..4], "pitch envelope rate"),
            levels: fields.four(&g[4..8], "pitch envelope level"),
        },
        algorithm: fields.get(g[8], 31, "algorithm") + 1,
        feedback: fields.get(g[9], 7, "feedback"),
        key_sync: fields.get(g[10], 1, "oscillator key sync") == 1,
        lfo: FmLfo {
            speed: fields.get(g[11], 99, "LFO speed"),
            delay: fields.get(g[12], 99, "LFO delay"),
            pitch_depth: fields.get(g[13], 99, "LFO pitch depth"),
            amplitude_depth: fields.get(g[14], 99, "LFO amplitude depth"),
            key_sync: fields.get(g[15], 1, "LFO key sync") == 1,
            wave: FmLfoWave::NAMES[fields.get(g[16], 5, "LFO wave") as usize].1,
            pitch_sensitivity: fields.get(g[17], 7, "pitch sensitivity"),
            ..FmLfo::default()
        },
        transpose: fields.get(g[18], 48, "transpose") as i8 - 24,
        operators,
        ..Fm::default()
    };
    let name = g[19..29]
        .iter()
        .map(|&b| {
            if (32..127).contains(&b) {
                b as char
            } else {
                '?'
            }
        })
        .collect();
    (fm, name)
}

/// Expands a 128-byte packed bank voice into the 155-byte single-voice layout.
fn unpack(packed: &[u8]) -> [u8; 155] {
    let mut out = [0; 155];
    for op in 0..6 {
        let (p, o) = (&packed[op * 17..], op * 21);
        out[o..o + 11].copy_from_slice(&p[..11]);
        out[o + 11] = p[11] & 3;
        out[o + 12] = (p[11] >> 2) & 3;
        out[o + 13] = p[12] & 7;
        out[o + 20] = (p[12] >> 3) & 15;
        out[o + 14] = p[13] & 3;
        out[o + 15] = (p[13] >> 2) & 7;
        out[o + 16] = p[14];
        out[o + 17] = p[15] & 1;
        out[o + 18] = (p[15] >> 1) & 31;
        out[o + 19] = p[16];
    }
    out[126..135].copy_from_slice(&packed[102..111]);
    out[135] = packed[111] & 7;
    out[136] = (packed[111] >> 3) & 1;
    out[137..141].copy_from_slice(&packed[112..116]);
    out[141] = packed[116] & 1;
    out[142] = (packed[116] >> 1) & 7;
    out[143] = (packed[116] >> 4) & 7;
    out[144..155].copy_from_slice(&packed[117..128]);
    out
}

/// A name of ASCII letters, digits, `_` and `-`: spaces inside become `_`, the rest `-`.
fn sanitize(original: &str) -> String {
    let name: String = original
        .trim()
        .chars()
        .map(|c| match c {
            c if c.is_ascii_alphanumeric() || c == '_' || c == '-' => c,
            ' ' => '_',
            _ => '-',
        })
        .collect();
    if name.is_empty() {
        "voice".into()
    } else {
        name
    }
}

/// Bytes of a 32-voice bank without its message wrapper: what some editors save.
pub const DX7_RAW_BANK_BYTES: usize = 4096;

/// What a system-exclusive message of a file holds.
enum Message<'a> {
    /// A single voice: its 155 parameter bytes.
    Voice(&'a [u8]),
    /// A 32-voice bank: its 4096 packed bytes.
    Bank(&'a [u8]),
}

/// Reads DX7 voice data: a single voice (163 bytes), a 32-voice bank (4104 bytes),
/// or a file of several such messages, such as two banks saved together.
///
/// Every DX7 voice and bank message of the file is read, in order. Minor faults are
/// accepted with a [`Dx7Warning`]: a wrong checksum, parameter bytes above their
/// range (clamped), a voice or bank whose byte count disagrees with its data bytes,
/// bytes outside any message, and messages that hold something else, such as a
/// TX802 performance, which are skipped. Structural faults are errors:
/// [`ErrorCode::DecodeError`] for a message cut short or a voice or bank message of
/// the wrong length, and [`ErrorCode::UnsupportedFormat`] for a file with no DX7
/// voice data at all. A bank saved without its wrapper has no header to recognise
/// it by; read it with [`read_dx7_raw_bank`].
pub fn read_dx7(bytes: &[u8]) -> Result<Dx7Voices> {
    if bytes.is_empty() {
        return Err(Error::decode("DX7 voice data is empty"));
    }
    let mut warnings = Vec::new();
    let mut messages = Vec::new();
    let mut stray = 0usize;
    let mut skipped = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != 0xf0 {
            stray += 1;
            at += 1;
            continue;
        }
        let Some(mut length) = bytes[at..].iter().position(|&b| b == 0xf7).map(|i| i + 1) else {
            return Err(Error::decode(format!(
                "A message starting at byte {at} does not end with 0xF7"
            )));
        };
        let known = dx7_length(&bytes[at..]);
        // A file that is exactly one message with the standard header is read as it
        // always was, whatever its data bytes hold: a damaged byte is clamped.
        let whole = at == 0
            && known == Some(bytes.len())
            && bytes[4..6] == standard_count(bytes[3])
            && bytes.last() == Some(&0xf7);
        if whole {
            length = bytes.len();
        } else if let Some(full) = known {
            // A DX7 message whose checksum byte reads F7, just before its final F7, is
            // a bad checksum, not the end of the message. An F7 anywhere else ends it.
            if length == full - 1 && bytes.get(at + full - 1) == Some(&0xf7) {
                length = full;
            }
        }
        // A new message before the end of this one: this one was cut short. The
        // checksum byte of a DX7 message, just before F7, may be damaged and is not
        // looked at.
        let body = &bytes[at + 1..(at + length - 1).max(at + 1)];
        let checked = if whole {
            &[][..]
        } else if known.is_some() && length > 2 {
            &body[..body.len() - 1]
        } else {
            body
        };
        if let Some(inner) = checked.iter().position(|&b| b == 0xf0) {
            return Err(Error::decode(format!(
                "The message starting at byte {at} is cut short at byte {}",
                at + 1 + inner
            )));
        }
        let message = &bytes[at..at + length];
        match classify(message, at, &mut warnings)? {
            Some(m) => messages.push(m),
            None => skipped.push(length),
        }
        at += length;
    }
    if messages.is_empty() {
        let hint = if bytes.len() == DX7_RAW_BANK_BYTES && !bytes.contains(&0xf0) {
            "; a 4096-byte bank without its message wrapper reads with read_dx7_raw_bank"
        } else {
            ""
        };
        return Err(Error::new(
            ErrorCode::UnsupportedFormat,
            format!("No DX7 single-voice or 32-voice message{hint}"),
        ));
    }
    if stray > 0 {
        warnings.push(Dx7Warning {
            voice: None,
            message: format!(
                "{stray} {} outside any message {} ignored",
                if stray == 1 { "byte" } else { "bytes" },
                if stray == 1 { "was" } else { "were" }
            ),
        });
    }
    if !skipped.is_empty() {
        let sizes: Vec<String> = skipped.iter().map(|n| n.to_string()).collect();
        warnings.push(Dx7Warning {
            voice: None,
            message: format!(
                "{} messages that are not DX7 voice data ({} bytes) were skipped",
                skipped.len(),
                sizes.join(", ")
            ),
        });
    }
    let mut voices: Vec<Dx7Voice> = Vec::new();
    let mut names = Names::default();
    for message in messages {
        match message {
            Message::Voice(data) => push_voice(data, &mut voices, &mut names, &mut warnings)?,
            Message::Bank(data) => {
                for packed in data.chunks_exact(128) {
                    push_voice(&unpack(packed), &mut voices, &mut names, &mut warnings)?;
                }
            }
        }
    }
    Ok(Dx7Voices { voices, warnings })
}

/// Reads a 32-voice bank saved without its message wrapper: exactly 4096 bytes of
/// packed voices, with no header and no checksum.
///
/// Such a file cannot be recognised by its bytes, so it has its own function. Fails
/// with [`ErrorCode::DecodeError`] if `bytes` is not 4096 bytes long, or holds a
/// byte above 0x7F, which voice data never does.
pub fn read_dx7_raw_bank(bytes: &[u8]) -> Result<Dx7Voices> {
    if bytes.len() != DX7_RAW_BANK_BYTES {
        return Err(Error::decode(format!(
            "A raw DX7 bank is {DX7_RAW_BANK_BYTES} bytes, not {}",
            bytes.len()
        )));
    }
    if let Some(at) = bytes.iter().position(|&b| b > 0x7f) {
        return Err(Error::decode(format!(
            "Byte {at} is {:#04x}; DX7 voice data is seven-bit",
            bytes[at]
        )));
    }
    let mut voices = Vec::new();
    let mut names = Names::default();
    let mut warnings = Vec::new();
    for packed in bytes.chunks_exact(128) {
        push_voice(&unpack(packed), &mut voices, &mut names, &mut warnings)?;
    }
    Ok(Dx7Voices { voices, warnings })
}

/// What a message is: DX7 voice data (with its checksum checked), or `None` for a
/// message of something else.
fn classify<'a>(
    message: &'a [u8],
    at: usize,
    warnings: &mut Vec<Dx7Warning>,
) -> Result<Option<Message<'a>>> {
    // F0 43 0n ff hh ll ... checksum F7, with `n` a channel and `ff` the format.
    if message.len() < 3 || message[1] != 0x43 || message[2] & 0xf0 != 0x00 {
        return Ok(None);
    }
    if message.len() <= 6 {
        // A bulk header ended by F7 before its format, or the start of a DX7 voice or
        // bank header before it is whole.
        let head = &message[3..message.len() - 1];
        if matches!(head.first(), None | Some(0x00 | 0x09)) {
            return Err(Error::decode(format!(
                "The DX7 message at byte {at} is cut short after {} bytes",
                message.len()
            )));
        }
        return Ok(None);
    }
    let (format, count) = (message[3], [message[4], message[5]]);
    let (expected, label) = match (format, count) {
        (0x00, _) => (DX7_VOICE_BYTES, "single-voice"),
        (0x09, _) => (DX7_BANK_BYTES, "32-voice"),
        _ => return Ok(None),
    };
    if message.len() != expected {
        return Err(Error::decode(format!(
            "The DX7 {label} message at byte {at} is {} bytes, not {expected}",
            message.len()
        )));
    }
    if count != standard_count(format) {
        let stated = (count[0] as usize) << 7 | count[1] as usize;
        let (what, all) = if format == 0x09 {
            ("bank", 4096)
        } else {
            ("voice", 155)
        };
        warnings.push(Dx7Warning {
            voice: None,
            message: format!(
                "The {what} at byte {at} says it holds {stated} data bytes; all {all} were read"
            ),
        });
    }
    let data = &message[6..message.len() - 2];
    let stored = message[message.len() - 2];
    if stored != checksum(data) {
        warnings.push(Dx7Warning {
            voice: None,
            message: format!(
                "Checksum is {stored:#04x}, expected {:#04x}; the data may be damaged",
                checksum(data)
            ),
        });
    }
    Ok(Some(if format == 0x00 {
        Message::Voice(data)
    } else {
        Message::Bank(data)
    }))
}

/// Reads the 155 parameter bytes of one voice and adds it under a unique name.
fn push_voice(
    data: &[u8],
    voices: &mut Vec<Dx7Voice>,
    names: &mut Names,
    warnings: &mut Vec<Dx7Warning>,
) -> Result<()> {
    let mut fields = Fields {
        voice: voices.len(),
        warnings,
    };
    let (fm, original_name) = voice(data, &mut fields);
    let base = sanitize(&original_name);
    let name = names.unique(base);
    let patch = FmPatch {
        name,
        volume: 15,
        voice: fm,
    };
    patch.validate()?;

    voices.push(Dx7Voice {
        patch,
        original_name,
    });
    Ok(())
}

/// The 155 parameter bytes of a patch, or the engine extensions that DX7 data
/// cannot hold.
fn parameters(patch: &FmPatch) -> Result<[u8; 155]> {
    patch.validate()?;
    let (v, d) = (&patch.voice, Fm::default());
    let mut extras = Vec::new();
    let mut extra = |used: bool, name: &'static str| {
        if used {
            extras.push(name);
        }
    };
    extra(patch.volume != 15, "volume");
    extra(v.unison != d.unison, "unison");
    extra(v.unison_detune != 0, "unison-detune");
    extra(v.spread != 0, "spread");
    extra(v.pan != 0, "pan");
    extra(v.glide_ms != 0, "glide");
    extra(v.legato, "legato");
    extra(v.oversample != 1, "oversample");
    extra(v.lfo.beats.is_some(), "lfo beats");
    extra(v.lfo.seed != FmLfo::default().seed, "lfo seed");
    extra(patch.name.len() > 10, "a name longer than ten characters");
    if !extras.is_empty() {
        return Err(Error::invalid(format!(
            "DX7 voice data cannot hold: {}",
            extras.join(", ")
        )));
    }
    let mut out = [0; 155];
    for (block, bytes) in out[..126].chunks_exact_mut(21).enumerate() {
        // A switched-off operator is written at level 0.
        let o = v.operators[5 - block].unwrap_or(FmOperator {
            level: 0,
            ..FmOperator::default()
        });
        bytes[0..4].copy_from_slice(&o.envelope.rates);
        bytes[4..8].copy_from_slice(&o.envelope.levels);
        bytes[8] = o.scaling.break_point;
        bytes[9] = o.scaling.left_depth;
        bytes[10] = o.scaling.right_depth;
        bytes[11] = o.scaling.left_curve as u8;
        bytes[12] = o.scaling.right_curve as u8;
        bytes[13] = o.rate_scaling;
        bytes[14] = o.amplitude_sensitivity;
        bytes[15] = o.velocity;
        bytes[16] = o.level;
        (bytes[17], bytes[18], bytes[19]) = match o.frequency {
            FmFrequency::Ratio { coarse, fine } => (0, coarse, fine),
            FmFrequency::Fixed { decade, fine } => (1, decade, fine),
        };
        bytes[20] = (o.detune + 7) as u8;
    }
    let g = &mut out[126..];
    g[0..4].copy_from_slice(&v.pitch_envelope.rates);
    g[4..8].copy_from_slice(&v.pitch_envelope.levels);
    g[8] = v.algorithm - 1;
    g[9] = v.feedback;
    g[10] = v.key_sync as u8;
    g[11] = v.lfo.speed;
    g[12] = v.lfo.delay;
    g[13] = v.lfo.pitch_depth;
    g[14] = v.lfo.amplitude_depth;
    g[15] = v.lfo.key_sync as u8;
    g[16] = v.lfo.wave as u8;
    g[17] = v.lfo.pitch_sensitivity;
    g[18] = (v.transpose + 24) as u8;
    g[19..29].fill(b' ');
    for (slot, c) in g[19..29].iter_mut().zip(patch.name.bytes()) {
        *slot = if c == b'_' { b' ' } else { c };
    }
    Ok(out)
}

fn message(header: [u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 8);
    out.extend([0xf0, 0x43]);
    out.extend(header);
    out.extend(data);
    out.push(checksum(data));
    out.push(0xf7);
    out
}

/// Writes one patch as a 163-byte single-voice message.
///
/// Fails with [`ErrorCode::InvalidArgument`], naming them, if the patch uses engine
/// extensions DX7 data cannot hold. `voices` is not stored. A switched-off operator
/// is written at output level 0, and `_` in the name as a space.
pub fn write_dx7_voice(patch: &FmPatch) -> Result<Vec<u8>> {
    Ok(message(VOICE_HEADER, &parameters(patch)?))
}

/// Writes exactly 32 patches as a 4104-byte bank message. See [`write_dx7_voice`].
pub fn write_dx7_bank(patches: &[FmPatch]) -> Result<Vec<u8>> {
    if patches.len() != DX7_BANK_VOICES {
        return Err(Error::invalid(format!(
            "A DX7 bank holds exactly {DX7_BANK_VOICES} voices, not {}",
            patches.len()
        )));
    }
    let mut data = Vec::with_capacity(4096);
    for patch in patches {
        let v = parameters(patch)?;
        for op in 0..6 {
            let o = &v[op * 21..];
            data.extend(&o[..11]);
            data.push(o[11] | o[12] << 2);
            data.push(o[13] | o[20] << 3);
            data.push(o[14] | o[15] << 2);
            data.push(o[16]);
            data.push(o[17] | o[18] << 1);
            data.push(o[19]);
        }
        data.extend(&v[126..135]);
        data.push(v[135] | v[136] << 3);
        data.extend(&v[137..141]);
        data.push(v[141] | v[142] << 1 | v[143] << 4);
        data.extend(&v[144..155]);
    }
    Ok(message(BANK_HEADER, &data))
}

/// The byte count of a standard DX7 header of `format`: `01 1B` (155) for a voice,
/// `20 00` (4096) for a bank.
fn standard_count(format: u8) -> [u8; 2] {
    if format == 0x09 {
        [BANK_HEADER[2], BANK_HEADER[3]]
    } else {
        [VOICE_HEADER[2], VOICE_HEADER[3]]
    }
}

/// The length a message starting at `bytes[0]` has if its header is a DX7 single
/// voice or bank, whatever its count bytes say.
fn dx7_length(bytes: &[u8]) -> Option<usize> {
    let header = bytes.get(..6)?;
    if header[0] != 0xf0 || header[1] != 0x43 || header[2] & 0xf0 != 0x00 {
        return None;
    }
    match (header[3], [header[4], header[5]]) {
        (0x00, _) => Some(DX7_VOICE_BYTES),
        (0x09, _) => Some(DX7_BANK_BYTES),
        _ => None,
    }
}

/// The names given so far, and for each base name the next suffix to try, so that
/// many voices of one name are named in linear time.
#[derive(Default)]
struct Names {
    used: HashSet<String>,
    next: std::collections::HashMap<String, usize>,
}

impl Names {
    /// `base`, or `base-2`, `base-3` and so on: the first not given yet.
    fn unique(&mut self, base: String) -> String {
        if self.used.insert(base.clone()) {
            return base;
        }
        let mut suffix = self.next.get(&base).copied().unwrap_or(2);
        let name = loop {
            let name = format!("{base}-{suffix}");
            suffix += 1;
            if self.used.insert(name.clone()) {
                break name;
            }
        };
        self.next.insert(base, suffix);
        name
    }
}
