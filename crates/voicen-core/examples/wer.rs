//! Word error rate of an OpenAI-compatible transcription engine (T-140; spec 007
//! FR-047, SC-004, research R23, decisions #185, #186). Owner-run: the gate and CI
//! compile and lint this example, never run it.
//!
//! Scoring is `voicen_core::wer` (one computation); WAV goes through
//! `test_support::realtime::read_wav` and `AudioBuffer::from_frames`; each item is
//! one `Engine::transcribe` of `OpenAiCompatibleEngine`. Only WAV is read (no FLAC
//! decoder, #186). The output is ids and counts only: never transcript text,
//! reference text or the key.
//!
//! Modes:
//!
//! - `verify-archive <test-clean.tar.gz>`: the archive's SHA-256 against the one
//!   pinned in the manifest header. Check the MD5 with `md5sum test-clean.tar.gz`
//!   too (the published one is in the same header).
//! - `librispeech <test-clean-dir>`: every utterance id of the committed manifest
//!   `examples/wer-librispeech-test-clean.txt`, read as
//!   `<dir>/<spk>/<ch>/<id>.wav` with the reference from `<dir>/<spk>/<ch>/<spk>-<ch>.trans.txt`,
//!   language `en`. Refused when the WAVs found hold less than 30 min of audio.
//! - `pairs <dir> [language]`: every `<name>.wav` in `<dir>` with a `<name>.txt`
//!   reference next to it (for example the owner's Russian calls, language `ru`).
//!
//! Running it: Rust runs only in the `core` image, through `scripts/tw-run core`,
//! which mounts only the repository and forwards no variable of its own. The
//! variables and the data directory go through its `TW_DOCKER_ARGS` hook. Pass the
//! variables by name only (`-e VOICEN_WER_KEY`, never `-e VOICEN_WER_KEY=...`), so
//! the key is never on a command line; export them in the calling shell first.
//! Mount the data read-only at `/data`, from an absolute host path without spaces
//! (`TW_DOCKER_ARGS` is split on whitespace).
//!
//! Preparing LibriSpeech test-clean (https://www.openslr.org/resources/12/test-clean.tar.gz,
//! CC BY 4.0), on the host except the `scripts/tw-run` lines:
//!
//! ```text
//! md5sum test-clean.tar.gz                  # must match archive-md5 in the manifest
//! TW_DOCKER_ARGS="-v <archive-dir>:/data:ro" scripts/tw-run core -- \
//!   cargo run -p voicen-core --example wer -- verify-archive /data/test-clean.tar.gz
//! tar -xzf test-clean.tar.gz                # gives LibriSpeech/test-clean/
//! # then, once per FLAC file X.flac (the one external conversion):
//! ffmpeg -i X.flac -ar 16000 -ac 1 -c:a pcm_s16le X.wav
//! export VOICEN_WER_KEY VOICEN_WER_BASE_URL VOICEN_WER_MODEL   # set beforehand
//! TW_DOCKER_ARGS="-e VOICEN_WER_KEY -e VOICEN_WER_BASE_URL -e VOICEN_WER_MODEL \
//!   -v <abs>/LibriSpeech/test-clean:/data:ro" scripts/tw-run core -- \
//!   cargo run -p voicen-core --example wer -- librispeech /data
//! ```
//!
//! Pairs (for example the owner's Russian calls):
//!
//! ```text
//! TW_DOCKER_ARGS="-e VOICEN_WER_KEY -e VOICEN_WER_BASE_URL -e VOICEN_WER_MODEL \
//!   -v <pairs-dir>:/data:ro" scripts/tw-run core -- \
//!   cargo run -p voicen-core --example wer -- pairs /data ru
//! ```
//!
//! Environment: `VOICEN_WER_KEY` (the API key; unset or empty = no
//! `Authorization` header), `VOICEN_WER_BASE_URL` (default
//! `https://api.openai.com/v1`, checked by `check_base_url`), `VOICEN_WER_MODEL`
//! (default `whisper-1`). A variable not forwarded by `-e` (or not set in the
//! calling shell) is unset inside the container, so its default applies; for the
//! key that means requests without `Authorization`.
//!
//! Exit 0: scored, no item failed. Exit 1: refused (too little audio, checksum
//! mismatch, nothing to score) or at least one item failed. Exit 2: wrong usage or
//! configuration.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use sha2::{Digest, Sha256};
use voicen_core::audio::AudioBuffer;
use voicen_core::engine::openai::OpenAiCompatibleEngine;
use voicen_core::engine::{Engine, TranscribeRequest};
use voicen_core::secrets::Secret;
use voicen_core::settings::url::check_base_url;
use voicen_core::test_support::realtime::read_wav;
use voicen_core::timeouts::Timeouts;
use voicen_core::wer::{normalize, score, WerCounts};

/// The committed LibriSpeech manifest: header comments, then one utterance id per line.
const MANIFEST: &str = include_str!("wer-librispeech-test-clean.txt");
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL: &str = "whisper-1";
/// The engine's sample rate (`AudioBuffer` is 16 kHz mono).
const RATE: usize = 16_000;
/// Decision #185: at least 30 min of LibriSpeech audio.
const MIN_LIBRISPEECH_SAMPLES: usize = 30 * 60 * RATE;
/// One utterance is at most about 35 s; the default 30 s API deadline is too tight.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

const USAGE: &str = "usage:
  wer verify-archive <test-clean.tar.gz>
  wer librispeech <test-clean-dir>
  wer pairs <dir> [language]

Run through the core image; variables by name only, data mounted read-only at /data:
  export VOICEN_WER_KEY VOICEN_WER_BASE_URL VOICEN_WER_MODEL
  TW_DOCKER_ARGS=\"-e VOICEN_WER_KEY -e VOICEN_WER_BASE_URL -e VOICEN_WER_MODEL -v <set-dir>:/data:ro\" \\
    scripts/tw-run core -- cargo run -p voicen-core --example wer -- librispeech /data
  (pairs: the same with -v <pairs-dir>:/data:ro and `pairs /data ru`;
   verify-archive: -v <archive-dir>:/data:ro and `verify-archive /data/test-clean.tar.gz`)

Prepare LibriSpeech test-clean (on the host):
  md5sum test-clean.tar.gz        (must match archive-md5 in wer-librispeech-test-clean.txt)
  verify-archive as above
  tar -xzf test-clean.tar.gz
  ffmpeg -i X.flac -ar 16000 -ac 1 -c:a pcm_s16le X.wav   (once per FLAC file)

Environment: VOICEN_WER_KEY, VOICEN_WER_BASE_URL (default https://api.openai.com/v1),
VOICEN_WER_MODEL (default whisper-1); a variable not passed with -e is unset in the container.";

/// One scored item: its id, audio and normalized reference.
struct Item {
    id: String,
    audio: AudioBuffer,
    reference: Vec<String>,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode, archive] if mode == "verify-archive" => verify_archive(Path::new(archive)),
        [mode, dir] if mode == "librispeech" => librispeech(Path::new(dir)),
        [mode, dir] if mode == "pairs" => pairs(Path::new(dir), None),
        [mode, dir, language] if mode == "pairs" => pairs(Path::new(dir), Some(language.clone())),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// The value of a `# <key>: <value>` header line of the manifest.
fn header(key: &str) -> Option<&'static str> {
    MANIFEST.lines().find_map(|line| {
        line.strip_prefix('#')?
            .trim()
            .strip_prefix(key)?
            .strip_prefix(':')
            .map(str::trim)
    })
}

/// The manifest's utterance ids (non-empty, non-comment lines).
fn manifest_ids() -> impl Iterator<Item = &'static str> {
    MANIFEST
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

fn verify_archive(path: &Path) -> ExitCode {
    let Some(pinned) = header("archive-sha256") else {
        eprintln!("wer: the manifest has no archive-sha256 line");
        return ExitCode::from(2);
    };
    let digest = match sha256_hex(path) {
        Ok(digest) => digest,
        Err(e) => {
            eprintln!("wer: cannot read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };
    let md5 = header("archive-md5").unwrap_or("(missing from the manifest)");
    if digest.eq_ignore_ascii_case(pinned) {
        println!("wer: SHA-256 matches the pinned {pinned}");
        println!("wer: check the MD5 with `md5sum` too; expected {md5}");
        ExitCode::SUCCESS
    } else {
        eprintln!("wer: SHA-256 {digest} differs from the pinned {pinned}");
        ExitCode::from(1)
    }
}

fn sha256_hex(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(buf.get(..n).unwrap_or_default());
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn librispeech(dir: &Path) -> ExitCode {
    let mut items = Vec::new();
    let mut missing = 0usize;
    let mut chapters: HashMap<(String, String), HashMap<String, String>> = HashMap::new();
    for id in manifest_ids() {
        let mut parts = id.split('-');
        let (Some(spk), Some(ch), Some(_), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            eprintln!("wer: manifest id {id} is not <speaker>-<chapter>-<utterance>");
            return ExitCode::from(2);
        };
        let chapter_dir = dir.join(spk).join(ch);
        let refs = chapters
            .entry((spk.to_string(), ch.to_string()))
            .or_insert_with(|| {
                read_transcripts(&chapter_dir.join(format!("{spk}-{ch}.trans.txt")))
            });
        let Some(reference) = refs.get(id) else {
            missing += 1;
            continue;
        };
        match load_wav(&chapter_dir.join(format!("{id}.wav"))) {
            Some(audio) => items.push(Item {
                id: id.to_string(),
                audio,
                reference: normalize(reference),
            }),
            None => missing += 1,
        }
    }
    let samples: usize = items.iter().map(|item| item.audio.samples().len()).sum();
    println!(
        "wer: {} of {} manifest utterances found ({missing} missing), {:.1} min of audio",
        items.len(),
        items.len() + missing,
        minutes(samples)
    );
    if samples < MIN_LIBRISPEECH_SAMPLES {
        eprintln!(
            "wer: refused: {:.1} min found, at least {:.0} min required (decision #185); \
             convert every FLAC of the manifest's chapters to WAV first",
            minutes(samples),
            minutes(MIN_LIBRISPEECH_SAMPLES)
        );
        return ExitCode::from(1);
    }
    run(&items, Some("en".to_string()))
}

/// `<id> <text>` lines of a LibriSpeech `.trans.txt`; an unreadable file is empty
/// (its utterances then count as missing).
fn read_transcripts(path: &Path) -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let (id, words) = line.split_once(' ')?;
            Some((id.to_string(), words.to_string()))
        })
        .collect()
}

fn pairs(dir: &Path, language: Option<String>) -> ExitCode {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("wer: cannot list {}: {e}", dir.display());
            return ExitCode::from(2);
        }
    };
    let mut wavs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "wav"))
        .collect();
    wavs.sort();
    let mut items = Vec::new();
    let mut missing = 0usize;
    for wav in wavs {
        let id = wav
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let reference = std::fs::read_to_string(wav.with_extension("txt"));
        match (reference, load_wav(&wav)) {
            (Ok(reference), Some(audio)) => items.push(Item {
                id,
                audio,
                reference: normalize(&reference),
            }),
            _ => {
                println!("{id} skipped: no readable reference .txt or PCM16 WAV");
                missing += 1;
            }
        }
    }
    let samples: usize = items.iter().map(|item| item.audio.samples().len()).sum();
    println!(
        "wer: {} pairs found ({missing} skipped), {:.1} min of audio",
        items.len(),
        minutes(samples)
    );
    run(&items, language)
}

/// A PCM16 WAV as 16 kHz mono; `None` when absent or unreadable.
fn load_wav(path: &Path) -> Option<AudioBuffer> {
    let bytes = std::fs::read(path).ok()?;
    let frames = read_wav(&bytes).ok()?;
    AudioBuffer::from_frames(&frames.samples, frames.rate, frames.channels).ok()
}

fn minutes(samples: usize) -> f64 {
    samples as f64 / RATE as f64 / 60.0
}

/// Transcribes and scores every item; prints one line per item and the set totals.
fn run(items: &[Item], language: Option<String>) -> ExitCode {
    if items.is_empty() {
        eprintln!("wer: nothing to score");
        return ExitCode::from(1);
    }
    let Some(engine) = engine() else {
        return ExitCode::from(2);
    };
    let timeouts = Timeouts {
        api_transcription: REQUEST_TIMEOUT,
        ..Timeouts::default()
    };
    let request = TranscribeRequest { language, timeouts };
    let mut total = WerCounts::default();
    let mut failed = 0usize;
    for item in items {
        match engine.transcribe(&item.audio, &request) {
            Ok(text) => {
                let counts = score(&item.reference, &normalize(&text));
                println!("{} {}", item.id, describe(&counts));
                total.add(&counts);
            }
            Err(reason) => {
                // The code only: a reason's details may carry server text.
                println!("{} failed {}", item.id, reason.code());
                failed += 1;
            }
        }
    }
    println!(
        "wer: total over {} scored items ({failed} failed): {}",
        items.len() - failed,
        describe(&total)
    );
    if failed > 0 {
        eprintln!("wer: {failed} items failed; the total leaves them out");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn describe(counts: &WerCounts) -> String {
    let rate = counts
        .rate()
        .map_or_else(|| "n/a".to_string(), |rate| format!("{:.2}%", rate * 100.0));
    format!(
        "S={} D={} I={} N={} WER={rate}",
        counts.substitutions, counts.deletions, counts.insertions, counts.reference_words
    )
}

/// The engine from `VOICEN_WER_*`; `None` (reason printed, never the key) when the
/// base URL is refused.
fn engine() -> Option<OpenAiCompatibleEngine> {
    let raw_url =
        std::env::var("VOICEN_WER_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let Ok(base_url) = check_base_url(&raw_url) else {
        eprintln!("wer: VOICEN_WER_BASE_URL is not an http(s) URL without userinfo");
        return None;
    };
    let model = std::env::var("VOICEN_WER_MODEL")
        .ok()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let key = std::env::var("VOICEN_WER_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .map(Secret::new);
    Some(OpenAiCompatibleEngine::new(base_url, model, key))
}
