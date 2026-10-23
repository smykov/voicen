//! Ogg Opus call tracks (spec 007 R5-R7; contracts/core-call.md "Codec" and
//! "`mic.opus` / `system.opus`"; T-116).

// Red tests of T-116 (T003 of spec 007), on real libopus. They pin this surface
// (names binding for the tests; the bodies are T004's):
//
// - `crate::call`: `StorageError { pub op: StorageOp, pub kind: io::ErrorKind }`,
//   `StorageOp { Write, Flush, Sync }` (Debug, Clone, Copy, PartialEq, Eq),
//   `DecodeEnd` (Debug; at least `Unreadable` and `Damaged`).
// - `call::codec`:
//   - `trait SyncWrite: Write { fn sync_data(&mut self) -> io::Result<()>; }`, implemented
//     for `std::fs::File`;
//   - `trait TrackEncoder { fn push(&mut self, pcm16k: &[f32]) -> Result<(), StorageError>;
//     fn finish(self) -> Result<(), StorageError>; }`;
//   - `trait TrackDecoder { fn next_window(&mut self, max_ms: u32)
//     -> Result<Option<AudioBuffer>, DecodeEnd>; }`;
//   - `OpusOgg<W: SyncWrite>`: `OpusOgg::create(inner: W, serial: u32,
//     clock: Arc<dyn MonotonicClock>) -> Result<OpusOgg<W>, StorageError>`, `impl TrackEncoder`;
//   - `OpusOggReader<R: Read + Seek>`: `OpusOggReader::open(inner: R)
//     -> Result<OpusOggReader<R>, DecodeEnd>`, `impl TrackDecoder`;
//   - `scan<R: Read + Seek>(inner: R) -> Result<TrackScan, DecodeEnd>`,
//     `TrackScan { pub duration_48k: u64, pub end: ReadEnd }`,
//     `ReadEnd { Complete, Truncated, Damaged }` (Debug, Clone, Copy, PartialEq, Eq).
//
// Reader rules (the task's invariant): the packets of the longest prefix of CRC-valid pages
// after a valid OpusHead; any read error ends the data; Damaged iff a CRC-valid page follows
// an invalid one; duration = last good granule - pre-skip.

use std::collections::VecDeque;
use std::io::{self, Read, Seek, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ogg::{OggReadError, Packet, PacketReader, PacketWriteEndInfo, PacketWriter};

use crate::audio::AudioBuffer;
use crate::call::{DecodeEnd, StorageError, StorageOp};
use crate::clock::MonotonicClock;

/// The encoder's sample rate: call tracks are 16 kHz mono (research R5).
const RATE: u32 = 16_000;
/// 20 ms at 16 kHz: the samples of one Opus packet.
const FRAME: usize = 320;
/// One packet in the 48 kHz granule units of Ogg Opus (RFC 7845).
const GRANULE_PER_PACKET: u64 = 960;
/// 16 kHz samples per 48 kHz granule unit, inverted: granule = 3 x samples.
const GRANULE_PER_SAMPLE: u64 = 3;
/// A page ends after this many packets (1 s, research R6).
const PACKETS_PER_PAGE: u64 = 50;
/// `sync_data` runs once this much clock time passed since the last one (research R6).
const SYNC_EVERY: Duration = Duration::from_secs(5);
/// Encoder settings (research R5).
const BITRATE: i32 = 24_000;
const COMPLEXITY: i32 = 9;
/// libopus's recommended output buffer for one packet.
const MAX_PACKET: usize = 4_000;
/// The longest Opus packet (120 ms) at 16 kHz.
const MAX_DECODED: usize = 1_920;

/// A writer whose written data can be made durable: `File::sync_data` in production, a
/// counting fake in tests.
pub trait SyncWrite: Write {
    fn sync_data(&mut self) -> io::Result<()>;
}

impl SyncWrite for std::fs::File {
    fn sync_data(&mut self) -> io::Result<()> {
        std::fs::File::sync_data(self)
    }
}

/// Writes one call track (contracts/core-call.md "Codec"). Runs on the session's writer
/// thread, never in the audio callback.
pub trait TrackEncoder {
    /// Appends 16 kHz mono samples; any chunking gives the same file.
    fn push(&mut self, pcm16k: &[f32]) -> Result<(), StorageError>;
    /// Ends the stream (EOS page, final granule trims the tail), flushes and syncs.
    fn finish(self) -> Result<(), StorageError>;
}

/// Reads one call track back as 16 kHz audio.
pub trait TrackDecoder {
    /// The next window of at most `max_ms` (at least 1 ms) of 16 kHz audio; `Ok(None)` at
    /// the end of the recoverable data, `Err(DecodeEnd::Damaged)` instead of it when a bad
    /// page sits before the end of the file.
    fn next_window(&mut self, max_ms: u32) -> Result<Option<AudioBuffer>, DecodeEnd>;
}

fn storage(op: StorageOp) -> impl Fn(io::Error) -> StorageError {
    move |e| StorageError { op, kind: e.kind() }
}

fn encode_failed(_: opus::Error) -> StorageError {
    StorageError {
        op: StorageOp::Encode,
        kind: io::ErrorKind::Other,
    }
}

/// The Ogg Opus writer of a call track (research R5, R6): page 1 `OpusHead`, page 2
/// `OpusTags`, then audio pages of 50 packets, each flushed; `sync_data` whenever 5 s of the
/// injected clock passed since the last sync, and after the EOS page.
pub struct OpusOgg<W: SyncWrite> {
    pages: PacketWriter<'static, W>,
    encoder: opus::Encoder,
    serial: u32,
    clock: Arc<dyn MonotonicClock>,
    last_sync: Instant,
    /// 48 kHz units the decoder drops at the start (encoder lookahead x 3).
    pre_skip: u64,
    /// Samples of the next packet, fewer than `FRAME`.
    frame: Vec<f32>,
    out: Vec<u8>,
    packets: u64,
    samples: u64,
}

impl<W: SyncWrite> OpusOgg<W> {
    /// Sets up the R5 encoder and writes the two header pages, each flushed.
    pub fn create(
        inner: W,
        serial: u32,
        clock: Arc<dyn MonotonicClock>,
    ) -> Result<OpusOgg<W>, StorageError> {
        let mut encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip)
            .map_err(encode_failed)?;
        encoder
            .set_bitrate(opus::Bitrate::Bits(BITRATE))
            .map_err(encode_failed)?;
        encoder.set_vbr(true).map_err(encode_failed)?;
        encoder.set_complexity(COMPLEXITY).map_err(encode_failed)?;
        encoder.set_dtx(false).map_err(encode_failed)?;
        encoder.set_inband_fec(false).map_err(encode_failed)?;
        let lookahead = encoder.get_lookahead().map_err(encode_failed)?;
        let pre_skip = u16::try_from(lookahead)
            .ok()
            .and_then(|l| l.checked_mul(3))
            .ok_or(StorageError {
                op: StorageOp::Encode,
                kind: io::ErrorKind::Other,
            })?;

        let mut head = Vec::with_capacity(19);
        head.extend_from_slice(b"OpusHead");
        head.push(1); // version
        head.push(1); // channels
        head.extend_from_slice(&pre_skip.to_le_bytes());
        head.extend_from_slice(&RATE.to_le_bytes()); // input sample rate
        head.extend_from_slice(&0i16.to_le_bytes()); // output gain
        head.push(0); // channel mapping family

        let vendor = format!("voicen {}", env!("CARGO_PKG_VERSION"));
        let vendor_len = u32::try_from(vendor.len()).map_err(|_| StorageError {
            op: StorageOp::Encode,
            kind: io::ErrorKind::Other,
        })?;
        let mut tags = Vec::with_capacity(16 + vendor.len());
        tags.extend_from_slice(b"OpusTags");
        tags.extend_from_slice(&vendor_len.to_le_bytes());
        tags.extend_from_slice(vendor.as_bytes());
        tags.extend_from_slice(&0u32.to_le_bytes()); // no user comments

        let last_sync = clock.now();
        let mut track = OpusOgg {
            pages: PacketWriter::new(inner),
            encoder,
            serial,
            clock,
            last_sync,
            pre_skip: u64::from(pre_skip),
            frame: Vec::with_capacity(FRAME),
            out: vec![0; MAX_PACKET],
            packets: 0,
            samples: 0,
        };
        for header in [head, tags] {
            track.write(header, PacketWriteEndInfo::EndPage, 0)?;
            track.flush()?;
        }
        Ok(track)
    }

    fn write(
        &mut self,
        packet: Vec<u8>,
        end: PacketWriteEndInfo,
        granule: u64,
    ) -> Result<(), StorageError> {
        self.pages
            .write_packet(packet, self.serial, end, granule)
            .map_err(storage(StorageOp::Write))
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.pages
            .inner_mut()
            .flush()
            .map_err(storage(StorageOp::Flush))
    }

    fn sync(&mut self, now: Instant) -> Result<(), StorageError> {
        self.pages
            .inner_mut()
            .sync_data()
            .map_err(storage(StorageOp::Sync))?;
        self.last_sync = now;
        Ok(())
    }

    /// Encodes the full `frame` into one packet. `last` carries the final granule.
    fn encode_frame(&mut self, last: Option<u64>) -> Result<(), StorageError> {
        let n = self
            .encoder
            .encode_float(&self.frame, &mut self.out)
            .map_err(encode_failed)?;
        let packet = self.out.get(..n).unwrap_or_default().to_vec();
        self.frame.clear();
        self.packets += 1;
        let (end, granule) = match last {
            Some(granule) => (PacketWriteEndInfo::EndStream, granule),
            None if self.packets.is_multiple_of(PACKETS_PER_PAGE) => (
                PacketWriteEndInfo::EndPage,
                self.packets * GRANULE_PER_PACKET,
            ),
            None => (
                PacketWriteEndInfo::NormalPacket,
                self.packets * GRANULE_PER_PACKET,
            ),
        };
        self.write(packet, end, granule)?;
        if end == PacketWriteEndInfo::EndPage {
            self.flush()?;
            let now = self.clock.now();
            if now.saturating_duration_since(self.last_sync) >= SYNC_EVERY {
                self.sync(now)?;
            }
        }
        Ok(())
    }
}

impl<W: SyncWrite> TrackEncoder for OpusOgg<W> {
    fn push(&mut self, pcm16k: &[f32]) -> Result<(), StorageError> {
        self.samples += pcm16k.len() as u64;
        let mut rest = pcm16k;
        while !rest.is_empty() {
            let take = (FRAME - self.frame.len()).min(rest.len());
            let (now, later) = rest.split_at(take);
            self.frame.extend_from_slice(now);
            rest = later;
            if self.frame.len() == FRAME {
                self.encode_frame(None)?;
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(), StorageError> {
        // Enough zero-padded packets to flush the encoder's lookahead: the final granule
        // (pre-skip + every input sample) then trims the padding (RFC 7845 end trimming).
        let final_granule = self.pre_skip + GRANULE_PER_SAMPLE * self.samples;
        let packets = final_granule
            .div_ceil(GRANULE_PER_PACKET)
            .max(self.packets + 1);
        while self.packets < packets {
            self.frame.resize(FRAME, 0.0);
            let last = (self.packets + 1 == packets).then_some(final_granule);
            self.encode_frame(last)?;
        }
        self.flush()?;
        let now = self.clock.now();
        self.sync(now)
    }
}

/// How the readable data of a track ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadEnd {
    /// The EOS page was read.
    Complete,
    /// The data ends early (a cut, a zero tail, a bad last page): recoverable up to there.
    Truncated,
    /// A bad page is followed by a valid one: only the prefix before it was read.
    Damaged,
}

/// What a track holds, without decoding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackScan {
    /// Readable audio in 48 kHz units: last good granule - pre-skip.
    pub duration_48k: u64,
    pub end: ReadEnd,
}

/// The pages of a track after its `OpusHead`, CRC-valid prefix only (research R7).
struct Pages<R: Read + Seek> {
    reader: PacketReader<R>,
    pre_skip: u64,
    tags_seen: bool,
    /// Granule of the page of the last packet handed out.
    granule: u64,
    end: Option<ReadEnd>,
}

/// The pre-skip of an `OpusHead` packet (RFC 7845 5.1), or `None` if it is not one.
fn opus_head_pre_skip(packet: &[u8]) -> Option<u64> {
    let rest = packet.strip_prefix(b"OpusHead")?;
    let [version, channels, s0, s1, _, _, _, _, _, _, mapping, ..] = rest else {
        return None;
    };
    let valid = version >> 4 == 0 && (1..=2).contains(channels) && *mapping == 0;
    valid.then(|| u64::from(u16::from_le_bytes([*s0, *s1])))
}

impl<R: Read + Seek> Pages<R> {
    fn open(inner: R) -> Result<Pages<R>, DecodeEnd> {
        let mut reader = PacketReader::new(inner);
        let pre_skip = match reader.read_packet() {
            Ok(Some(head)) if head.last_in_page() => opus_head_pre_skip(&head.data),
            _ => None,
        }
        .ok_or(DecodeEnd::Unreadable)?;
        Ok(Pages {
            reader,
            pre_skip,
            tags_seen: false,
            granule: 0,
            end: None,
        })
    }

    /// The next audio packet of a CRC-valid page, or `None` once the data ended (`end` says
    /// how). Any read error ends the data.
    fn next_audio(&mut self) -> Option<Packet> {
        while self.end.is_none() {
            match self.reader.read_packet() {
                Ok(Some(packet)) => {
                    if packet.last_in_stream() {
                        self.end = Some(ReadEnd::Complete);
                    }
                    if !self.tags_seen {
                        self.tags_seen = true;
                        if packet.data.starts_with(b"OpusTags") {
                            continue;
                        }
                    }
                    self.granule = packet.absgp_page();
                    return Some(packet);
                }
                Ok(None) => self.end = Some(ReadEnd::Truncated),
                Err(OggReadError::HashMismatch(..)) => self.end = Some(self.after_bad_page()),
                Err(_) => self.end = Some(ReadEnd::Truncated),
            }
        }
        None
    }

    /// Damaged iff a CRC-valid page follows the bad one. Each mismatch consumes its page,
    /// so this walks forward to the end of the file at most.
    fn after_bad_page(&mut self) -> ReadEnd {
        loop {
            match self.reader.read_packet() {
                Ok(Some(_)) => return ReadEnd::Damaged,
                Err(OggReadError::HashMismatch(..)) => continue,
                Ok(None) | Err(_) => return ReadEnd::Truncated,
            }
        }
    }

    fn duration_48k(&self) -> u64 {
        self.granule.saturating_sub(self.pre_skip)
    }
}

/// Reads how long a track is and how it ends, without decoding the audio.
pub fn scan<R: Read + Seek>(inner: R) -> Result<TrackScan, DecodeEnd> {
    let mut pages = Pages::open(inner)?;
    while pages.next_audio().is_some() {}
    Ok(TrackScan {
        duration_48k: pages.duration_48k(),
        end: pages.end.unwrap_or(ReadEnd::Truncated),
    })
}

/// The Ogg Opus reader of a call track: the audio of the CRC-valid prefix, the pre-skip
/// dropped and the tail trimmed to the last good granule.
pub struct OpusOggReader<R: Read + Seek> {
    pages: Pages<R>,
    decoder: opus::Decoder,
    /// 16 kHz samples of pre-skip still to drop.
    skip: usize,
    /// 16 kHz samples queued or handed out so far.
    emitted: u64,
    queue: VecDeque<i16>,
    pcm: Vec<i16>,
}

impl<R: Read + Seek> OpusOggReader<R> {
    /// Reads the `OpusHead`; `Err(DecodeEnd::Unreadable)` without a valid one.
    pub fn open(inner: R) -> Result<OpusOggReader<R>, DecodeEnd> {
        let pages = Pages::open(inner)?;
        let decoder =
            opus::Decoder::new(RATE, opus::Channels::Mono).map_err(|_| DecodeEnd::Codec)?;
        let skip = usize::try_from(pages.pre_skip / GRANULE_PER_SAMPLE).unwrap_or(usize::MAX);
        Ok(OpusOggReader {
            pages,
            decoder,
            skip,
            emitted: 0,
            queue: VecDeque::new(),
            pcm: vec![0; MAX_DECODED],
        })
    }

    /// Decodes one packet into the queue, keeping no more than its page's granule allows.
    fn decode(&mut self, packet: &Packet) -> Result<(), DecodeEnd> {
        let n = self
            .decoder
            .decode(&packet.data, &mut self.pcm, false)
            .map_err(|_| DecodeEnd::Codec)?;
        let decoded = self.pcm.get(..n).unwrap_or_default();
        let dropped = self.skip.min(decoded.len());
        self.skip -= dropped;
        let fresh = decoded.get(dropped..).unwrap_or_default();
        let limit = packet.absgp_page().saturating_sub(self.pages.pre_skip) / GRANULE_PER_SAMPLE;
        let room = usize::try_from(limit.saturating_sub(self.emitted)).unwrap_or(usize::MAX);
        let kept = fresh.get(..room.min(fresh.len())).unwrap_or_default();
        self.queue.extend(kept);
        self.emitted += kept.len() as u64;
        Ok(())
    }
}

impl<R: Read + Seek> TrackDecoder for OpusOggReader<R> {
    fn next_window(&mut self, max_ms: u32) -> Result<Option<AudioBuffer>, DecodeEnd> {
        let want = usize::try_from(max_ms.max(1))
            .unwrap_or(usize::MAX)
            .saturating_mul(16);
        while self.queue.len() < want {
            let Some(packet) = self.pages.next_audio() else {
                break;
            };
            self.decode(&packet)?;
        }
        if self.queue.is_empty() {
            return match self.pages.end {
                Some(ReadEnd::Damaged) => Err(DecodeEnd::Damaged),
                _ => Ok(None),
            };
        }
        let n = want.min(self.queue.len());
        Ok(Some(AudioBuffer::from_16k_mono(
            self.queue.drain(..n).collect(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        scan, OpusOgg, OpusOggReader, ReadEnd, SyncWrite, TrackDecoder, TrackEncoder, TrackScan,
    };
    use crate::call::{DecodeEnd, StorageError, StorageOp};
    use crate::clock::MonotonicClock;
    use std::io::{self, Cursor, ErrorKind, Write};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    const SERIAL: u32 = 0x5EED_0116;
    const RATE: usize = 16_000;
    const EOS: u8 = 0x04;
    const BOS: u8 = 0x02;

    // ---- fakes ---------------------------------------------------------------------------

    /// A monotonic clock that moves only when the test says so.
    struct StepClock {
        now: Mutex<Instant>,
    }

    impl StepClock {
        fn new() -> Arc<StepClock> {
            Arc::new(StepClock {
                now: Mutex::new(crate::clock::instant()),
            })
        }

        fn advance(&self, d: Duration) {
            *self.now.lock().unwrap_or_else(PoisonError::into_inner) += d;
        }
    }

    impl MonotonicClock for StepClock {
        fn now(&self) -> Instant {
            *self.now.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    fn dyn_clock(c: &Arc<StepClock>) -> Arc<dyn MonotonicClock> {
        c.clone()
    }

    /// What the inner writer saw, with the byte count at that moment.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Ev {
        Write(usize),
        Flush(usize),
        Sync(usize),
    }

    #[derive(Default)]
    struct SinkState {
        bytes: Vec<u8>,
        events: Vec<Ev>,
        flushes: usize,
        syncs: usize,
        /// A write fails once this many bytes are stored.
        fail_write_at: Option<(usize, ErrorKind)>,
        /// The n-th flush (0-based) and every later one fail.
        fail_flush_from: Option<(usize, ErrorKind)>,
        /// The n-th sync (0-based) and every later one fail.
        fail_sync_from: Option<(usize, ErrorKind)>,
        /// Only the n-th flush (0-based) fails; the ones before and after succeed.
        fail_flush_only: Option<(usize, ErrorKind)>,
        /// Only the n-th sync (0-based) fails; the ones before and after succeed.
        fail_sync_only: Option<(usize, ErrorKind)>,
    }

    /// The counting fake writer: records bytes, flushes and syncs in order; fails on demand.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<SinkState>>);

    impl Sink {
        fn state(&self) -> std::sync::MutexGuard<'_, SinkState> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner)
        }
        fn bytes(&self) -> Vec<u8> {
            self.state().bytes.clone()
        }
        fn events(&self) -> Vec<Ev> {
            self.state().events.clone()
        }
    }

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut s = self.state();
            if let Some((at, kind)) = s.fail_write_at {
                if s.bytes.len() + buf.len() > at {
                    return Err(io::Error::new(kind, "fake write failure"));
                }
            }
            s.bytes.extend_from_slice(buf);
            let n = s.bytes.len();
            s.events.push(Ev::Write(n));
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            let mut s = self.state();
            let i = s.flushes;
            s.flushes += 1;
            if let Some((from, kind)) = s.fail_flush_from {
                if i >= from {
                    return Err(io::Error::new(kind, "fake flush failure"));
                }
            }
            if let Some((only, kind)) = s.fail_flush_only {
                if i == only {
                    return Err(io::Error::new(kind, "fake flush failure"));
                }
            }
            let n = s.bytes.len();
            s.events.push(Ev::Flush(n));
            Ok(())
        }
    }

    impl SyncWrite for Sink {
        fn sync_data(&mut self) -> io::Result<()> {
            let mut s = self.state();
            let i = s.syncs;
            s.syncs += 1;
            if let Some((from, kind)) = s.fail_sync_from {
                if i >= from {
                    return Err(io::Error::new(kind, "fake sync failure"));
                }
            }
            if let Some((only, kind)) = s.fail_sync_only {
                if i == only {
                    return Err(io::Error::new(kind, "fake sync failure"));
                }
            }
            let n = s.bytes.len();
            s.events.push(Ev::Sync(n));
            Ok(())
        }
    }

    // ---- signal --------------------------------------------------------------------------

    /// Deterministic speech-like signal at 16 kHz: a voiced tone with six harmonics and a
    /// gliding pitch, under a 4 Hz syllable envelope with a pause every 3 s, plus noise.
    struct Speech {
        n: u64,
        phase: f64,
        f0: f64,
        noise: f64,
        rng: u32,
    }

    impl Speech {
        fn new(f0: f64, noise: f64, seed: u32) -> Speech {
            Speech {
                n: 0,
                phase: 0.0,
                f0,
                noise,
                rng: seed,
            }
        }

        fn next_sample(&mut self) -> f32 {
            use std::f64::consts::TAU;
            let t = self.n as f64 / RATE as f64;
            self.n += 1;
            let pitch = self.f0 * (1.0 + 0.1 * (TAU * 0.7 * t).sin());
            self.phase = (self.phase + TAU * pitch / RATE as f64) % TAU;
            let voiced: f64 = (1..=6)
                .map(|h| (h as f64 * self.phase).sin() / h as f64)
                .sum();
            let syllable = (TAU * 4.0 * t).sin().max(0.0);
            let envelope = if t % 3.0 > 2.2 { 0.0 } else { syllable };
            self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = f64::from(self.rng >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
            (0.3 * envelope * voiced + self.noise * white) as f32
        }

        fn take(&mut self, n: usize) -> Vec<f32> {
            (0..n).map(|_| self.next_sample()).collect()
        }
    }

    fn speech(n: usize) -> Vec<f32> {
        Speech::new(140.0, 0.01, 7).take(n)
    }

    // ---- Ogg page parser (independent of the code under test) ----------------------------

    #[derive(Debug, Clone)]
    struct Page {
        start: usize,
        end: usize,
        flags: u8,
        granule: u64,
        serial: u32,
        seq: u32,
        packets: Vec<Vec<u8>>,
    }

    /// RFC 3533 page CRC: polynomial 0x04c11db7, no reflection, initial value 0.
    fn ogg_crc(data: &[u8]) -> u32 {
        let mut crc = 0u32;
        for &b in data {
            crc ^= u32::from(b) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04c1_1db7
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    fn le_u32(b: &[u8]) -> u32 {
        u32::from_le_bytes(b[..4].try_into().unwrap())
    }

    /// Every page of a well-formed file, back to back from byte 0, each CRC checked. No
    /// packet of a call track spans a page (Opus packets are far below 255 bytes).
    fn pages(bytes: &[u8]) -> Vec<Page> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let h = &bytes[at..];
            assert!(h.len() >= 27, "short page header at {at}");
            assert_eq!(&h[..4], b"OggS", "capture pattern of page {}", out.len());
            assert_eq!(h[4], 0, "stream structure version");
            let nseg = usize::from(h[26]);
            let lacing = &h[27..27 + nseg];
            let body: usize = lacing.iter().map(|&l| usize::from(l)).sum();
            let end = at + 27 + nseg + body;
            assert!(end <= bytes.len(), "page {} runs past the end", out.len());
            let mut copy = bytes[at..end].to_vec();
            copy[22..26].fill(0);
            assert_eq!(
                ogg_crc(&copy),
                le_u32(&h[22..26]),
                "CRC of page {}",
                out.len()
            );
            assert!(
                lacing.last().is_some_and(|&l| l < 255),
                "page {} ends inside a packet",
                out.len()
            );
            let mut packets = Vec::new();
            let mut cur = Vec::new();
            let mut pos = at + 27 + nseg;
            for &l in lacing {
                cur.extend_from_slice(&bytes[pos..pos + usize::from(l)]);
                pos += usize::from(l);
                if l < 255 {
                    packets.push(std::mem::take(&mut cur));
                }
            }
            out.push(Page {
                start: at,
                end,
                flags: h[5],
                granule: u64::from_le_bytes(h[6..14].try_into().unwrap()),
                serial: le_u32(&h[14..18]),
                seq: le_u32(&h[18..22]),
                packets,
            });
            at = end;
        }
        out
    }

    fn fix_crc(bytes: &mut [u8], p: &Page) {
        bytes[p.start + 22..p.start + 26].fill(0);
        let crc = ogg_crc(&bytes[p.start..p.end]);
        bytes[p.start + 22..p.start + 26].copy_from_slice(&crc.to_le_bytes());
    }

    /// Flips one bit inside page `p`'s body, so its CRC no longer matches.
    fn corrupt_body(bytes: &mut [u8], p: &Page) {
        let mid = p.end - (p.end - p.start) / 3;
        bytes[mid] ^= 0x10;
    }

    // ---- helpers -------------------------------------------------------------------------

    /// libopus's lookahead for the R5 encoder at 16 kHz (pre-skip = lookahead x 3).
    fn lookahead() -> u64 {
        let mut enc =
            opus::Encoder::new(16_000, opus::Channels::Mono, opus::Application::Voip).unwrap();
        u64::try_from(enc.get_lookahead().unwrap()).unwrap()
    }

    fn pre_skip() -> u64 {
        lookahead() * 3
    }

    fn try_encode(
        sink: &Sink,
        clock: &Arc<StepClock>,
        samples: &[f32],
        chunk: usize,
    ) -> Result<(), StorageError> {
        let mut enc = OpusOgg::create(sink.clone(), SERIAL, dyn_clock(clock))?;
        for c in samples.chunks(chunk.max(1)) {
            enc.push(c)?;
        }
        enc.finish()
    }

    fn encode(samples: &[f32]) -> Vec<u8> {
        let sink = Sink::default();
        try_encode(&sink, &StepClock::new(), samples, RATE).expect("encode");
        sink.bytes()
    }

    /// Every window of the file until the reader ends: the samples and how it ended.
    fn decode(bytes: &[u8], max_ms: u32) -> (Vec<i16>, Result<(), DecodeEnd>) {
        let mut reader = match OpusOggReader::open(Cursor::new(bytes.to_vec())) {
            Ok(r) => r,
            Err(e) => return (Vec::new(), Err(e)),
        };
        let mut out = Vec::new();
        let cap = bytes.len() * 400 / max_ms.max(1) as usize + 1_000;
        for _ in 0..cap {
            match reader.next_window(max_ms) {
                Ok(Some(w)) => {
                    let s = w.samples();
                    assert!(!s.is_empty(), "an empty window");
                    assert!(
                        s.len() <= max_ms as usize * 16,
                        "window of {} samples > {max_ms} ms",
                        s.len()
                    );
                    out.extend_from_slice(s);
                }
                Ok(None) => return (out, Ok(())),
                Err(e) => return (out, Err(e)),
            }
        }
        panic!("the reader never ended");
    }

    /// The track decoded straight from its parsed packets with libopus, the first
    /// pre-skip / 3 samples dropped and cut to `n`: what the reader must return.
    fn reference_decode(bytes: &[u8], n: usize) -> Vec<i16> {
        let mut dec = opus::Decoder::new(16_000, opus::Channels::Mono).unwrap();
        let mut out = Vec::new();
        for pkt in pages(bytes)[2..].iter().flat_map(|pg| &pg.packets) {
            let mut buf = [0i16; 1_920];
            let k = dec.decode(pkt, &mut buf, false).unwrap();
            out.extend_from_slice(&buf[..k]);
        }
        let skip = usize::try_from(pre_skip() / 3).unwrap();
        out[skip..skip + n].to_vec()
    }

    fn scan_of(bytes: &[u8]) -> Result<TrackScan, DecodeEnd> {
        scan(Cursor::new(bytes.to_vec()))
    }

    /// Mean energy of 20 ms frames.
    fn envelope(x: impl Iterator<Item = f64>) -> Vec<f64> {
        let v: Vec<f64> = x.collect();
        v.chunks(320)
            .map(|c| c.iter().map(|s| s * s).sum::<f64>() / c.len() as f64)
            .collect()
    }

    fn correlation(a: &[f64], b: &[f64]) -> f64 {
        let n = a.len().min(b.len());
        let (a, b) = (&a[..n], &b[..n]);
        let ma = a.iter().sum::<f64>() / n as f64;
        let mb = b.iter().sum::<f64>() / n as f64;
        let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
        let va: f64 = a.iter().map(|x| (x - ma).powi(2)).sum();
        let vb: f64 = b.iter().map(|y| (y - mb).powi(2)).sum();
        cov / (va.sqrt() * vb.sqrt())
    }

    // ---- page 1 and page 2 (contracts/core-call.md "`mic.opus` / `system.opus`") ----------

    #[test]
    fn page_1_is_bos_with_only_opus_head() {
        // "Page 1 | BOS, OpusHead only: version 1, channels 1, pre-skip = lookahead x 3,
        // input rate 16000, gain 0, mapping 0; granule 0"
        let bytes = encode(&speech(RATE));
        let p = pages(&bytes);
        let head = &p[0];
        assert_eq!(head.flags, BOS, "page 1 flags: BOS only");
        assert_eq!(head.granule, 0);
        assert_eq!(head.seq, 0);
        let mut expected = b"OpusHead".to_vec();
        expected.push(1); // version
        expected.push(1); // channels
        expected.extend_from_slice(&u16::try_from(pre_skip()).unwrap().to_le_bytes());
        expected.extend_from_slice(&16_000u32.to_le_bytes());
        expected.extend_from_slice(&0i16.to_le_bytes()); // output gain
        expected.push(0); // mapping family
        assert_eq!(head.packets, vec![expected], "page 1 holds only OpusHead");
    }

    #[test]
    fn page_2_is_opus_tags_with_vendor_and_no_comments() {
        // "Page 2 | OpusTags: vendor `voicen <version>`, 0 comments; granule 0"; audio starts
        // on a fresh page.
        let bytes = encode(&speech(RATE));
        let p = pages(&bytes);
        let tags = &p[1];
        assert_eq!(tags.flags, 0, "page 2: not BOS, not EOS, not continued");
        assert_eq!(tags.granule, 0);
        assert_eq!(tags.seq, 1);
        let vendor = format!("voicen {}", env!("CARGO_PKG_VERSION"));
        let mut expected = b"OpusTags".to_vec();
        expected.extend_from_slice(&u32::try_from(vendor.len()).unwrap().to_le_bytes());
        expected.extend_from_slice(vendor.as_bytes());
        expected.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(tags.packets, vec![expected], "page 2 holds only OpusTags");
        assert!(p.len() > 2, "audio pages follow the headers");
        for page in &p[2..] {
            for pkt in &page.packets {
                assert!(!pkt.starts_with(b"Opus"), "a header packet among the audio");
            }
        }
    }

    #[test]
    fn pages_share_one_serial_and_count_up() {
        let bytes = encode(&speech(3 * RATE));
        for (i, page) in pages(&bytes).iter().enumerate() {
            assert_eq!(page.serial, SERIAL, "serial of page {i}");
            assert_eq!(page.seq as usize, i, "sequence number of page {i}");
            if i > 0 {
                assert_eq!(page.flags & BOS, 0, "BOS on page {i}");
            }
        }
    }

    // ---- granules, paging, flush, sync --------------------------------------------------

    #[test]
    fn granule_advances_960_per_packet_and_pages_end_every_50_packets() {
        let n = 5 * RATE + 7_777;
        let bytes = encode(&speech(n));
        let p = pages(&bytes);
        let audio = &p[2..];
        let (last, full) = audio.split_last().unwrap();
        let mut packets = 0u64;
        for (i, page) in full.iter().enumerate() {
            assert_eq!(page.packets.len(), 50, "audio page {i}: 50 packets (1 s)");
            packets += 50;
            assert_eq!(page.granule, 960 * packets, "granule of audio page {i}");
        }
        assert!(
            (1..=50).contains(&last.packets.len()),
            "last page: {} packets",
            last.packets.len()
        );
        packets += last.packets.len() as u64;
        let final_granule = pre_skip() + 3 * n as u64;
        assert_eq!(last.granule, final_granule, "final granule");
        assert_eq!(
            packets,
            final_granule.div_ceil(960),
            "packets: enough to flush the lookahead, no more"
        );
    }

    #[test]
    fn every_page_is_flushed_before_the_next_write() {
        let sink = Sink::default();
        try_encode(&sink, &StepClock::new(), &speech(4 * RATE + 100), 3_000).unwrap();
        let bytes = sink.bytes();
        let events = sink.events();
        for (i, page) in pages(&bytes).iter().enumerate().skip(1) {
            assert!(
                events.contains(&Ev::Flush(page.end)),
                "no flush right after page {i} (ends at {}): {events:?}",
                page.end
            );
        }
    }

    /// Pushes `seconds` 1-second chunks, moving the fake clock by `step` before each; the
    /// number of syncs during the pushes (not at create, not at finish).
    fn syncs_during_pushes(step: Duration, seconds: usize) -> (usize, Sink) {
        let sink = Sink::default();
        let clock = StepClock::new();
        let mut enc = OpusOgg::create(sink.clone(), SERIAL, dyn_clock(&clock)).unwrap();
        let before = sink.events().len();
        let mut sig = Speech::new(140.0, 0.01, 7);
        for _ in 0..seconds {
            clock.advance(step);
            enc.push(&sig.take(RATE)).unwrap();
        }
        let syncs = sink.events()[before..]
            .iter()
            .filter(|e| matches!(e, Ev::Sync(_)))
            .count();
        enc.finish().unwrap();
        (syncs, sink)
    }

    #[test]
    fn sync_data_runs_whenever_5_s_of_clock_time_passed() {
        // (clock step per 1 s of audio, seconds, syncs): >= 5 s since the last sync, on
        // the injected clock, not on the audio time.
        for (step_ms, seconds, expected) in [
            (1_000, 12, 2),
            (2_500, 8, 4),
            (4_999, 6, 3),
            (5_000, 3, 3),
            (0, 20, 0),
        ] {
            let (syncs, _) = syncs_during_pushes(Duration::from_millis(step_ms), seconds);
            assert_eq!(syncs, expected, "step {step_ms} ms x {seconds}");
        }
    }

    #[test]
    fn a_sync_follows_a_flushed_page() {
        let (_, sink) = syncs_during_pushes(Duration::from_secs(5), 3);
        let events = sink.events();
        for (i, e) in events.iter().enumerate() {
            if let Ev::Sync(n) = e {
                assert!(
                    events[..i].contains(&Ev::Flush(*n)),
                    "sync at {n} bytes without a flush of those bytes: {events:?}"
                );
            }
        }
    }

    #[test]
    fn finish_flushes_and_syncs_after_the_last_page() {
        // R6 "Stop": the EOS page is written, then the file is synced.
        let sink = Sink::default();
        try_encode(&sink, &StepClock::new(), &speech(RATE / 2), RATE).unwrap();
        let total = sink.bytes().len();
        let events = sink.events();
        let last_write = events
            .iter()
            .rposition(|e| matches!(e, Ev::Write(_)))
            .unwrap();
        let tail = &events[last_write + 1..];
        assert!(tail.contains(&Ev::Flush(total)), "no final flush: {tail:?}");
        assert_eq!(
            tail.last(),
            Some(&Ev::Sync(total)),
            "no final sync: {tail:?}"
        );
    }

    #[test]
    fn push_in_any_chunking_writes_the_same_bytes() {
        let sig = speech(3 * RATE + 211);
        let whole = encode(&sig);
        for chunk in [1, 7, 319, 321, 4_801] {
            let sink = Sink::default();
            try_encode(&sink, &StepClock::new(), &sig, chunk).unwrap();
            assert!(sink.bytes() == whole, "chunks of {chunk} samples");
        }
    }

    // ---- end of stream ------------------------------------------------------------------

    #[test]
    fn last_page_carries_eos_and_its_granule_trims_the_tail() {
        for n in [0, 1, 319, 320, 321, RATE, 2 * RATE + 1, 50 * 320 * 2] {
            let sig = speech(n);
            let bytes = encode(&sig);
            let p = pages(&bytes);
            let (last, rest) = p.split_last().unwrap();
            assert_eq!(last.flags & EOS, EOS, "n={n}: EOS on the last page");
            assert!(
                !last.packets.is_empty(),
                "n={n}: the EOS page carries packets"
            );
            assert!(
                rest.iter().all(|pg| pg.flags & EOS == 0),
                "n={n}: EOS before the last page"
            );
            assert_eq!(
                last.granule,
                pre_skip() + 3 * n as u64,
                "n={n}: final granule"
            );
            let (samples, end) = decode(&bytes, 1_000);
            assert!(end.is_ok(), "n={n}: {end:?}");
            assert_eq!(samples.len(), n, "n={n}: decoded length = input length");
            let s = scan_of(&bytes).unwrap();
            assert_eq!(s.end, ReadEnd::Complete, "n={n}");
            assert_eq!(s.duration_48k, 3 * n as u64, "n={n}: duration");
        }
    }

    // ---- size (SC-007: two 60-minute tracks <= 40 MB) -----------------------------------

    /// A writer that only counts (60 minutes need no copy of the bytes).
    #[derive(Clone, Default)]
    struct Counter(Arc<Mutex<(usize, Vec<u8>)>>);

    impl Write for Counter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut s = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            s.0 += buf.len();
            // Keep the last 64 KiB: the final page is read from it.
            s.1.extend_from_slice(buf);
            if s.1.len() > 128 * 1024 {
                let cut = s.1.len() - 64 * 1024;
                s.1.drain(..cut);
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl SyncWrite for Counter {
        fn sync_data(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Encodes 60 minutes of `sig` (30 s generated once, looped: the generator, not
    /// libopus, is the slow part in a debug build); returns (bytes, granule of the last
    /// page).
    fn sixty_minutes(mut sig: Speech) -> (usize, u64) {
        let block = sig.take(30 * RATE);
        let counter = Counter::default();
        let clock = StepClock::new();
        let mut enc = OpusOgg::create(counter.clone(), SERIAL, dyn_clock(&clock)).unwrap();
        for _ in 0..120 {
            for second in block.chunks(RATE) {
                clock.advance(Duration::from_secs(1));
                enc.push(second).unwrap();
            }
        }
        enc.finish().unwrap();
        let s = counter.0.lock().unwrap_or_else(PoisonError::into_inner);
        let tail = &s.1;
        let at = tail
            .windows(4)
            .rposition(|w| w == b"OggS")
            .expect("a last page");
        let last = &pages(&tail[at..])[0];
        assert_eq!(last.flags & EOS, EOS);
        (s.0, last.granule)
    }

    const TRACK_BUDGET: usize = 20_000_000; // 40 MB for two tracks

    #[test]
    fn sixty_minutes_of_the_mic_track_fit_in_half_of_40_mb() {
        let (size, granule) = sixty_minutes(Speech::new(140.0, 0.01, 7));
        assert_eq!(
            granule,
            pre_skip() + 3 * 3_600 * RATE as u64,
            "all 60 min encoded"
        );
        assert!(size <= TRACK_BUDGET, "mic track: {size} B");
        eprintln!("mic track, 60 min: {size} B");
    }

    #[test]
    fn sixty_minutes_of_the_system_track_fit_in_half_of_40_mb() {
        let (size, granule) = sixty_minutes(Speech::new(210.0, 0.03, 11));
        assert_eq!(
            granule,
            pre_skip() + 3 * 3_600 * RATE as u64,
            "all 60 min encoded"
        );
        assert!(size <= TRACK_BUDGET, "system track: {size} B");
        eprintln!("system track, 60 min: {size} B");
    }

    // ---- R5 encoder settings and next_window --------------------------------------------

    #[test]
    fn encoder_uses_the_r5_settings() {
        // "Opus Voip, 16 kHz encoder, 20 ms packets, VBR 24 kbit/s", DTX off.
        let mut sig = speech(60 * RATE);
        sig[10 * RATE..12 * RATE].fill(0.0); // 2 s of digital silence
        let bytes = encode(&sig);
        let p = pages(&bytes);
        let packets: Vec<&Vec<u8>> = p[2..].iter().flat_map(|pg| &pg.packets).collect();
        for (i, pkt) in packets.iter().enumerate() {
            assert_eq!(
                opus::packet::get_nb_samples(pkt, 48_000).unwrap(),
                960,
                "packet {i}"
            );
            assert_eq!(opus::packet::get_nb_frames(pkt).unwrap(), 1, "packet {i}");
            assert_eq!(
                opus::packet::get_nb_channels(pkt).unwrap(),
                opus::Channels::Mono,
                "packet {i}"
            );
            assert!(
                matches!(
                    opus::packet::get_bandwidth(pkt).unwrap(),
                    opus::Bandwidth::Narrowband
                        | opus::Bandwidth::Mediumband
                        | opus::Bandwidth::Wideband
                ),
                "packet {i}: a 16 kHz encoder never goes above wideband"
            );
        }
        // DTX off: no 1-2 byte "nothing to send" packets, even in digital silence.
        let silent = &packets[10 * 50 + 5..12 * 50 - 5];
        assert!(silent.iter().all(|p| p.len() > 2), "DTX packets in silence");
        // VBR: silence costs less than speech.
        let mean = |s: &[&Vec<u8>]| s.iter().map(|p| p.len()).sum::<usize>() / s.len();
        let speech_mean = mean(&packets[20 * 50..30 * 50]);
        assert!(
            mean(silent) * 2 < speech_mean,
            "VBR: silence {} B vs speech {speech_mean} B",
            mean(silent)
        );
        // 24 kbit/s: 60 s of audio come to about 180 KB.
        let bits_per_s = bytes.len() as f64 * 8.0 / 60.0;
        assert!(
            (18_000.0..=30_000.0).contains(&bits_per_s),
            "{bits_per_s} bit/s"
        );
        // Voip: speech is coded in SILK (TOC configs 0-11) or hybrid (12-15), never CELT.
        let celt = packets.iter().filter(|p| p[0] >> 3 >= 16).count();
        assert_eq!(celt, 0, "CELT-only packets in a Voip speech track");
    }

    #[test]
    fn next_window_returns_at_most_max_ms_of_16k_audio_and_the_whole_track() {
        let n = 7 * RATE + 123;
        let sig = speech(n);
        let bytes = encode(&sig);
        let (reference, end) = decode(&bytes, 1_000);
        assert!(end.is_ok());
        assert_eq!(reference.len(), n);
        assert!(
            reference == reference_decode(&bytes, n),
            "not the packets' audio with the pre-skip trimmed"
        );
        // The decoded audio follows the input's envelope (not silence, not shifted).
        let a = envelope(sig.iter().map(|&s| f64::from(s)));
        let b = envelope(reference.iter().map(|&s| f64::from(s) / 32_768.0));
        let r = correlation(&a, &b);
        assert!(r > 0.9, "envelope correlation {r}");
        for max_ms in [1, 7, 20, 333, 30_000] {
            let (samples, end) = decode(&bytes, max_ms); // decode() checks the window bound
            assert!(end.is_ok(), "max_ms={max_ms}: {end:?}");
            assert!(samples == reference, "max_ms={max_ms}: other audio");
        }
    }

    // ---- reading truncated, damaged and unreadable files (R7) --------------------------

    struct Fixture {
        bytes: Vec<u8>,
        pages: Vec<Page>,
        full: Vec<i16>,
    }

    fn fixture() -> Fixture {
        let bytes = encode(&speech(6 * RATE + 4_321));
        let pages = pages(&bytes);
        let (full, end) = decode(&bytes, 1_000);
        assert!(end.is_ok(), "{end:?}");
        assert!(
            full == reference_decode(&bytes, 6 * RATE + 4_321),
            "fixture decode"
        );
        Fixture { bytes, pages, full }
    }

    /// The reader stops after page `good` (its last good page): scan gives `end` and the
    /// duration up to that page; the windows are exactly the full decode's prefix, ending
    /// with `Ok(None)` (recoverable) or `Err(Damaged)`.
    fn assert_prefix(f: &Fixture, bytes: &[u8], good: usize, end: ReadEnd, what: &str) {
        let dur = f.pages[good].granule - pre_skip();
        let s = scan_of(bytes).unwrap_or_else(|e| panic!("{what}: scan {e:?}"));
        assert_eq!(s.end, end, "{what}");
        assert_eq!(
            s.duration_48k, dur,
            "{what}: duration = last good granule - pre-skip"
        );
        let (samples, how) = decode(bytes, 500);
        match end {
            ReadEnd::Damaged => {
                assert!(matches!(how, Err(DecodeEnd::Damaged)), "{what}: {how:?}");
            }
            _ => assert!(how.is_ok(), "{what}: {how:?}"),
        }
        assert_eq!(samples.len() as u64, dur / 3, "{what}: decoded length");
        assert!(
            samples[..] == f.full[..samples.len()],
            "{what}: not the prefix"
        );
    }

    #[test]
    fn a_cut_anywhere_ends_recoverable_data_at_the_last_whole_page() {
        // A crash leaves the file cut: before a page header (Ok(None)), inside the 27-byte
        // header (Ok(None)), inside the segment table or body (UnexpectedEof).
        let f = fixture();
        let last = f.pages.len() - 1;
        for k in 3..=last {
            let pg = &f.pages[k];
            let nseg = usize::from(f.bytes[pg.start + 26]);
            for cut in [
                pg.start,
                pg.start + 10,
                pg.start + 27 + nseg / 2,
                pg.start + 27 + nseg + 3,
                pg.end - 1,
            ] {
                let what = format!("cut at {cut} in page {k}");
                assert_prefix(&f, &f.bytes[..cut], k - 1, ReadEnd::Truncated, &what);
            }
        }
    }

    #[test]
    fn a_zero_filled_tail_ends_recoverable_data() {
        // The file grew but the data never landed: zeros after the last good page, short
        // (inside a page) and longer than the reader's capture-pattern search.
        let f = fixture();
        let k = 4;
        for (cut, zeros) in [
            (f.pages[k].start, 4_096),
            (f.pages[k].start + 40, 4_096),
            (f.pages[k].start, 200 * 1024),
            (f.pages[k].end - 7, 200 * 1024),
        ] {
            let mut bytes = f.bytes[..cut].to_vec();
            bytes.resize(cut + zeros, 0);
            let what = format!("cut at {cut} + {zeros} zero bytes");
            assert_prefix(&f, &bytes, k - 1, ReadEnd::Truncated, &what);
        }
    }

    #[test]
    fn a_crc_mismatch_on_the_last_page_ends_recoverable_data() {
        let f = fixture();
        let last = f.pages.len() - 1;
        // The EOS page of a finished file.
        let mut bytes = f.bytes.clone();
        corrupt_body(&mut bytes, &f.pages[last]);
        assert_prefix(&f, &bytes, last - 1, ReadEnd::Truncated, "EOS page");
        // The last page of a file cut after page 4.
        let mut bytes = f.bytes[..f.pages[4].end].to_vec();
        corrupt_body(&mut bytes, &f.pages[4]);
        assert_prefix(&f, &bytes, 3, ReadEnd::Truncated, "last page of a cut file");
        // A torn last page: its granule bytes changed, body intact.
        let mut bytes = f.bytes.clone();
        bytes[f.pages[last].start + 7] ^= 0x01;
        assert_prefix(&f, &bytes, last - 1, ReadEnd::Truncated, "EOS page header");
    }

    #[test]
    fn a_crc_mismatch_before_the_last_page_gives_a_damaged_file_with_its_prefix() {
        let f = fixture();
        let last = f.pages.len() - 1;
        for k in 3..last {
            let mut bytes = f.bytes.clone();
            corrupt_body(&mut bytes, &f.pages[k]);
            assert_prefix(
                &f,
                &bytes,
                k - 1,
                ReadEnd::Damaged,
                &format!("page {k} body"),
            );
            let mut bytes = f.bytes.clone();
            bytes[f.pages[k].start + 7] ^= 0x01;
            assert_prefix(
                &f,
                &bytes,
                k - 1,
                ReadEnd::Damaged,
                &format!("page {k} header"),
            );
        }
        // A bad page, then a cut inside the next one: no CRC-valid page follows -> recoverable.
        let mut bytes = f.bytes[..f.pages[5].end - 3].to_vec();
        corrupt_body(&mut bytes, &f.pages[4]);
        assert_prefix(&f, &bytes, 3, ReadEnd::Truncated, "bad page 4, page 5 cut");
    }

    /// Corrupts pages `from..from + run`: the body of even-numbered pages, a header byte
    /// (granule) of odd-numbered ones, so a run mixes both kinds of CRC mismatch.
    fn corrupt_run(f: &Fixture, from: usize, run: usize) -> Vec<u8> {
        let mut bytes = f.bytes.clone();
        for (i, p) in f.pages.iter().enumerate().skip(from).take(run) {
            if i % 2 == 0 {
                corrupt_body(&mut bytes, p);
            } else {
                bytes[p.start + 7] ^= 0x01;
            }
        }
        bytes
    }

    #[test]
    fn consecutive_bad_pages_before_a_valid_page_give_a_damaged_file_with_its_prefix() {
        // Damaged iff a CRC-valid page follows the bad one, however many bad pages lie
        // between: a run of 2 or 3 CRC-invalid pages, then at least one valid page.
        let f = fixture();
        let last = f.pages.len() - 1;
        for run in [2, 3] {
            for k in 3..=last - run {
                let bytes = corrupt_run(&f, k, run);
                let what = format!("bad pages {k}..{}, page {} valid", k + run - 1, k + run);
                assert_prefix(&f, &bytes, k - 1, ReadEnd::Damaged, &what);
            }
        }
    }

    #[test]
    fn consecutive_bad_pages_up_to_the_end_of_the_file_end_recoverable_data() {
        // No CRC-valid page after the run: the data ends at the last good page.
        let f = fixture();
        let last = f.pages.len() - 1;
        for k in 3..last {
            let bytes = corrupt_run(&f, k, last + 1 - k);
            let what = format!("bad pages {k}..{last}");
            assert_prefix(&f, &bytes, k - 1, ReadEnd::Truncated, &what);
        }
    }

    #[test]
    fn a_file_without_a_valid_opus_head_is_unreadable() {
        let f = fixture();
        let head = &f.pages[0];
        let mut cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("zeros", vec![0; 4_096]),
            ("not ogg", b"RIFF\0\0\0\0WAVEfmt ".repeat(64)),
            ("cut inside page 1", f.bytes[..head.end - 5].to_vec()),
            ("page 1 missing", f.bytes[head.end..].to_vec()),
        ];
        let mut bad_crc = f.bytes.clone();
        corrupt_body(&mut bad_crc, head);
        cases.push(("page 1 CRC mismatch", bad_crc));
        // A CRC-valid page 1 whose packet is not an OpusHead.
        let mut magic = f.bytes.clone();
        let at = head.start + 27 + usize::from(f.bytes[head.start + 26]) + 7;
        assert_eq!(magic[at], b'd');
        magic[at] = b'X';
        fix_crc(&mut magic, head);
        cases.push(("OpusHeaX", magic));
        for (what, bytes) in cases {
            assert!(
                matches!(scan_of(&bytes), Err(DecodeEnd::Unreadable)),
                "{what}: scan"
            );
            assert!(
                matches!(
                    OpusOggReader::open(Cursor::new(bytes)),
                    Err(DecodeEnd::Unreadable)
                ),
                "{what}: open"
            );
        }
    }

    // ---- storage failures (FR-016) ------------------------------------------------------

    /// Encodes 8 s, the clock moving 1 s per second of audio; the first error.
    fn first_error(sink: &Sink) -> Option<StorageError> {
        try_encode_timed(sink, 8).err()
    }

    fn try_encode_timed(sink: &Sink, seconds: usize) -> Result<(), StorageError> {
        let clock = StepClock::new();
        let mut enc = OpusOgg::create(sink.clone(), SERIAL, dyn_clock(&clock))?;
        let mut sig = Speech::new(140.0, 0.01, 7);
        for _ in 0..seconds {
            clock.advance(Duration::from_secs(1));
            enc.push(&sig.take(RATE))?;
        }
        enc.finish()
    }

    fn assert_storage(e: Option<StorageError>, op: StorageOp, kind: ErrorKind, what: &str) {
        let e = e.unwrap_or_else(|| panic!("{what}: the failure was swallowed"));
        assert_eq!((e.op, e.kind), (op, kind), "{what}");
    }

    #[test]
    fn a_write_error_maps_to_storage_error_write() {
        for (at, kind) in [
            (0, ErrorKind::StorageFull),
            (60, ErrorKind::PermissionDenied),
            (3_000, ErrorKind::StorageFull),
            (20_000, ErrorKind::Other),
        ] {
            let sink = Sink::default();
            sink.state().fail_write_at = Some((at, kind));
            assert_storage(
                first_error(&sink),
                StorageOp::Write,
                kind,
                &format!("write fails at {at} B"),
            );
        }
    }

    #[test]
    fn a_flush_error_maps_to_storage_error_flush() {
        for (from, kind) in [(0, ErrorKind::BrokenPipe), (3, ErrorKind::TimedOut)] {
            let sink = Sink::default();
            sink.state().fail_flush_from = Some((from, kind));
            assert_storage(
                first_error(&sink),
                StorageOp::Flush,
                kind,
                &format!("flush {from} fails"),
            );
        }
    }

    #[test]
    fn a_sync_error_maps_to_storage_error_sync() {
        // The periodic sync (5 s on the clock) and the final one at finish.
        for (from, kind) in [(0, ErrorKind::TimedOut), (1, ErrorKind::PermissionDenied)] {
            let sink = Sink::default();
            sink.state().fail_sync_from = Some((from, kind));
            assert_storage(
                first_error(&sink),
                StorageOp::Sync,
                kind,
                &format!("sync {from} fails"),
            );
        }
        let sink = Sink::default();
        sink.state().fail_sync_from = Some((0, ErrorKind::Unsupported));
        let err = try_encode(&sink, &StepClock::new(), &speech(3 * RATE), RATE).err();
        assert_storage(
            err,
            StorageOp::Sync,
            ErrorKind::Unsupported,
            "final sync fails",
        );
    }

    /// Pushes 20 ms chunks (one packet each), the clock moving 20 ms before each, until a
    /// `push` fails: the 0-based index of that push, its error, and the sink's events at
    /// that moment. Panics if every push of `seconds` succeeds (the failure was swallowed
    /// mid-recording; `finish` is never reached).
    fn first_failing_push(sink: &Sink, seconds: usize) -> (usize, StorageError, Vec<Ev>) {
        let clock = StepClock::new();
        let mut enc = OpusOgg::create(sink.clone(), SERIAL, dyn_clock(&clock)).unwrap();
        let mut sig = Speech::new(140.0, 0.01, 7);
        for i in 0..seconds * 50 {
            clock.advance(Duration::from_millis(20));
            if let Err(e) = enc.push(&sig.take(320)) {
                return (i, e, sink.events());
            }
        }
        panic!("no push failed in {seconds} s: the mid-recording failure was swallowed");
    }

    #[test]
    fn push_fails_on_the_page_whose_flush_fails() {
        // FR-016: a mid-recording write failure stops the recording. Flushes 0 and 1 are
        // the header pages; flush 2 + k ends audio page k + 1, i.e. packet 50 (k + 1).
        // Only that one flush fails: the error must come from that push, not from a later
        // flush or from finish.
        for (only, kind) in [
            (2, ErrorKind::BrokenPipe),
            (4, ErrorKind::TimedOut),
            (6, ErrorKind::StorageFull),
        ] {
            let sink = Sink::default();
            sink.state().fail_flush_only = Some((only, kind));
            let (at, e, events) = first_failing_push(&sink, 8);
            let page = only - 1;
            assert_eq!(
                (at, e.op, e.kind),
                (page * 50 - 1, StorageOp::Flush, kind),
                "flush {only} fails: the push closing audio page {page} returns it"
            );
            // Nothing runs after the failed flush: no periodic sync on unflushed data.
            let last_flush_ok = events.iter().rposition(|e| matches!(e, Ev::Flush(_)));
            let last_write = events.iter().rposition(|e| matches!(e, Ev::Write(_)));
            assert!(
                last_write > last_flush_ok
                    && !events[last_write.unwrap_or(0)..]
                        .iter()
                        .any(|e| matches!(e, Ev::Sync(_))),
                "flush {only} fails: the page's bytes stay unflushed and unsynced: {events:?}"
            );
        }
    }

    #[test]
    fn push_fails_on_the_page_whose_periodic_sync_fails() {
        // The clock moves 20 ms per packet: 5 s pass at the end of every 5th audio page
        // (packet 250 k), where the periodic sync k - 1 (0-based) runs. Only that one sync
        // fails: the error must come from that push, not from finish.
        for (only, kind) in [(0, ErrorKind::TimedOut), (1, ErrorKind::PermissionDenied)] {
            let sink = Sink::default();
            sink.state().fail_sync_only = Some((only, kind));
            let (at, e, events) = first_failing_push(&sink, 12);
            assert_eq!(
                (at, e.op, e.kind),
                ((only + 1) * 250 - 1, StorageOp::Sync, kind),
                "sync {only} fails: the push closing audio page {} returns it",
                (only + 1) * 5
            );
            // The failing sync came right after its page was flushed; nothing was written
            // after it.
            let total = sink.bytes().len();
            assert_eq!(
                events.last(),
                Some(&Ev::Flush(total)),
                "sync {only} fails: the flushed page is the last thing written: {events:?}"
            );
        }
    }

    // ---- the production writer ----------------------------------------------------------

    #[test]
    fn a_track_written_to_a_file_reads_back() {
        let path = std::env::temp_dir().join(format!(
            "voicen-t116-{}-{SERIAL:x}.opus",
            std::process::id()
        ));
        let sig = speech(2 * RATE);
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = OpusOgg::create(file, SERIAL, dyn_clock(&StepClock::new())).unwrap();
        enc.push(&sig).unwrap();
        enc.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let s = scan(std::fs::File::open(&path).unwrap());
        std::fs::remove_file(&path).unwrap();
        assert!(
            bytes == encode(&sig),
            "the file holds what the fake writer got"
        );
        let s = s.unwrap();
        assert_eq!(s.end, ReadEnd::Complete);
        assert_eq!(s.duration_48k, 3 * sig.len() as u64);
    }
}
