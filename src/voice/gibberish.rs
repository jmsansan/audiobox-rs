//! Text-to-gibberish planning: dialogue text in, invented syllables out.
use std::sync::Arc;

use super::{
    Articulation, Marker, MarkerKind, Rng, UtterancePlan, VocalEvent, VoiceLimits, VoiceProfile,
    Vowel, fnv1a, splitmix,
};
use crate::{Error, Result};

/// How freely the planner varies rhythm, level and pitch. Delivery is separate
/// from timbre: the same profile can speak either way.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Delivery {
    /// Equal syllables at full level and the profile's base pitch; only the
    /// phrase-ending contours and pauses move. A control for comparisons.
    Rigid,
    /// Seeded, bounded variation of syllable length, level and pitch, with
    /// declining phrase intonation and stressed word starts.
    #[default]
    Expressive,
}

/// Options of [`plan_gibberish`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GibberishOptions {
    pub delivery: Delivery,
    /// Base syllable length in microseconds, 60000..=400000. Default 150000.
    pub syllable_us: u32,
    /// Root of the planner's choices. The same text, profile, options and seed
    /// always give the same plan. Default 0.
    pub seed: u64,
    pub limits: VoiceLimits,
}
impl Default for GibberishOptions {
    fn default() -> Self {
        Self {
            delivery: Delivery::Expressive,
            syllable_us: 150_000,
            seed: 0,
            limits: VoiceLimits::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ending {
    Statement,
    Question,
    Exclamation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Class {
    /// A letter of an alphabet, grouped three to a syllable.
    Alpha,
    /// A kana, hangul or CJK character: one syllable each.
    Syllabic,
    /// Attaches to the previous character.
    Combining,
    /// Joins a word without sounding: apostrophes and hyphens.
    Joiner,
    Space,
    Pause,
    End(Ending),
    Open(Ending),
    /// Emoji, symbols and unsupported scripts: a silent word boundary.
    Skip,
}

fn classify(c: char) -> Class {
    let u = c as u32;
    match c {
        'a'..='z' | 'A'..='Z' | '0'..='9' => Class::Alpha,
        ',' | ';' | ':' | '、' | '，' | '；' | '：' => Class::Pause,
        '.' | '…' | '。' | '．' => Class::End(Ending::Statement),
        '?' | '？' => Class::End(Ending::Question),
        '!' | '！' => Class::End(Ending::Exclamation),
        '¿' => Class::Open(Ending::Question),
        '¡' => Class::Open(Ending::Exclamation),
        '\'' | '’' | '-' | '‐' => Class::Joiner,
        '\u{30FB}' => Class::Space,
        '\u{30FC}' | '\u{200D}' => Class::Combining,
        _ if c.is_whitespace() => Class::Space,
        _ => match u {
            0xD7 | 0xF7 => Class::Skip,
            0xC0..=0x24F | 0x370..=0x3FF | 0x400..=0x4FF => Class::Alpha,
            0x300..=0x36F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF => Class::Combining,
            0xFE00..=0xFE0F | 0xFE20..=0xFE2F => Class::Combining,
            0x3040..=0x30FF | 0x4E00..=0x9FFF | 0xAC00..=0xD7A3 => Class::Syllabic,
            _ => Class::Skip,
        },
    }
}

#[derive(Clone, Debug)]
struct Syllable {
    start: usize,
    end: usize,
    letters: usize,
    hash: u64,
}
#[derive(Clone, Debug)]
struct Word {
    start: usize,
    end: usize,
    syllables: Vec<Syllable>,
}
#[derive(Clone, Debug)]
enum Item {
    Word(Word),
    Comma,
}
#[derive(Clone, Debug)]
struct Phrase {
    start: usize,
    end: usize,
    opening: Option<Ending>,
    closing: Option<Ending>,
    items: Vec<Item>,
}
impl Phrase {
    fn new(start: usize) -> Self {
        Self {
            start,
            end: start,
            opening: None,
            closing: None,
            items: Vec::new(),
        }
    }
    fn has_words(&self) -> bool {
        self.items.iter().any(|i| matches!(i, Item::Word(_)))
    }
    fn ending(&self) -> Ending {
        self.closing.or(self.opening).unwrap_or(Ending::Statement)
    }
}

/// Splits text into phrases, words and syllables, with byte spans.
fn segment(text: &str) -> Vec<Phrase> {
    let mut phrases = Vec::new();
    let mut phrase: Option<Phrase> = None;
    let mut word: Option<Word> = None;
    // Whether the last scalar extended a syllable (so a combining mark attaches).
    let mut attached = false;
    let mut joined = false;
    let close_word = |phrase: &mut Option<Phrase>, word: &mut Option<Word>| {
        if let Some(w) = word.take() {
            let p = phrase.get_or_insert_with(|| Phrase::new(w.start));
            p.end = w.end;
            p.items.push(Item::Word(w));
        }
    };
    for (at, c) in text.char_indices() {
        let end = at + c.len_utf8();
        let class = classify(c);
        // Letters after closing punctuation start a new phrase.
        if matches!(class, Class::Alpha | Class::Syllabic | Class::Open(_))
            && phrase.as_ref().is_some_and(|p| p.closing.is_some())
        {
            phrases.extend(phrase.take());
        }
        match class {
            Class::Alpha | Class::Syllabic => {
                let w = word.get_or_insert_with(|| Word {
                    start: at,
                    end,
                    syllables: Vec::new(),
                });
                w.end = end;
                let lower = if c.is_ascii() {
                    c.to_ascii_lowercase()
                } else {
                    c
                };
                let mut buf = [0u8; 4];
                let bytes = lower.encode_utf8(&mut buf).as_bytes();
                let extend = class == Class::Alpha
                    && !joined
                    && w.syllables
                        .last()
                        .is_some_and(|s| s.letters > 0 && s.letters < 3);
                if extend {
                    let s = w.syllables.last_mut().expect("checked above");
                    s.end = end;
                    s.letters += 1;
                    s.hash = fnv_extend(s.hash, bytes);
                } else {
                    w.syllables.push(Syllable {
                        start: at,
                        end,
                        // A syllabic character fills its syllable at once.
                        letters: if class == Class::Alpha { 1 } else { 3 },
                        hash: fnv1a(bytes),
                    });
                }
                attached = true;
                joined = false;
            }
            Class::Combining => {
                if attached {
                    if let Some(w) = word.as_mut() {
                        w.end = end;
                        let s = w.syllables.last_mut().expect("a word has a syllable");
                        s.end = end;
                        s.hash = fnv_extend(s.hash, c.encode_utf8(&mut [0u8; 4]).as_bytes());
                    }
                }
            }
            Class::Joiner => {
                if word.is_some() {
                    joined = true;
                }
                attached = false;
            }
            Class::Space | Class::Skip => {
                close_word(&mut phrase, &mut word);
                attached = false;
                joined = false;
            }
            Class::Pause => {
                close_word(&mut phrase, &mut word);
                if let Some(p) = phrase.as_mut() {
                    if p.closing.is_none() && p.has_words() {
                        p.items.push(Item::Comma);
                        p.end = end;
                    }
                }
                attached = false;
                joined = false;
            }
            Class::End(kind) => {
                close_word(&mut phrase, &mut word);
                if let Some(p) = phrase.as_mut() {
                    if p.has_words() {
                        // Repeated marks extend one ending; a question mark wins.
                        p.closing = Some(match (p.closing, kind) {
                            (Some(Ending::Question), _) | (_, Ending::Question) => Ending::Question,
                            (Some(Ending::Exclamation), _) | (_, Ending::Exclamation) => {
                                Ending::Exclamation
                            }
                            _ => Ending::Statement,
                        });
                        p.end = end;
                    }
                }
                attached = false;
                joined = false;
            }
            Class::Open(kind) => {
                close_word(&mut phrase, &mut word);
                match phrase.as_mut() {
                    Some(p) if p.has_words() => {}
                    Some(p) => {
                        p.opening = Some(kind);
                    }
                    None => {
                        let mut p = Phrase::new(at);
                        p.opening = Some(kind);
                        phrase = Some(p);
                    }
                }
                attached = false;
                joined = false;
            }
        }
    }
    close_word(&mut phrase, &mut word);
    phrases.extend(phrase);
    phrases.retain(Phrase::has_words);
    // A trailing comma sounds as nothing.
    for p in &mut phrases {
        while matches!(p.items.last(), Some(Item::Comma)) {
            p.items.pop();
        }
    }
    phrases
}

fn fnv_extend(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash = (hash ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Pause lengths of planner versions 1 and 2, in microseconds.
const WORD_GAP_US: u32 = 45_000;
const COMMA_US: u32 = 220_000;
const PHRASE_US: u32 = 380_000;
const SYLLABLE_DOMAIN: u64 = 0x7379_6c6c;
const DELIVERY_DOMAIN: u64 = 0x6465_6c69;

struct Builder {
    events: Vec<VocalEvent>,
    markers: Vec<Marker>,
    limits: VoiceLimits,
}
impl Builder {
    fn push(&mut self, event: VocalEvent) -> Result<()> {
        if event.duration_us == 0 {
            return Ok(());
        }
        if self.events.len() >= self.limits.max_events {
            return Err(Error::limit(format!(
                "Dialogue plans at most {} events",
                self.limits.max_events
            )));
        }
        self.events.push(event);
        Ok(())
    }
    fn mark(&mut self, kind: MarkerKind, bytes: std::ops::Range<usize>) -> Result<usize> {
        if self.markers.len() >= self.limits.max_markers {
            return Err(Error::limit(format!(
                "Dialogue plans at most {} markers",
                self.limits.max_markers
            )));
        }
        let at = self.events.len();
        self.markers.push(Marker {
            kind,
            bytes,
            events: at..at,
        });
        Ok(self.markers.len() - 1)
    }
    fn close(&mut self, marker: usize) {
        self.markers[marker].events.end = self.events.len();
    }
}

/// Compiles dialogue text into an invented-syllable plan with phrase, word and
/// syllable markers. Nothing is pronounced: the text only sets rhythm, stress,
/// pauses and intonation, and the same word always gets the same syllables for a
/// given seed.
///
/// # Text handling (planner versions 1 and 2)
///
/// Input is any `&str` of at most [`VoiceLimits::max_text_bytes`] bytes; longer
/// text is [`ErrorCode::LimitExceeded`](crate::ErrorCode::LimitExceeded). Scalars
/// are classified with fixed tables, not grapheme segmentation:
///
/// * ASCII letters and digits, Latin-1 and Latin Extended-A/B letters, Greek and
///   Cyrillic are letters; up to three consecutive letters form one syllable.
///   Kana, hangul syllables and CJK ideographs are one syllable each.
/// * Combining marks (U+0300–036F, U+1AB0–1AFF, U+1DC0–1DFF, U+20D0–20FF,
///   U+FE20–FE2F), variation selectors, the zero-width joiner and the katakana
///   prolonged sound mark attach to the preceding letter and add no syllable.
/// * Apostrophes and hyphens join a word silently; the next letter starts a new
///   syllable.
/// * `,` `;` `:` (and their CJK forms) are pauses within a phrase; `.` `…` `?` `!`
///   (and CJK forms) close it, repeated marks counting once with `?` winning over
///   `!`. `¿` and `¡` open a phrase and set its ending when no closing mark
///   follows.
/// * Whitespace, emoji, symbols and all other scripts are silent word boundaries.
///
/// Text without letters plans no events.
///
/// # Prosody
///
/// Pitches are semitones around the profile's `pitch`, scaled by its `range` R.
/// Each word's first syllable is stressed. Word gaps are 45 ms closures, commas
/// 220 ms, and phrase ends 380 ms; a pause at the very end is omitted, so the
/// release tail follows the last syllable directly. The last syllable of a
/// phrase is 1.3 times longer and carries the contour of the ending:
///
/// | Ending | Last syllable glides |
/// | --- | --- |
/// | statement | from its pitch down by 0.5 R |
/// | question | from 0.1 R below its pitch up to 0.7 R above it |
/// | exclamation | from 0.35 R above its pitch down by 0.3 R, at full level |
/// | before a comma | up by 0.15 R |
///
/// [`Delivery::Rigid`] keeps every other syllable equal: [`GibberishOptions::syllable_us`]
/// long, full level, at the base pitch. [`Delivery::Expressive`] draws from a
/// seeded stream: syllable length ×0.82..1.18 (stressed ×1.12), unstressed level
/// 0.72..0.9, a declination from +0.25 R to -0.15 R across the phrase, +0.2 R on
/// stress, ±0.1 R random offset and a -0.04 R drift within each syllable; word gaps
/// 30..70 ms and pauses ×0.85..1.15. Every value is fixed in the plan before
/// rendering, and pitches are clamped to ±R and to 20..=4000 Hz.
///
/// Syllables open with a plosive (closure for 12% and burst for 6% of the
/// syllable) one time in four, a fricative (22%) or a breath (18%) one time in
/// eight each, and directly with the vowel otherwise, chosen with the vowel from a
/// hash of the syllable's letters (ASCII lowercased) and the seed (planner
/// version 2; version 1 used plosives 3/8, fricatives 2/8, breaths 1/8).
pub fn plan_gibberish(
    text: &str,
    profile: &VoiceProfile,
    options: &GibberishOptions,
) -> Result<UtterancePlan> {
    options.limits.check()?;
    profile.check_fields()?;
    if !(60_000..=400_000).contains(&options.syllable_us) {
        return Err(Error::invalid(format!(
            "Syllable length must be 60000..=400000 us, not {}",
            options.syllable_us
        )));
    }
    if text.len() > options.limits.max_text_bytes {
        return Err(Error::limit(format!(
            "Dialogue text exceeds {} bytes",
            options.limits.max_text_bytes
        )));
    }
    let phrases = segment(text);
    let expressive = options.delivery == Delivery::Expressive;
    let mut rng = Rng::new(splitmix(
        fnv1a(text.as_bytes()) ^ splitmix(options.seed ^ DELIVERY_DOMAIN),
    ));
    let syllable_key = splitmix(options.seed ^ SYLLABLE_DOMAIN);
    let range = profile.range;
    let base = profile.pitch;
    let hz =
        |st: f64| (base * crate::math::exp2(st.clamp(-range, range) / 12.0)).clamp(20.0, 4000.0);
    let us = |x: f64| (x + 0.5).floor().max(0.0) as u32;
    let syllable = options.syllable_us as f64;
    let mut b = Builder {
        events: Vec::new(),
        markers: Vec::new(),
        limits: options.limits,
    };
    let phrase_count = phrases.len();
    for (pi, phrase) in phrases.iter().enumerate() {
        let total: usize = phrase
            .items
            .iter()
            .map(|i| match i {
                Item::Word(w) => w.syllables.len(),
                Item::Comma => 0,
            })
            .sum();
        let phrase_marker = b.mark(MarkerKind::Phrase, phrase.start..phrase.end)?;
        let mut index = 0usize;
        for (ii, item) in phrase.items.iter().enumerate() {
            let Item::Word(word) = item else {
                let pause = if expressive {
                    COMMA_US as f64 * (0.85 + 0.3 * rng.unit())
                } else {
                    COMMA_US as f64
                };
                b.push(VocalEvent::rest(us(pause)))?;
                continue;
            };
            let before_comma = matches!(phrase.items.get(ii + 1), Some(Item::Comma));
            let word_marker = b.mark(MarkerKind::Word, word.start..word.end)?;
            for (si, syl) in word.syllables.iter().enumerate() {
                let stressed = si == 0;
                let last_in_phrase = index + 1 == total;
                let last_in_word = si + 1 == word.syllables.len();
                let h = splitmix(syl.hash ^ syllable_key);
                let vowel = Vowel::ALL[(h % 5) as usize];
                let onset = (h >> 8) % 8;
                let mut length = syllable;
                let mut level = 1.0;
                let mut st = 0.0;
                let mut drift = 0.0;
                if expressive {
                    length *= 0.82 + 0.36 * rng.unit();
                    if stressed {
                        length *= 1.12;
                    } else {
                        level = 0.72 + 0.18 * rng.unit();
                    }
                    let u = if total > 1 {
                        index as f64 / (total - 1) as f64
                    } else {
                        0.0
                    };
                    st = range * (0.25 - 0.4 * u);
                    if stressed {
                        st += 0.2 * range;
                    }
                    st += 0.1 * range * rng.signed();
                    drift = -0.04 * range;
                }
                let (mut from, mut to) = (st, st + drift);
                if last_in_phrase {
                    length *= 1.3;
                    match phrase.ending() {
                        Ending::Statement => to = st - 0.5 * range,
                        Ending::Question => {
                            from = st - 0.1 * range;
                            to = st + 0.7 * range;
                        }
                        Ending::Exclamation => {
                            from = st + 0.35 * range;
                            to = from - 0.3 * range;
                            level = 1.0;
                        }
                    }
                } else if before_comma && last_in_word {
                    to = st + 0.15 * range;
                }
                let syl_marker = b.mark(MarkerKind::Syllable, syl.start..syl.end)?;
                let mut vowel_len = length;
                let gesture = |a, fraction: f64| {
                    VocalEvent::gesture(a, vowel, us(length * fraction)).with_level(
                        if a == Articulation::Closure {
                            0.0
                        } else {
                            level
                        },
                    )
                };
                match onset {
                    0 | 1 => {
                        b.push(gesture(Articulation::Closure, 0.12))?;
                        b.push(gesture(Articulation::Fricative, 0.06))?;
                        vowel_len -= length * 0.18;
                    }
                    2 => {
                        b.push(gesture(Articulation::Fricative, 0.22))?;
                        vowel_len -= length * 0.22;
                    }
                    3 => {
                        b.push(gesture(Articulation::Breath, 0.18))?;
                        vowel_len -= length * 0.18;
                    }
                    _ => {}
                }
                b.push(
                    VocalEvent::vowel(vowel, us(vowel_len), hz(from))
                        .glide_to(hz(to))
                        .with_level(level),
                )?;
                b.close(syl_marker);
                index += 1;
            }
            b.close(word_marker);
            let next_is_word = matches!(phrase.items.get(ii + 1), Some(Item::Word(_)));
            if next_is_word {
                let gap = if expressive {
                    30_000.0 + 40_000.0 * rng.unit()
                } else {
                    WORD_GAP_US as f64
                };
                b.push(VocalEvent::rest(us(gap)))?;
            }
        }
        // The phrase marker covers its syllables, not the pause after it.
        b.close(phrase_marker);
        if pi + 1 < phrase_count {
            let pause = if expressive {
                PHRASE_US as f64 * (0.85 + 0.3 * rng.unit())
            } else {
                PHRASE_US as f64
            };
            b.push(VocalEvent::rest(us(pause)))?;
        }
    }
    Ok(UtterancePlan::with_text(
        b.events,
        b.markers,
        Arc::from(text),
    ))
}
