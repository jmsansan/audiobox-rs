//! Starting patches written for this crate. `docs/fm-presets.md` explains each one.
use super::FmPatch;

/// Names of the built-in FM presets, for `preset=NAME` on an `fm` line and
/// [`FmPatch::preset`].
pub const FM_PRESETS: [&str; 11] = [
    "epiano",
    "epiano-soft",
    "bell",
    "marimba",
    "brass",
    "bass-round",
    "bass-hard",
    "organ",
    "clav",
    "strings",
    "metal",
];

const TEXT: [&str; 11] = [
    // Three pairs: a bright tine that dies fast over two detuned bodies.
    "fm epiano algorithm=5 feedback=4 volume=9
  lfo wave=sine speed=30 amp=22
  op 1 ratio=1 rates=96,25,25,67 levels=99,75,0,0 velocity=2 rate-scaling=2
  op 2 ratio=14 level=60 rates=95,50,35,78 levels=99,75,0,0 velocity=7 scaling=39,0,30
  op 3 ratio=1 detune=-3 rates=95,20,20,50 levels=99,95,0,0 velocity=2 amp-sensitivity=1
  op 4 ratio=1 level=78 rates=95,29,20,50 levels=99,95,0,0 velocity=6 scaling=39,0,12
  op 5 ratio=1 detune=3 rates=95,20,20,50 levels=99,95,0,0 velocity=2 amp-sensitivity=1
  op 6 ratio=1 level=74 rates=95,29,20,50 levels=99,95,0,0 velocity=6 scaling=39,0,12
end",
    // The same idea with less tine, softer modulators and a slower attack.
    "fm epiano-soft algorithm=5 feedback=2 volume=9
  lfo wave=sine speed=26 amp=30
  op 1 ratio=1 rates=80,25,22,60 levels=99,70,0,0 velocity=2 rate-scaling=2
  op 2 ratio=10 level=46 rates=90,55,35,78 levels=99,60,0,0 velocity=6 scaling=39,0,30
  op 3 ratio=1 detune=-2 rates=78,20,18,45 levels=99,92,0,0 velocity=2 amp-sensitivity=2
  op 4 ratio=1 level=68 rates=85,30,20,50 levels=99,85,0,0 velocity=5 scaling=39,0,12
  op 5 ratio=1 detune=2 rates=78,20,18,45 levels=99,92,0,0 velocity=2 amp-sensitivity=2
  op 6 ratio=1 level=64 rates=85,30,20,50 levels=99,85,0,0 velocity=5 scaling=39,0,12
end",
    // Modulators near 3.5 times the note give the partials of a struck tube.
    "fm bell algorithm=5 volume=9
  op 1 ratio=1 rates=99,33,28,36 levels=99,0,0,0 velocity=2 rate-scaling=2
  op 2 ratio=3,17 level=76 rates=99,40,30,45 levels=99,0,0,0 velocity=5 scaling=39,0,20
  op 3 ratio=1 detune=3 level=94 rates=99,35,28,36 levels=99,0,0,0 velocity=2 rate-scaling=2
  op 4 ratio=3,17 detune=-3 level=72 rates=99,42,30,45 levels=99,0,0,0 velocity=5 scaling=39,0,20
  op 5 ratio=4 level=84 rates=99,48,40,48 levels=99,0,0,0 velocity=4 rate-scaling=3
  op 6 ratio=9,10 level=64 rates=99,60,45,55 levels=99,0,0,0 velocity=6 scaling=39,0,35
end",
    // A short mallet: the click is one fast modulator, the wood another.
    "fm marimba algorithm=5 volume=11
  op 1 ratio=1 rates=99,50,40,55 levels=99,0,0,0 velocity=2 rate-scaling=4
  op 2 ratio=4 level=74 rates=99,57,50,70 levels=99,0,0,0 velocity=6 scaling=39,0,25
  op 3 ratio=1 level=90 rates=99,54,40,55 levels=99,0,0,0 velocity=2 rate-scaling=4
  op 4 ratio=10 level=58 rates=99,78,60,75 levels=99,0,0,0 velocity=7 scaling=39,0,40
end",
    // Feedback on the modulator, which swells in after the carrier.
    "fm brass algorithm=2 feedback=6 voices=4 unison=2 unison-detune=6 spread=40 volume=15
  lfo wave=triangle speed=33 delay=55 pitch=12 pitch-sensitivity=3
  op 1 ratio=1 rates=62,40,30,55 levels=99,92,88,0 velocity=2
  op 2 ratio=1 level=86 rates=48,45,30,55 levels=99,93,90,0 velocity=5 scaling=39,0,22
end",
    // Three bright pairs, one an octave up, so the note is heard on small speakers
    // through its harmonics; mono with a short glide.
    "fm bass-round algorithm=5 feedback=5 voices=1 glide=30 legato=on volume=11
  op 1 ratio=1 rates=99,45,35,65 levels=99,90,80,0 velocity=2
  op 2 ratio=1 level=90 rates=99,50,38,65 levels=99,88,80,0 velocity=2 scaling=39,0,20
  op 3 ratio=2 level=95 rates=99,48,35,65 levels=99,86,72,0 velocity=2
  op 4 ratio=1 level=86 rates=99,52,40,65 levels=99,82,70,0 velocity=3 scaling=39,0,20
  op 5 ratio=1 detune=2 level=92 rates=99,45,35,65 levels=99,88,76,0 velocity=2
  op 6 ratio=1 level=88 rates=99,50,38,65 levels=99,86,76,0 velocity=3 scaling=39,0,20
end",
    // Harder modulators, a pair on the third harmonic and strong feedback.
    "fm bass-hard algorithm=5 feedback=6 voices=1 glide=20 legato=on volume=10
  op 1 ratio=1 rates=99,45,35,65 levels=99,85,70,0 velocity=2
  op 2 ratio=1 level=93 rates=99,48,38,65 levels=99,92,86,0 velocity=3 scaling=39,0,25
  op 3 ratio=3 level=92 rates=99,48,35,65 levels=99,84,68,0 velocity=3
  op 4 ratio=1 level=90 rates=99,50,42,65 levels=99,86,74,0 velocity=4 scaling=39,0,25
  op 5 ratio=2 level=95 rates=99,45,35,65 levels=99,88,74,0 velocity=2
  op 6 ratio=1 level=92 rates=99,48,40,65 levels=99,90,82,0 velocity=3 scaling=39,0,25
end",
    // Six carriers as drawbars; a little feedback on the highest one.
    "fm organ algorithm=32 feedback=3 volume=9
  lfo wave=sine speed=40 amp=14 pitch=3 pitch-sensitivity=2
  op 1 ratio=0.5 level=92 rates=95,99,99,80 amp-sensitivity=1
  op 2 ratio=1 rates=95,99,99,80 amp-sensitivity=1
  op 3 ratio=2 level=90 rates=95,99,99,80 amp-sensitivity=1
  op 4 ratio=3 level=84 rates=95,99,99,80 amp-sensitivity=1
  op 5 ratio=4 level=80 rates=95,99,99,80 scaling=39,0,25 amp-sensitivity=1
  op 6 ratio=6 level=72 rates=95,99,99,80 scaling=39,0,35 amp-sensitivity=1
end",
    // A plucked comp sound: bright at the start, hollow as it decays.
    "fm clav algorithm=2 feedback=5 volume=12
  op 1 ratio=1 rates=99,50,35,70 levels=99,60,0,0 velocity=2 rate-scaling=3
  op 2 ratio=3 level=84 rates=99,58,45,70 levels=99,50,0,0 velocity=6 scaling=39,0,25
  op 3 ratio=1 level=88 rates=99,56,40,70 levels=99,40,0,0 velocity=2 rate-scaling=3
  op 4 ratio=5 level=70 rates=99,68,50,70 levels=99,0,0,0 velocity=7 scaling=39,0,35
end",
    // Slow modulators under a slow attack, widened by three unison copies.
    "fm strings algorithm=2 feedback=5 voices=6 unison=3 unison-detune=12 spread=70 volume=15
  lfo wave=triangle speed=28 delay=40 pitch=6 pitch-sensitivity=2
  op 1 ratio=1 rates=45,30,25,40 levels=99,95,90,0 velocity=1
  op 2 ratio=1 level=84 rates=40,28,20,40 levels=90,99,92,0 velocity=3 scaling=39,0,20
  op 3 ratio=1 detune=5 level=92 rates=42,30,25,40 levels=99,95,90,0 velocity=1
  op 4 ratio=2 level=76 rates=30,20,20,40 levels=80,99,90,0 velocity=3 scaling=39,0,25
end",
    // Fixed-frequency partials: the note changes nothing but the keyboard response.
    "fm metal algorithm=5 feedback=5 volume=10
  op 1 fixed=100,45 rates=99,48,40,50 levels=99,0,0,0 velocity=3
  op 2 fixed=100,73 level=80 rates=99,56,45,60 levels=99,0,0,0 velocity=6
  op 3 fixed=1000,5 level=94 rates=99,52,40,52 levels=99,0,0,0 velocity=3
  op 4 fixed=100,91 level=82 rates=99,60,45,60 levels=99,0,0,0 velocity=6
  op 5 fixed=1000,37 level=88 rates=99,56,40,56 levels=99,0,0,0 velocity=4
  op 6 fixed=1000,62 level=78 rates=99,64,45,60 levels=99,0,0,0 velocity=7
end",
];

impl FmPatch {
    /// A built-in starting patch by one of the [`FM_PRESETS`] names.
    pub fn preset(name: &str) -> Option<FmPatch> {
        let index = FM_PRESETS.iter().position(|n| *n == name)?;
        Some(FmPatch::parse(TEXT[index]).expect("built-in presets parse"))
    }
}
