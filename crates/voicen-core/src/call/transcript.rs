//! `transcript.json` schema 1 (requirements §7): the wire types, `validate()` (research R17),
//! `merge()` of both tracks' engine output, and the one Markdown renderer
//! (contracts/core-call.md "`transcript.json`", "`transcript.md`"; T-123, decision #196).
//!
//! Every value here may hold transcript text: nothing in this module logs, and an error
//! carries a rule and a segment index only.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use serde::{Deserialize, Serialize};

use crate::call::{CallId, InvalidRule, Track};
use crate::settings::{is_lang_code, LangCode};

/// The only `schema` value of this type.
const SCHEMA: u32 = 1;
/// The only `source` value of a segment in 1a.
const SOURCE_FINAL: &str = "final";

/// One engine reference, serialized as the one string `"<kind>:<model>"` (`"api:whisper-1"`);
/// the same form in `meta.json` and in `transcript.json` `engines.transcription`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EngineRef(String);

impl EngineRef {
    /// `"<kind>:<model>"`.
    pub fn new(kind: &str, model: &str) -> EngineRef {
        EngineRef(format!("{kind}:{model}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A speaker of the call: `{id, name}`; in 1a exactly `me` and `s1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Speaker {
    pub id: String,
    pub name: String,
}

/// `transcript.json`, schema 1. Closed fields (`track`, `speaker`, `source`, `lang`) are plain
/// strings so that [`TranscriptV1::validate`] is the one gate with an R17 rule name; keys are in
/// the order of requirements §7.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptV1 {
    pub schema: u32,
    pub call_id: CallId,
    pub title: String,
    /// ISO-8601 with the local offset.
    pub started_at: String,
    pub duration_ms: i64,
    pub speakers: Vec<Speaker>,
    pub segments: Vec<SegmentV1>,
    pub engines: EnginesV1,
}

/// One line. `start_ms`/`end_ms` are relative to `started_at`. No `hidden`, no `confidence` in 1a.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentV1 {
    pub start_ms: i64,
    pub end_ms: i64,
    pub track: String,
    pub speaker: String,
    pub lang: Option<String>,
    pub text: String,
    /// Must be `Some("final")`; a missing key is the `bad_source` rule, not a parse error.
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnginesV1 {
    pub transcription: EngineRef,
    /// Always `null` in 1a.
    pub diarization: Option<EngineRef>,
}

/// Why a `transcript.json` text is not a schema-1 value at all (no R17 rule applies).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptParseError {
    /// Not JSON, an unknown key (`hidden`, `confidence`, ...), a missing required key, a wrong type.
    Malformed,
    /// A well-formed document whose `schema` is not 1.
    UnsupportedSchema(i64),
}

impl TranscriptV1 {
    /// Parses a `transcript.json` text: unknown keys and `schema != 1` are refused.
    pub fn from_json(text: &str) -> Result<TranscriptV1, TranscriptParseError> {
        // The schema is read first: a later schema may have keys this type refuses.
        let probe: SchemaProbe =
            serde_json::from_str(text).map_err(|_| TranscriptParseError::Malformed)?;
        if probe.schema != i64::from(SCHEMA) {
            return Err(TranscriptParseError::UnsupportedSchema(probe.schema));
        }
        serde_json::from_str(text).map_err(|_| TranscriptParseError::Malformed)
    }

    /// The compact JSON text, keys in §7 order.
    pub fn to_json(&self) -> String {
        // Strings, integers and sequences only: serialization has no failing path, and an
        // empty text would be refused by `from_json`.
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The first broken R17 rule and the index of its segment, never the text.
    pub fn validate(&self) -> Result<(), (InvalidRule, u32)> {
        let mut previous_start = i64::MIN;
        for (index, segment) in self.segments.iter().enumerate() {
            if let Err(rule) = self.check_segment(segment, previous_start) {
                return Err((rule, u32::try_from(index).unwrap_or(u32::MAX)));
            }
            previous_start = segment.start_ms;
        }
        Ok(())
    }

    fn check_segment(&self, segment: &SegmentV1, previous_start: i64) -> Result<(), InvalidRule> {
        if segment.start_ms < previous_start {
            return Err(InvalidRule::Unsorted);
        }
        if segment.start_ms < 0
            || segment.start_ms > segment.end_ms
            || segment.end_ms > self.duration_ms
        {
            return Err(InvalidRule::OutOfRange);
        }
        if !self.speakers.iter().any(|s| s.id == segment.speaker) {
            return Err(InvalidRule::UnknownSpeaker);
        }
        if ![Track::Mic, Track::System]
            .iter()
            .any(|t| t.as_str() == segment.track)
        {
            return Err(InvalidRule::UnknownTrack);
        }
        if segment.source.as_deref() != Some(SOURCE_FINAL) {
            return Err(InvalidRule::BadSource);
        }
        if segment.lang.as_deref().is_some_and(|l| !is_lang_code(l)) {
            return Err(InvalidRule::BadLang);
        }
        if segment.text.trim().is_empty() {
            return Err(InvalidRule::EmptyText);
        }
        Ok(())
    }
}

/// Reads only `schema`, whatever else the document holds.
#[derive(Deserialize)]
struct SchemaProbe {
    schema: i64,
}

/// An engine segment, in seconds from the chunk start (research R10).
#[derive(Debug, Clone, PartialEq)]
pub struct EngineSegment {
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
    pub lang: Option<LangCode>,
}

/// One uploaded chunk. `start_ms`/`end_ms` are track-relative. `segments: Some(..)` is timed
/// mode (`text` unused); `segments: None` is time-less mode: one line with `text` spanning the chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkInput {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub lang: Option<LangCode>,
    pub segments: Option<Vec<EngineSegment>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackInput {
    pub track: Track,
    /// The track's start offset from `started_at`.
    pub start_offset_ms: u64,
    pub chunks: Vec<ChunkInput>,
}

/// Everything `merge` needs, from the call's `meta.json` and the pass.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeInput {
    pub call_id: CallId,
    pub title: String,
    pub started_at: String,
    pub duration_ms: u64,
    pub speakers: Vec<Speaker>,
    pub engine: EngineRef,
    pub tracks: Vec<TrackInput>,
}

/// Both tracks' lines as one transcript, in the order (start, mic before system, input order).
/// Total over any engine output, and the result passes [`TranscriptV1::validate`] as long as
/// `input.speakers` holds the ids `me` and `s1`, as a call's `meta.json` does.
///
/// A line's time is `offset + clamp(chunk_start + engine_time, chunk)`, then clamped to
/// `duration_ms` with the text kept (#196 (3)). A non-finite or negative engine time counts as 0;
/// an end before its start is the start. A blank text is dropped.
pub fn merge(input: &MergeInput) -> TranscriptV1 {
    let duration_ms = input.duration_ms;
    let mut lines: Vec<(Track, SegmentV1)> = Vec::new();
    for track in &input.tracks {
        for chunk in &track.chunks {
            for line in chunk_lines(chunk) {
                if line.text.trim().is_empty() {
                    continue;
                }
                let on_call =
                    |ms: u64| wire_ms(ms.saturating_add(track.start_offset_ms).min(duration_ms));
                let segment = SegmentV1 {
                    start_ms: on_call(line.start_ms),
                    end_ms: on_call(line.end_ms),
                    track: track.track.as_str().to_string(),
                    speaker: speaker_id(track.track).to_string(),
                    lang: line.lang.map(|l| l.as_str().to_string()),
                    text: line.text.to_string(),
                    source: Some(SOURCE_FINAL.to_string()),
                };
                lines.push((track.track, segment));
            }
        }
    }
    // Stable, so lines with the same key keep the input order.
    lines.sort_by_key(|(track, segment)| (segment.start_ms, track_rank(*track)));
    TranscriptV1 {
        schema: SCHEMA,
        call_id: input.call_id.clone(),
        title: input.title.clone(),
        started_at: input.started_at.clone(),
        duration_ms: wire_ms(duration_ms),
        speakers: input.speakers.clone(),
        segments: lines.into_iter().map(|(_, segment)| segment).collect(),
        engines: EnginesV1 {
            transcription: input.engine.clone(),
            diarization: None,
        },
    }
}

/// A line of a chunk with its times in track time, inside the chunk.
struct ChunkLine<'a> {
    start_ms: u64,
    end_ms: u64,
    text: &'a str,
    lang: Option<LangCode>,
}

fn chunk_lines(chunk: &ChunkInput) -> Vec<ChunkLine<'_>> {
    let chunk_start = chunk.start_ms;
    // Not `clamp`: a reversed chunk would panic it, and merge is total.
    let chunk_end = chunk.end_ms.max(chunk_start);
    let Some(segments) = &chunk.segments else {
        return vec![ChunkLine {
            start_ms: chunk_start,
            end_ms: chunk_end,
            text: &chunk.text,
            lang: chunk.lang,
        }];
    };
    segments
        .iter()
        .map(|segment| {
            let in_chunk = |seconds: f64| {
                chunk_start
                    .saturating_add(engine_ms(seconds))
                    .min(chunk_end)
            };
            let start_ms = in_chunk(segment.start_s);
            ChunkLine {
                start_ms,
                end_ms: in_chunk(segment.end_s).max(start_ms),
                text: &segment.text,
                lang: segment.lang.or(chunk.lang),
            }
        })
        .collect()
}

/// An engine time in whole milliseconds, rounded; a non-finite or negative one is 0.
fn engine_ms(seconds: f64) -> u64 {
    if seconds.is_finite() && seconds > 0.0 {
        // `as` saturates a value too large for u64; the chunk clamp then applies.
        (seconds * 1000.0).round() as u64
    } else {
        0
    }
}

fn wire_ms(ms: u64) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}

/// The fixed speaker of a track until diarization (plan.md, 1c).
fn speaker_id(track: Track) -> &'static str {
    match track {
        Track::Mic => "me",
        Track::System => "s1",
    }
}

fn track_rank(track: Track) -> u8 {
    match track {
        Track::Mic => 0,
        Track::System => 1,
    }
}

/// The only Markdown producer. `title` and the speaker `names` come from `meta.json`, never from
/// the transcript's own copies. A speaker missing from `names` is shown by its id.
///
/// The layout is line-based, so every whitespace run in a title, a name or a line's text is one
/// space here (and an edge run is removed); `transcript.json` keeps the original text. The date
/// and time are the wall-clock part of `started_at`, with no conversion from its offset. A
/// transcript without segments ends after the blank line.
pub fn render_markdown(transcript: &TranscriptV1, title: &str, names: &[Speaker]) -> String {
    let mut out = format!(
        "# {}\n{} \u{b7} {}\n\n",
        one_line(title),
        wall_clock(&transcript.started_at),
        duration_text(transcript.duration_ms),
    );
    for segment in &transcript.segments {
        let name = names
            .iter()
            .find(|s| s.id == segment.speaker)
            .map_or(segment.speaker.as_str(), |s| s.name.as_str());
        out.push_str(&format!(
            "**[{}] {}:** {}\n",
            clock_text(segment.start_ms),
            one_line(name),
            one_line(&segment.text),
        ));
    }
    out
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `YYYY-MM-DD HH:MM` of an ISO-8601 text; a shorter or other text is shown as it is, because
/// `started_at` is not one of the R17 rules and the renderer must not fail on it.
fn wall_clock(started_at: &str) -> String {
    match (started_at.get(..10), started_at.get(11..16)) {
        (Some(date), Some(time)) => format!("{date} {time}"),
        _ => one_line(started_at),
    }
}

/// Whole seconds, rounded down; a negative time is 0.
fn hours_minutes_seconds(ms: i64) -> (i64, i64, i64) {
    let seconds = ms.max(0) / 1000;
    (seconds / 3600, seconds / 60 % 60, seconds % 60)
}

fn clock_text(ms: i64) -> String {
    let (h, m, s) = hours_minutes_seconds(ms);
    format!("{h:02}:{m:02}:{s:02}")
}

fn duration_text(ms: i64) -> String {
    let (h, m, s) = hours_minutes_seconds(ms);
    format!("{h}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers ------------------------------------------------------------------

    const CALL: &str = "2026-10-30_140509";
    const STARTED: &str = "2026-10-30T14:05:09+03:00";

    /// A full valid document in §7 key order, compact.
    const FULL: &str = concat!(
        r#"{"schema":1,"call_id":"2026-10-30_140509","title":"Weekly sync","#,
        r#""started_at":"2026-10-30T14:05:09+03:00","duration_ms":60000,"#,
        r#""speakers":[{"id":"me","name":"Me"},{"id":"s1","name":"Other party"}],"#,
        r#""segments":[{"start_ms":0,"end_ms":2000,"track":"mic","speaker":"me","lang":"en","text":"Hello there","source":"final"},"#,
        r#"{"start_ms":1500,"end_ms":4000,"track":"system","speaker":"s1","lang":null,"text":"Hi","source":"final"}],"#,
        r#""engines":{"transcription":"api:whisper-1","diarization":null}}"#
    );

    fn seg(
        start: i64,
        end: i64,
        track: &str,
        speaker: &str,
        lang: &str,
        text: &str,
        source: &str,
    ) -> String {
        format!(
            r#"{{"start_ms":{start},"end_ms":{end},"track":"{track}","speaker":"{speaker}","lang":{lang},"text":"{text}","source":{source}}}"#
        )
    }

    /// A good line `start` ms .. `start + 100` ms.
    fn ok(start: i64) -> String {
        seg(start, start + 100, "mic", "me", "null", "x", r#""final""#)
    }

    fn doc(duration: i64, segments: &[String]) -> String {
        format!(
            r#"{{"schema":1,"call_id":"{CALL}","title":"t","started_at":"{STARTED}","duration_ms":{duration},"speakers":[{{"id":"me","name":"Me"}},{{"id":"s1","name":"Other party"}}],"segments":[{}],"engines":{{"transcription":"api:whisper-1","diarization":null}}}}"#,
            segments.join(",")
        )
    }

    fn load(text: &str) -> TranscriptV1 {
        match TranscriptV1::from_json(text) {
            Ok(t) => t,
            Err(e) => panic!("fixture does not parse: {e:?}"),
        }
    }

    fn verdict(duration: i64, segments: &[String]) -> Result<(), (InvalidRule, u32)> {
        load(&doc(duration, segments)).validate()
    }

    fn lang(code: &str) -> LangCode {
        match LangCode::parse(code) {
            Some(l) => l,
            None => panic!("not a language code: {code}"),
        }
    }

    fn call_id() -> CallId {
        match CallId::parse(CALL) {
            Some(c) => c,
            None => panic!("bad call id"),
        }
    }

    fn speakers() -> Vec<Speaker> {
        vec![
            Speaker {
                id: "me".into(),
                name: "Me".into(),
            },
            Speaker {
                id: "s1".into(),
                name: "Other party".into(),
            },
        ]
    }

    fn input(duration_ms: u64, tracks: Vec<TrackInput>) -> MergeInput {
        MergeInput {
            call_id: call_id(),
            title: "Weekly sync".into(),
            started_at: STARTED.into(),
            duration_ms,
            speakers: speakers(),
            engine: EngineRef::new("api", "whisper-1"),
            tracks,
        }
    }

    fn es(start_s: f64, end_s: f64, text: &str) -> EngineSegment {
        EngineSegment {
            start_s,
            end_s,
            text: text.into(),
            lang: None,
        }
    }

    fn timed(start_ms: u64, end_ms: u64, segments: Vec<EngineSegment>) -> ChunkInput {
        ChunkInput {
            start_ms,
            end_ms,
            text: String::new(),
            lang: None,
            segments: Some(segments),
        }
    }

    fn timeless(start_ms: u64, end_ms: u64, text: &str) -> ChunkInput {
        ChunkInput {
            start_ms,
            end_ms,
            text: text.into(),
            lang: None,
            segments: None,
        }
    }

    fn track(track: Track, offset: u64, chunks: Vec<ChunkInput>) -> TrackInput {
        TrackInput {
            track,
            start_offset_ms: offset,
            chunks,
        }
    }

    /// One mic track with one timed chunk.
    fn mic_one(
        offset: u64,
        chunk_start: u64,
        chunk_end: u64,
        duration: u64,
        segs: Vec<EngineSegment>,
    ) -> MergeInput {
        input(
            duration,
            vec![track(
                Track::Mic,
                offset,
                vec![timed(chunk_start, chunk_end, segs)],
            )],
        )
    }

    type Row<'a> = (i64, i64, &'a str, &'a str, &'a str);

    fn rows(t: &TranscriptV1) -> Vec<Row<'_>> {
        t.segments
            .iter()
            .map(|s| {
                (
                    s.start_ms,
                    s.end_ms,
                    s.track.as_str(),
                    s.speaker.as_str(),
                    s.text.as_str(),
                )
            })
            .collect()
    }

    // ---- schema 1 types: Acceptance 1 -------------------------------------------------

    #[test]
    fn a_full_document_round_trips_byte_for_byte_in_section_7_key_order() {
        // Bite: reordered struct fields, a renamed key, a skipped field, pretty output.
        assert_eq!(load(FULL).to_json(), FULL);
    }

    #[test]
    fn an_unknown_top_level_key_is_refused() {
        // Bite: dropping deny_unknown_fields on TranscriptV1.
        let text = FULL.replacen(r#""schema":1,"#, r#""schema":1,"extra":true,"#, 1);
        assert_eq!(
            TranscriptV1::from_json(&text),
            Err(TranscriptParseError::Malformed)
        );
    }

    #[test]
    fn a_hidden_key_in_a_segment_is_refused() {
        // Bite: dropping deny_unknown_fields on SegmentV1, or adding a `hidden` field.
        let text = FULL.replacen(r#""text":"Hi","#, r#""text":"Hi","hidden":false,"#, 1);
        assert_eq!(
            TranscriptV1::from_json(&text),
            Err(TranscriptParseError::Malformed)
        );
    }

    #[test]
    fn a_confidence_key_in_a_segment_is_refused() {
        let text = FULL.replacen(r#""text":"Hi","#, r#""text":"Hi","confidence":0.9,"#, 1);
        assert_eq!(
            TranscriptV1::from_json(&text),
            Err(TranscriptParseError::Malformed)
        );
    }

    #[test]
    fn an_unknown_key_in_engines_is_refused() {
        // Bite: dropping deny_unknown_fields on EnginesV1.
        let text = FULL.replacen(
            r#""diarization":null"#,
            r#""diarization":null,"model":"x""#,
            1,
        );
        assert_eq!(
            TranscriptV1::from_json(&text),
            Err(TranscriptParseError::Malformed)
        );
    }

    #[test]
    fn schema_2_is_refused() {
        // Bite: not checking `schema`.
        let text = FULL.replacen(r#""schema":1"#, r#""schema":2"#, 1);
        assert_eq!(
            TranscriptV1::from_json(&text),
            Err(TranscriptParseError::UnsupportedSchema(2))
        );
    }

    #[test]
    fn schema_0_is_refused() {
        // Bite: checking `schema >= 1` or `!= 2`.
        let text = FULL.replacen(r#""schema":1"#, r#""schema":0"#, 1);
        assert_eq!(
            TranscriptV1::from_json(&text),
            Err(TranscriptParseError::UnsupportedSchema(0))
        );
    }

    #[test]
    fn text_that_is_not_json_is_malformed() {
        assert_eq!(
            TranscriptV1::from_json("{"),
            Err(TranscriptParseError::Malformed)
        );
    }

    #[test]
    fn engines_carry_the_api_model_string_and_a_null_diarization() {
        // Bite: engines as an object, a different prefix, a non-null diarization.
        let t = load(FULL);
        assert_eq!(t.engines.transcription.as_str(), "api:whisper-1");
        assert_eq!(t.engines.diarization, None);
        assert_eq!(t.schema, 1);
    }

    #[test]
    fn the_engine_ref_is_built_as_kind_colon_model() {
        assert_eq!(
            EngineRef::new("api", "gpt-4o-transcribe").as_str(),
            "api:gpt-4o-transcribe"
        );
    }

    #[test]
    fn a_merged_transcript_has_no_hidden_or_confidence_key() {
        // Bite: a `hidden`/`confidence` field on the type, serialized as null.
        let t = merge(&mic_one(0, 0, 10_000, 10_000, vec![es(1.0, 2.0, "a")]));
        let json = t.to_json();
        assert!(!json.contains("hidden"), "{json}");
        assert!(!json.contains("confidence"), "{json}");
    }

    // ---- shared wire names ------------------------------------------------------------

    #[test]
    fn track_as_str_is_the_wire_name_of_the_diag_table() {
        // Pinned by tests/diag_format.rs (`track=` lines). Bite: any other spelling.
        assert_eq!(Track::Mic.as_str(), "mic");
        assert_eq!(Track::System.as_str(), "system");
    }

    #[test]
    fn invalid_rule_as_str_is_the_wire_name_of_the_diag_table() {
        // Pinned by tests/diag_format.rs RULES_ALL. Bite: any spelling or two rules swapped.
        let all = [
            (InvalidRule::Unsorted, "unsorted"),
            (InvalidRule::OutOfRange, "out_of_range"),
            (InvalidRule::UnknownSpeaker, "unknown_speaker"),
            (InvalidRule::UnknownTrack, "unknown_track"),
            (InvalidRule::BadSource, "bad_source"),
            (InvalidRule::BadLang, "bad_lang"),
            (InvalidRule::EmptyText, "empty_text"),
        ];
        for (rule, name) in all {
            assert_eq!(rule.as_str(), name);
        }
    }

    // ---- validate: Acceptance 2 (one fixture per rule) ---------------------------------

    #[test]
    fn a_valid_document_passes_validate() {
        assert_eq!(load(FULL).validate(), Ok(()));
    }

    #[test]
    fn the_boundaries_pass_validate() {
        // 0 == start == end == duration, equal starts, a null lang. Bite: `<` for `<=`.
        let segs = [
            seg(0, 0, "mic", "me", "null", "x", r#""final""#),
            seg(0, 5000, "system", "s1", r#""ru""#, "x", r#""final""#),
            seg(5000, 5000, "mic", "me", "null", "x", r#""final""#),
        ];
        assert_eq!(verdict(5000, &segs), Ok(()));
    }

    #[test]
    fn a_document_without_segments_passes_validate() {
        assert_eq!(verdict(0, &[]), Ok(()));
    }

    #[test]
    fn unsorted_segments_are_refused_at_the_later_one() {
        // Bite: no order check; reporting the earlier index.
        assert_eq!(
            verdict(10_000, &[ok(3000), ok(1000)]),
            Err((InvalidRule::Unsorted, 1))
        );
    }

    #[test]
    fn unsorted_reports_the_index_of_the_first_descent() {
        assert_eq!(
            verdict(10_000, &[ok(1000), ok(5000), ok(2000)]),
            Err((InvalidRule::Unsorted, 2))
        );
    }

    #[test]
    fn a_negative_start_is_out_of_range() {
        // Bite: no `0 <=` check.
        let s = seg(-1, 5, "mic", "me", "null", "x", r#""final""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::OutOfRange, 0)));
    }

    #[test]
    fn a_start_after_the_end_is_out_of_range() {
        // Bite: no `start <= end` check.
        let s = seg(500, 400, "mic", "me", "null", "x", r#""final""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::OutOfRange, 0)));
    }

    #[test]
    fn an_end_after_the_duration_is_out_of_range() {
        // Bite: no `end <= duration` check.
        let s = seg(500, 1001, "mic", "me", "null", "x", r#""final""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::OutOfRange, 0)));
    }

    #[test]
    fn an_unknown_speaker_is_refused() {
        // Bite: no speaker check; checking against a constant list that includes s2.
        let s = seg(0, 10, "mic", "s2", "null", "x", r#""final""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::UnknownSpeaker, 0)));
    }

    #[test]
    fn an_unknown_track_is_refused() {
        // Bite: no track check; accepting any non-empty string.
        let s = seg(0, 10, "other", "me", "null", "x", r#""final""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::UnknownTrack, 0)));
    }

    #[test]
    fn a_capitalised_track_is_refused() {
        let s = seg(0, 10, "Mic", "me", "null", "x", r#""final""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::UnknownTrack, 0)));
    }

    #[test]
    fn a_source_other_than_final_is_refused() {
        let s = seg(0, 10, "mic", "me", "null", "x", r#""draft""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::BadSource, 0)));
    }

    #[test]
    fn a_missing_source_is_refused_as_bad_source() {
        // The key absent, not null: parses (source is Option), validate refuses.
        let s = r#"{"start_ms":0,"end_ms":10,"track":"mic","speaker":"me","lang":null,"text":"x"}"#;
        assert_eq!(
            verdict(1000, &[s.to_string()]),
            Err((InvalidRule::BadSource, 0))
        );
    }

    #[test]
    fn a_null_source_is_refused_as_bad_source() {
        let s = seg(0, 10, "mic", "me", "null", "x", "null");
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::BadSource, 0)));
    }

    #[test]
    fn a_language_outside_the_code_table_is_refused() {
        // Bite: any two letters accepted; no lang check.
        let s = seg(0, 10, "mic", "me", r#""xx""#, "x", r#""final""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::BadLang, 0)));
    }

    #[test]
    fn an_upper_case_language_is_refused() {
        let s = seg(0, 10, "mic", "me", r#""EN""#, "x", r#""final""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::BadLang, 0)));
    }

    #[test]
    fn an_empty_language_string_is_refused() {
        let s = seg(0, 10, "mic", "me", r#""""#, "x", r#""final""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::BadLang, 0)));
    }

    #[test]
    fn an_empty_text_is_refused() {
        // Bite: no text check.
        let s = seg(0, 10, "mic", "me", "null", "", r#""final""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::EmptyText, 0)));
    }

    #[test]
    fn a_whitespace_only_text_is_refused() {
        // Bite: `is_empty()` without trim. (The JSON escapes decode to a space, newline, tab.)
        let s = seg(0, 10, "mic", "me", "null", "  \\n\\t ", r#""final""#);
        assert_eq!(verdict(1000, &[s]), Err((InvalidRule::EmptyText, 0)));
    }

    #[test]
    fn the_first_broken_segment_wins_and_a_later_rule_does_not_replace_it() {
        // Segment 1 has bad_lang, segment 3 has empty_text. Bite: last-failure-wins,
        // a rule-priority scan over all segments.
        let segs = [
            ok(0),
            seg(100, 200, "mic", "me", r#""zz""#, "x", r#""final""#),
            ok(200),
            seg(300, 400, "mic", "me", "null", "", r#""final""#),
        ];
        assert_eq!(verdict(10_000, &segs), Err((InvalidRule::BadLang, 1)));
    }

    #[test]
    fn the_first_broken_segment_wins_in_the_other_rule_order_too() {
        let segs = [
            ok(0),
            seg(100, 200, "mic", "me", "null", "", r#""final""#),
            ok(200),
            seg(300, 400, "mic", "me", r#""zz""#, "x", r#""final""#),
        ];
        assert_eq!(verdict(10_000, &segs), Err((InvalidRule::EmptyText, 1)));
    }

    #[test]
    fn the_segment_index_counts_from_zero_over_all_segments() {
        let segs = [
            ok(0),
            ok(100),
            ok(200),
            seg(300, 400, "mic", "s9", "null", "x", r#""final""#),
        ];
        assert_eq!(
            verdict(10_000, &segs),
            Err((InvalidRule::UnknownSpeaker, 3))
        );
    }

    // ---- merge: Acceptance 3 ------------------------------------------------------------

    #[test]
    fn merge_copies_the_header_and_fixes_schema_engine_and_source() {
        let t = merge(&mic_one(0, 0, 10_000, 10_000, vec![es(1.0, 2.0, "a")]));
        assert_eq!(t.schema, 1);
        assert_eq!(t.call_id, call_id());
        assert_eq!(t.title, "Weekly sync");
        assert_eq!(t.started_at, STARTED);
        assert_eq!(t.duration_ms, 10_000);
        assert_eq!(t.speakers, speakers());
        assert_eq!(t.engines.transcription.as_str(), "api:whisper-1");
        assert_eq!(t.engines.diarization, None);
        assert_eq!(t.segments.len(), 1);
        assert_eq!(
            t.segments.first().and_then(|s| s.source.as_deref()),
            Some("final")
        );
        assert_eq!(t.segments.first().and_then(|s| s.lang.as_deref()), None);
    }

    fn two_tracks(mic: Vec<EngineSegment>, system: Vec<EngineSegment>) -> MergeInput {
        input(
            60_000,
            vec![
                track(Track::System, 0, vec![timed(0, 60_000, system)]),
                track(Track::Mic, 0, vec![timed(0, 60_000, mic)]),
            ],
        )
    }

    #[test]
    fn overlapping_lines_of_both_tracks_are_kept_in_start_order() {
        // The system track is listed first in the input. Bite: concatenating per track,
        // dropping overlaps, sorting by end.
        let t = merge(&two_tracks(
            vec![es(0.0, 5.0, "m0"), es(10.0, 11.0, "m1")],
            vec![es(2.0, 3.0, "s0"), es(4.0, 12.0, "s1")],
        ));
        assert_eq!(
            rows(&t),
            vec![
                (0, 5000, "mic", "me", "m0"),
                (2000, 3000, "system", "s1", "s0"),
                (4000, 12_000, "system", "s1", "s1"),
                (10_000, 11_000, "mic", "me", "m1"),
            ]
        );
    }

    #[test]
    fn equal_starts_put_the_mic_line_before_the_system_line_whatever_the_input_order() {
        // Bite: stable sort by start only (system listed first would win).
        let t = merge(&two_tracks(
            vec![es(1.0, 2.0, "m")],
            vec![es(1.0, 2.0, "s")],
        ));
        assert_eq!(
            rows(&t),
            vec![
                (1000, 2000, "mic", "me", "m"),
                (1000, 2000, "system", "s1", "s")
            ]
        );
    }

    #[test]
    fn equal_starts_of_one_track_keep_the_input_order() {
        // Bite: an unstable sort, or a sort on text.
        let t = merge(&mic_one(
            0,
            0,
            10_000,
            10_000,
            vec![es(1.0, 2.0, "zeta"), es(1.0, 3.0, "alpha")],
        ));
        assert_eq!(
            rows(&t),
            vec![
                (1000, 2000, "mic", "me", "zeta"),
                (1000, 3000, "mic", "me", "alpha")
            ]
        );
    }

    #[test]
    fn a_line_is_offset_plus_chunk_start_plus_engine_time() {
        // 1000 + 60000 + 1200 / 2500. Bite: any term left out.
        let t = merge(&mic_one(
            1000,
            60_000,
            120_000,
            200_000,
            vec![es(1.2, 2.5, "a")],
        ));
        assert_eq!(rows(&t), vec![(62_200, 63_500, "mic", "me", "a")]);
    }

    #[test]
    fn the_second_chunk_of_a_track_starts_at_its_own_chunk_start() {
        let tr = track(
            Track::Mic,
            500,
            vec![
                timed(0, 30_000, vec![es(1.0, 2.0, "a")]),
                timed(30_000, 60_000, vec![es(1.0, 2.0, "b")]),
            ],
        );
        let t = merge(&input(100_000, vec![tr]));
        assert_eq!(
            rows(&t),
            vec![
                (1500, 2500, "mic", "me", "a"),
                (31_500, 32_500, "mic", "me", "b")
            ]
        );
    }

    #[test]
    fn engine_times_round_to_the_nearest_millisecond() {
        // 0.0006 s -> 1 ms (a floor gives 0), 2.0004 s -> 2000 ms (a ceil gives 2001).
        let t = merge(&mic_one(
            0,
            0,
            10_000,
            10_000,
            vec![es(0.0006, 2.0004, "a")],
        ));
        assert_eq!(rows(&t), vec![(1, 2000, "mic", "me", "a")]);
    }

    #[test]
    fn a_start_past_the_chunk_end_is_clamped_to_the_chunk_end_with_offset_0() {
        let t = merge(&mic_one(0, 0, 10_000, 60_000, vec![es(12.0, 15.0, "a")]));
        assert_eq!(rows(&t), vec![(10_000, 10_000, "mic", "me", "a")]);
    }

    #[test]
    fn an_end_past_the_chunk_end_is_clamped_to_the_chunk_end_with_offset_0() {
        // Bite: no clamp of the end alone.
        let t = merge(&mic_one(0, 0, 10_000, 60_000, vec![es(4.0, 15.0, "a")]));
        assert_eq!(rows(&t), vec![(4000, 10_000, "mic", "me", "a")]);
    }

    #[test]
    fn a_start_past_the_chunk_end_keeps_the_offset_when_clamped() {
        // Chunk [0, 120000] on a track that starts 5000 after started_at: the clamp is in
        // track time, then the offset is added. Bite: clamping offset+chunk+engine to the chunk.
        let t = merge(&mic_one(
            5000,
            0,
            120_000,
            200_000,
            vec![es(130.0, 140.0, "a")],
        ));
        assert_eq!(rows(&t), vec![(125_000, 125_000, "mic", "me", "a")]);
    }

    #[test]
    fn a_line_inside_the_chunk_keeps_the_offset_even_when_the_sum_passes_the_chunk_end() {
        // 5000 + 118 s = 123000 > 120000 (the chunk end in track time) and is NOT clamped to it;
        // the end 130 s is clamped to the chunk end, 5000 + 120000.
        let t = merge(&mic_one(
            5000,
            0,
            120_000,
            200_000,
            vec![es(118.0, 130.0, "a")],
        ));
        assert_eq!(rows(&t), vec![(123_000, 125_000, "mic", "me", "a")]);
    }

    #[test]
    fn a_negative_start_is_clamped_to_the_chunk_start_with_offset_0() {
        let t = merge(&mic_one(
            0,
            10_000,
            20_000,
            60_000,
            vec![es(-3.0, 2.0, "a")],
        ));
        assert_eq!(rows(&t), vec![(10_000, 12_000, "mic", "me", "a")]);
    }

    #[test]
    fn a_negative_start_is_clamped_to_the_chunk_start_plus_the_offset() {
        let t = merge(&mic_one(
            5000,
            10_000,
            20_000,
            60_000,
            vec![es(-3.0, 2.0, "a")],
        ));
        assert_eq!(rows(&t), vec![(15_000, 17_000, "mic", "me", "a")]);
    }

    #[test]
    fn a_nan_start_is_the_chunk_start() {
        let t = merge(&mic_one(
            0,
            10_000,
            20_000,
            60_000,
            vec![es(f64::NAN, 2.0, "a")],
        ));
        assert_eq!(rows(&t), vec![(10_000, 12_000, "mic", "me", "a")]);
    }

    #[test]
    fn a_nan_end_is_the_start() {
        let t = merge(&mic_one(0, 0, 20_000, 60_000, vec![es(3.0, f64::NAN, "a")]));
        assert_eq!(rows(&t), vec![(3000, 3000, "mic", "me", "a")]);
    }

    #[test]
    fn a_negative_end_is_the_start() {
        let t = merge(&mic_one(0, 0, 20_000, 60_000, vec![es(3.0, -1.0, "a")]));
        assert_eq!(rows(&t), vec![(3000, 3000, "mic", "me", "a")]);
    }

    #[test]
    fn a_reversed_pair_ends_at_the_start() {
        // Bite: swapping instead of end = start, or leaving end < start (validate would refuse).
        let t = merge(&mic_one(0, 0, 20_000, 60_000, vec![es(5.0, 2.0, "a")]));
        assert_eq!(rows(&t), vec![(5000, 5000, "mic", "me", "a")]);
    }

    #[test]
    fn an_empty_engine_segment_is_dropped() {
        let t = merge(&mic_one(
            0,
            0,
            10_000,
            10_000,
            vec![es(1.0, 2.0, "a"), es(2.0, 3.0, ""), es(3.0, 4.0, "b")],
        ));
        assert_eq!(
            rows(&t),
            vec![
                (1000, 2000, "mic", "me", "a"),
                (3000, 4000, "mic", "me", "b")
            ]
        );
    }

    #[test]
    fn a_whitespace_only_engine_segment_is_dropped() {
        // Bite: `is_empty()` without trim.
        let t = merge(&mic_one(
            0,
            0,
            10_000,
            10_000,
            vec![es(1.0, 2.0, " \n\t "), es(3.0, 4.0, "b")],
        ));
        assert_eq!(rows(&t), vec![(3000, 4000, "mic", "me", "b")]);
    }

    #[test]
    fn an_empty_time_less_chunk_is_dropped() {
        let t = merge(&input(
            10_000,
            vec![track(Track::Mic, 0, vec![timeless(0, 5000, "  ")])],
        ));
        assert!(t.segments.is_empty());
    }

    #[test]
    fn a_segment_language_overrides_the_chunk_language() {
        let mut c = timed(
            0,
            10_000,
            vec![EngineSegment {
                lang: Some(lang("ru")),
                ..es(1.0, 2.0, "a")
            }],
        );
        c.lang = Some(lang("en"));
        let t = merge(&input(10_000, vec![track(Track::Mic, 0, vec![c])]));
        assert_eq!(
            t.segments.first().and_then(|s| s.lang.as_deref()),
            Some("ru")
        );
    }

    #[test]
    fn a_segment_without_language_takes_the_chunk_language() {
        let mut c = timed(0, 10_000, vec![es(1.0, 2.0, "a")]);
        c.lang = Some(lang("en"));
        let t = merge(&input(10_000, vec![track(Track::Mic, 0, vec![c])]));
        assert_eq!(
            t.segments.first().and_then(|s| s.lang.as_deref()),
            Some("en")
        );
    }

    #[test]
    fn no_language_anywhere_is_null() {
        let t = merge(&mic_one(0, 0, 10_000, 10_000, vec![es(1.0, 2.0, "a")]));
        assert_eq!(t.segments.len(), 1);
        assert_eq!(t.segments.first().and_then(|s| s.lang.as_deref()), None);
    }

    #[test]
    fn a_time_less_chunk_is_one_line_spanning_the_chunk() {
        let t = merge(&input(
            60_000,
            vec![track(
                Track::System,
                0,
                vec![timeless(10_000, 40_000, "whole")],
            )],
        ));
        assert_eq!(rows(&t), vec![(10_000, 40_000, "system", "s1", "whole")]);
    }

    #[test]
    fn a_time_less_chunk_keeps_the_offset_at_both_ends() {
        let t = merge(&input(
            60_000,
            vec![track(
                Track::Mic,
                5000,
                vec![timeless(10_000, 40_000, "whole")],
            )],
        ));
        assert_eq!(rows(&t), vec![(15_000, 45_000, "mic", "me", "whole")]);
    }

    #[test]
    fn a_time_less_chunk_takes_the_chunk_language() {
        let mut c = timeless(0, 1000, "whole");
        c.lang = Some(lang("de"));
        let t = merge(&input(5000, vec![track(Track::Mic, 0, vec![c])]));
        assert_eq!(
            t.segments.first().and_then(|s| s.lang.as_deref()),
            Some("de")
        );
    }

    #[test]
    fn a_start_past_the_duration_is_clamped_to_it_with_the_text_unchanged() {
        // The longer track's tail (#196 (3)): the line is kept, start = end = duration.
        let t = merge(&input(
            50_000,
            vec![track(
                Track::System,
                0,
                vec![timed(0, 60_000, vec![es(55.0, 58.0, "tail")])],
            )],
        ));
        assert_eq!(rows(&t), vec![(50_000, 50_000, "system", "s1", "tail")]);
    }

    #[test]
    fn an_end_past_the_duration_is_clamped_to_it_and_the_start_is_kept() {
        let t = merge(&input(
            50_000,
            vec![track(
                Track::System,
                0,
                vec![timed(0, 60_000, vec![es(45.0, 55.0, "tail")])],
            )],
        ));
        assert_eq!(rows(&t), vec![(45_000, 50_000, "system", "s1", "tail")]);
    }

    #[test]
    fn the_duration_clamp_applies_after_the_offset() {
        // 3000 + 48 s = 51000 > 50000.
        let t = merge(&input(
            50_000,
            vec![track(
                Track::Mic,
                3000,
                vec![timed(0, 60_000, vec![es(48.0, 49.0, "tail")])],
            )],
        ));
        assert_eq!(rows(&t), vec![(50_000, 50_000, "mic", "me", "tail")]);
    }

    #[test]
    fn a_time_less_chunk_past_the_duration_is_clamped_and_kept() {
        let t = merge(&input(
            30_000,
            vec![track(
                Track::System,
                0,
                vec![timeless(20_000, 60_000, "tail")],
            )],
        ));
        assert_eq!(rows(&t), vec![(20_000, 30_000, "system", "s1", "tail")]);
    }

    #[test]
    fn a_line_inside_the_duration_is_not_touched_by_the_duration_clamp() {
        let t = merge(&mic_one(0, 0, 60_000, 50_000, vec![es(10.0, 50.0, "a")]));
        assert_eq!(rows(&t), vec![(10_000, 50_000, "mic", "me", "a")]);
    }

    #[test]
    fn every_merge_fixture_passes_validate() {
        // Total over any engine output: NaN, negative, reversed, out of chunk, blank, past duration.
        let wild = vec![
            es(f64::NAN, f64::NAN, "nan"),
            es(-5.0, -1.0, "neg"),
            es(9.0, 1.0, "reversed"),
            es(900.0, 901.0, "far"),
            es(1.0, 2.0, " "),
            es(f64::INFINITY, f64::INFINITY, "inf"),
            es(1.0, 2.0, "fine"),
        ];
        let fixtures = vec![
            two_tracks(vec![es(0.0, 5.0, "m0")], vec![es(2.0, 3.0, "s0")]),
            two_tracks(vec![es(1.0, 2.0, "m")], vec![es(1.0, 2.0, "s")]),
            mic_one(1000, 60_000, 120_000, 200_000, vec![es(1.2, 2.5, "a")]),
            mic_one(5000, 0, 120_000, 200_000, vec![es(130.0, 140.0, "a")]),
            mic_one(5000, 0, 120_000, 200_000, vec![es(118.0, 130.0, "a")]),
            mic_one(5000, 10_000, 20_000, 60_000, vec![es(-3.0, 2.0, "a")]),
            mic_one(0, 0, 20_000, 60_000, vec![es(3.0, f64::NAN, "a")]),
            mic_one(0, 0, 10_000, 10_000, wild.clone()),
            mic_one(7000, 0, 10_000, 3000, wild),
            input(
                50_000,
                vec![track(
                    Track::System,
                    0,
                    vec![timed(0, 60_000, vec![es(55.0, 58.0, "tail")])],
                )],
            ),
            input(
                50_000,
                vec![track(
                    Track::Mic,
                    3000,
                    vec![timed(0, 60_000, vec![es(48.0, 49.0, "tail")])],
                )],
            ),
            input(
                30_000,
                vec![track(
                    Track::System,
                    0,
                    vec![timeless(20_000, 60_000, "tail")],
                )],
            ),
            input(
                60_000,
                vec![track(
                    Track::Mic,
                    5000,
                    vec![timeless(10_000, 40_000, "whole")],
                )],
            ),
            input(
                0,
                vec![track(
                    Track::Mic,
                    0,
                    vec![timeless(0, 1000, "zero duration")],
                )],
            ),
        ];
        for (n, fixture) in fixtures.iter().enumerate() {
            let t = merge(fixture);
            assert_eq!(t.validate(), Ok(()), "fixture {n}: {:?}", rows(&t));
        }
    }

    // ---- Markdown: Acceptance 4 ---------------------------------------------------------

    fn md_doc(segments: &[String], duration: i64, started_at: &str) -> TranscriptV1 {
        let text = doc(duration, segments).replacen(STARTED, started_at, 1);
        // The transcript's own copies of title and names differ from the meta.json ones below.
        let text = text.replacen(r#""title":"t""#, r#""title":"Stale title""#, 1);
        let text = text.replacen(r#""name":"Me""#, r#""name":"Stale me""#, 1);
        load(&text.replacen(r#""name":"Other party""#, r#""name":"Stale other""#, 1))
    }

    fn meta_names() -> Vec<Speaker> {
        vec![
            Speaker {
                id: "me".into(),
                name: "Alice".into(),
            },
            Speaker {
                id: "s1".into(),
                name: "Bob".into(),
            },
        ]
    }

    #[test]
    fn the_markdown_is_exactly_the_contract_layout_with_names_and_title_from_meta() {
        // Contract example: title line, `YYYY-MM-DD HH:MM · H:MM:SS` (the wall-clock part of
        // started_at, no conversion from its +03:00 offset), a blank line, one line per segment.
        // Bite: title or names from the transcript, UTC conversion, round instead of floor of
        // the second (5999 ms -> 00:00:05), padded hours, a missing blank line or final newline.
        let segs = [
            seg(3000, 3500, "mic", "me", "null", "text", r#""final""#),
            seg(5999, 6500, "system", "s1", "null", "reply", r#""final""#),
        ];
        let t = md_doc(&segs, 3_725_000, STARTED);
        let want = "# Weekly\n2026-10-30 14:05 · 1:02:05\n\n**[00:00:03] Alice:** text\n**[00:00:05] Bob:** reply\n";
        assert_eq!(render_markdown(&t, "Weekly", &meta_names()), want);
    }

    #[test]
    fn the_markdown_pads_the_clock_to_two_digits_and_the_duration_hours_not() {
        // 3_661_000 ms -> 01:01:01; a Z offset is not converted either; duration 0:00:42.
        let segs = [seg(
            3_661_000,
            3_662_000,
            "system",
            "s1",
            "null",
            "late",
            r#""final""#,
        )];
        let t = md_doc(&segs, 3_700_000, "2026-01-02T03:04:05Z");
        assert_eq!(
            render_markdown(&t, "Weekly", &meta_names()),
            "# Weekly\n2026-01-02 03:04 · 1:01:40\n\n**[01:01:01] Bob:** late\n"
        );
        let short = md_doc(&[], 42_000, "2026-01-02T03:04:05Z");
        assert!(render_markdown(&short, "Weekly", &meta_names()).contains(" · 0:00:42\n"));
    }

    // ---- developer choices (the record leaves them open) --------------------------------

    #[test]
    fn an_infinite_engine_time_counts_as_zero() {
        let t = merge(&mic_one(
            0,
            10_000,
            20_000,
            60_000,
            vec![es(f64::INFINITY, f64::INFINITY, "a")],
        ));
        assert_eq!(rows(&t), vec![(10_000, 10_000, "mic", "me", "a")]);
    }

    #[test]
    fn a_reversed_chunk_does_not_panic_and_passes_validate() {
        // Bite: `u64::clamp(start, end)` panics when the chunk's end is before its start.
        let tr = track(
            Track::Mic,
            0,
            vec![
                timed(5000, 1000, vec![es(1.0, 2.0, "timed")]),
                timeless(5000, 1000, "whole"),
            ],
        );
        let t = merge(&input(60_000, vec![tr]));
        assert_eq!(
            rows(&t),
            vec![
                (5000, 5000, "mic", "me", "timed"),
                (5000, 5000, "mic", "me", "whole")
            ]
        );
        assert_eq!(t.validate(), Ok(()));
    }

    #[test]
    fn the_markdown_folds_whitespace_and_the_json_keeps_the_original() {
        // Bite: a line break in the engine text starting a second Markdown line.
        let segs = [seg(
            1000,
            2000,
            "mic",
            "me",
            "null",
            " one\\n two\\tthree ",
            r#""final""#,
        )];
        let t = md_doc(&segs, 60_000, STARTED);
        assert_eq!(
            t.segments.first().map(|s| s.text.as_str()),
            Some(" one\n two\tthree ")
        );
        assert!(t.to_json().contains(r#""text":" one\n two\tthree ""#));
        assert_eq!(
            render_markdown(&t, "Two\nlines", &meta_names()),
            "# Two lines\n2026-10-30 14:05 · 0:01:00\n\n**[00:00:01] Alice:** one two three\n"
        );
    }

    #[test]
    fn a_transcript_without_segments_ends_after_the_blank_line() {
        let t = md_doc(&[], 42_000, STARTED);
        assert_eq!(
            render_markdown(&t, "Weekly", &meta_names()),
            "# Weekly\n2026-10-30 14:05 · 0:00:42\n\n"
        );
    }

    #[test]
    fn a_speaker_missing_from_the_names_is_shown_by_id_not_by_the_stale_copy() {
        let segs = [seg(0, 10, "system", "s1", "null", "x", r#""final""#)];
        let t = md_doc(&segs, 1000, STARTED);
        assert!(render_markdown(&t, "T", &[]).contains("**[00:00:00] s1:** x\n"));
    }

    #[test]
    fn a_started_at_that_is_not_iso_does_not_panic_the_renderer() {
        // Bite: slicing with `[..10]`, or `get` on a multi-byte boundary unwrapped.
        for odd in ["short", "2026-10-3\u{e9}T14:05:09Z"] {
            let t = md_doc(&[], 1000, odd);
            assert!(render_markdown(&t, "T", &[]).starts_with("# T\n"));
        }
    }

    // ---- failure-branch bite: survivors of the catalog ------------------------------------

    #[test]
    fn a_huge_finite_engine_time_is_clamped_to_the_chunk() {
        // Bite: `chunk_start + engine_ms(..)`, which overflows once `as u64` has saturated.
        let t = merge(&mic_one(
            5000,
            10_000,
            20_000,
            60_000,
            vec![es(1e300, 1e300, "a")],
        ));
        assert_eq!(rows(&t), vec![(25_000, 25_000, "mic", "me", "a")]);
    }

    #[test]
    fn a_huge_track_offset_saturates_and_is_clamped_to_the_duration() {
        // Bite: `ms + start_offset_ms` overflows.
        let t = merge(&mic_one(
            u64::MAX,
            1000,
            2000,
            60_000,
            vec![es(0.0, 1.0, "a")],
        ));
        assert_eq!(rows(&t), vec![(60_000, 60_000, "mic", "me", "a")]);
        assert_eq!(t.validate(), Ok(()));
    }

    #[test]
    fn a_time_past_the_wire_range_stays_positive_and_passes_validate() {
        // Bite: `ms as i64` wraps u64::MAX to -1, which validate refuses.
        let t = merge(&mic_one(
            0,
            0,
            u64::MAX,
            u64::MAX,
            vec![es(1e300, 1e300, "a")],
        ));
        assert_eq!(t.duration_ms, i64::MAX);
        assert_eq!(rows(&t), vec![(i64::MAX, i64::MAX, "mic", "me", "a")]);
        assert_eq!(t.validate(), Ok(()));
    }

    #[test]
    fn a_merged_line_keeps_the_engine_text_unchanged() {
        // Bite: trimming or folding in merge; only the renderer folds, the JSON keeps the original.
        let t = merge(&mic_one(
            0,
            0,
            10_000,
            10_000,
            vec![es(1.0, 2.0, " one\n two\tthree ")],
        ));
        assert_eq!(
            rows(&t),
            vec![(1000, 2000, "mic", "me", " one\n two\tthree ")]
        );
    }

    #[test]
    fn a_line_break_in_a_speaker_name_does_not_start_a_second_markdown_line() {
        // Bite: writing the meta.json name without folding it.
        let segs = [seg(0, 10, "mic", "me", "null", "x", r#""final""#)];
        let t = md_doc(&segs, 1000, STARTED);
        let names = [Speaker {
            id: "me".into(),
            name: " Al\nice ".into(),
        }];
        assert_eq!(
            render_markdown(&t, "T", &names),
            "# T\n2026-10-30 14:05 · 0:00:01\n\n**[00:00:00] Al ice:** x\n"
        );
    }
}
