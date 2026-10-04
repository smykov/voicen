//! RIFF WAV writer for an [`AudioBuffer`]: PCM 16-bit little-endian, 16 000 Hz,
//! 1 channel, written by hand (`hound` is not consented; decision #9). Ten minutes
//! are about 19.2 MB (decisions #1).

use super::AudioBuffer;

/// The complete WAV file: a 44-byte header, then the samples.
pub fn encode(audio: &AudioBuffer) -> Vec<u8> {
    // T-040 skeleton: wrong on purpose until implemented (red tests first).
    let _ = audio;
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_bytes_pcm16_16k_mono() {
        // Every header byte (canonical 44-byte PCM header), then the samples
        // little-endian. Bite: big-endian samples, byte rate 16000 (forgot the 2
        // bytes per sample), block align 1, RIFF size without the 36, data size in
        // samples instead of bytes, a WAVE_FORMAT_EXTENSIBLE header.
        let buf = AudioBuffer::from_16k_mono(vec![1, -2, 0x1234]);
        let mut expected: Vec<u8> = Vec::new();
        expected.extend_from_slice(b"RIFF");
        expected.extend_from_slice(&42u32.to_le_bytes()); // 36 + data bytes (6)
        expected.extend_from_slice(b"WAVE");
        expected.extend_from_slice(b"fmt ");
        expected.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        expected.extend_from_slice(&1u16.to_le_bytes()); // PCM
        expected.extend_from_slice(&1u16.to_le_bytes()); // mono
        expected.extend_from_slice(&16_000u32.to_le_bytes()); // sample rate
        expected.extend_from_slice(&32_000u32.to_le_bytes()); // byte rate
        expected.extend_from_slice(&2u16.to_le_bytes()); // block align
        expected.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        expected.extend_from_slice(b"data");
        expected.extend_from_slice(&6u32.to_le_bytes()); // data bytes
        expected.extend_from_slice(&[0x01, 0x00, 0xFE, 0xFF, 0x34, 0x12]);
        assert_eq!(encode(&buf), expected);
    }

    #[test]
    fn empty_buffer_is_header_only_and_size_scales() {
        // Sizes follow the sample count: 0 samples -> 44 bytes with data size 0;
        // 10 min at 16 kHz -> 44 + 19 200 000 bytes (decisions #1).
        // Bite: sizes hard-coded, or a fixed-size padding.
        let empty = encode(&AudioBuffer::from_16k_mono(Vec::new()));
        assert_eq!(empty.len(), 44);
        assert_eq!(empty.get(4..8), Some(&36u32.to_le_bytes()[..]));
        assert_eq!(empty.get(40..44), Some(&0u32.to_le_bytes()[..]));

        let ten_min = encode(&AudioBuffer::from_16k_mono(vec![0; 16_000 * 600]));
        assert_eq!(ten_min.len(), 44 + 19_200_000);
        assert_eq!(
            ten_min.get(4..8),
            Some(&(36u32 + 19_200_000).to_le_bytes()[..])
        );
        assert_eq!(ten_min.get(40..44), Some(&19_200_000u32.to_le_bytes()[..]));
    }
}
