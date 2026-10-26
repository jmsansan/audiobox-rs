//! Optional, external RFC 8251 corpus. No downloaded audio is shipped in the crate.
use super::{decoder::OpusDecoder, packet};

#[test]
#[ignore = "requires AUDIOBOX_OPUS_TESTVECTORS pointing to the official RFC 8251 corpus"]
fn official_rfc8251_ranges_and_pcm() {
    let root = std::path::PathBuf::from(
        std::env::var_os("AUDIOBOX_OPUS_TESTVECTORS").expect("corpus path"),
    );
    let mut packets = 0;
    for id in 1..=12 {
        let data = std::fs::read(root.join(format!("testvector{id:02}.bit"))).unwrap();
        let reference = std::fs::read(root.join(format!("testvector{id:02}.dec"))).unwrap();
        let mut decoder = OpusDecoder::new(2).unwrap();
        let mut output = [0.0; 11520];
        let mut at = 0;
        let mut sample = 0;
        let mut signal = 0.0f64;
        let mut error = 0.0f64;
        while at < data.len() {
            let len = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            let expected = u32::from_be_bytes(data[at + 4..at + 8].try_into().unwrap());
            at += 8;
            let payload = &data[at..at + len];
            at += len;
            let packet = packet::parse_packet(payload).unwrap();
            let frames = decoder
                .decode_packet(&packet, payload, &mut output)
                .unwrap();
            assert_eq!(
                decoder.range_final(),
                expected,
                "vector {id}, packet {packets}"
            );
            for &x in &output[..frames * 2] {
                let y =
                    i16::from_le_bytes(reference[sample * 2..sample * 2 + 2].try_into().unwrap())
                        as f64;
                let d = (x as f64 * 32768.0).round().clamp(-32768.0, 32767.0) - y;
                signal += y * y;
                error += d * d;
                sample += 1;
            }
            packets += 1;
        }
        assert_eq!(sample * 2, reference.len(), "vector {id} length");
        let snr = 10.0 * (signal / error).log10();
        eprintln!("RFC8251 vector {id:02}: {sample} samples, SNR={snr:.2} dB");
        assert!(error == 0.0 || snr > 80.0, "vector {id}, SNR={snr}");
    }
    eprintln!("All {packets} official packet ranges agree");
}
