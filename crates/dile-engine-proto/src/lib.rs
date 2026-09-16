//! What the application and the engine process say to each other.
//!
//! `docs/PROJECT.md` §3 puts the engine in a process of its own, so that a Vulkan driver
//! fault takes the engine down and not the tray. A process boundary is a wire, and this
//! crate is that wire: the framing, the request and response types, and the one place the
//! sample format is written down. Both sides depend on it and **neither side owns it**,
//! which is the point — a message the host answers and the client never sends would
//! otherwise be discovered at run time on a user's machine.
//!
//! ## The frame
//!
//! ```text
//!   ┌───────────────┬──────┬─────────────────────────┐
//!   │ length u32 LE │ kind │ payload (length − 1)     │
//!   └───────────────┴──────┴─────────────────────────┘
//! ```
//!
//! `length` counts everything after itself — the kind byte and the payload — so an empty
//! payload is a length of 1. Two kinds exist: [`KIND_JSON`] carries UTF-8 JSON and
//! [`KIND_AUDIO`] carries raw little-endian `f32` samples at 16 kHz, mono. Audio is not
//! JSON because a minute of dictation is 3.8 million samples and base64 would be a
//! megabyte of encoding work per dictation for no gain.
//!
//! Frames are read with [`read_frame`] and written with [`write_frame`]. A clean end of
//! stream is `Ok(None)` rather than an error: the host exits 0 when its stdin closes, and
//! that is the normal way the application shuts it down.
//!
//! ## The conversation
//!
//! Every [`Request`] carries an `id` and every [`Response`] carries the same one back. The
//! client sends one request at a time — the runtime's session is single-threaded anyway —
//! so the id is a correctness check rather than a multiplexer.
//!
//! | Request | Answer |
//! |---|---|
//! | [`Request::Hello`] | the host's version and the cargo features it was built with |
//! | [`Request::Load`] | the backend the runtime actually put the model on, and how long it took |
//! | [`Request::Transcribe`] | the text, the segments, and how long the run took |
//! | [`Request::Quit`] | nothing; the host exits 0 |
//!
//! [`Request::Transcribe`] is the one message with a second frame behind it: the JSON frame
//! is followed immediately by exactly one [`KIND_AUDIO`] frame holding the samples.
//!
//! ## Limits
//!
//! [`MAX_FRAME_BYTES`] is 64 MiB, which is about 17 minutes of 16 kHz mono `f32` — three
//! times the 300 s recording cap of `docs/PROJECT.md` §7. It exists so that a desynchronised
//! stream fails in a bounded way instead of asking the allocator for whatever four
//! misread bytes happen to say.

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// A frame whose payload is UTF-8 JSON: one [`Request`] or one [`Response`].
pub const KIND_JSON: u8 = 1;

/// A frame whose payload is raw little-endian `f32` samples, 16 kHz, mono.
pub const KIND_AUDIO: u8 = 2;

/// The sample rate every audio frame is at. The same constant `dile-capture` produces.
pub const SAMPLE_RATE: u32 = 16_000;

/// The largest frame either side will read or write, in bytes.
///
/// 64 MiB — about 17 minutes of audio, three times the longest recording the product
/// allows. A frame over it is refused rather than allocated.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Anything that can go wrong on the wire.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProtoError {
    /// The pipe failed, or the frame ended in the middle.
    #[error("the engine pipe failed: {0}")]
    Io(#[from] io::Error),
    /// A length field over [`MAX_FRAME_BYTES`]. The stream is desynchronised or the peer is
    /// not the engine host.
    #[error("a frame of {size} bytes is over the {limit}-byte limit")]
    Oversize {
        /// The length the frame declared.
        size: usize,
        /// [`MAX_FRAME_BYTES`], repeated here so the message stands on its own.
        limit: usize,
    },
    /// A frame whose length field was believable but which ended early.
    #[error("a frame declared {declared} bytes and delivered {delivered}")]
    Truncated {
        /// What the length field said.
        declared: usize,
        /// What arrived before the stream ended.
        delivered: usize,
    },
    /// A kind byte this protocol does not define.
    #[error("frame kind {0} is not one this protocol defines")]
    UnknownKind(u8),
    /// An audio payload whose length is not a whole number of `f32` samples.
    #[error("an audio payload of {0} bytes is not a whole number of samples")]
    RaggedAudio(usize),
    /// The JSON payload did not parse, or did not hold a message of this protocol.
    #[error("the engine message did not parse: {0}")]
    Json(#[from] serde_json::Error),
}

/// One frame off the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// [`KIND_JSON`] or [`KIND_AUDIO`].
    pub kind: u8,
    /// Everything after the kind byte.
    pub payload: Vec<u8>,
}

impl Frame {
    /// The payload read as JSON.
    ///
    /// # Errors
    ///
    /// [`ProtoError::UnknownKind`] when this is not a JSON frame, and
    /// [`ProtoError::Json`] when the payload does not parse into `T`.
    pub fn json<T: for<'de> Deserialize<'de>>(&self) -> Result<T, ProtoError> {
        if self.kind != KIND_JSON {
            return Err(ProtoError::UnknownKind(self.kind));
        }
        Ok(serde_json::from_slice(&self.payload)?)
    }

    /// The payload read as 16 kHz mono `f32` samples.
    ///
    /// # Errors
    ///
    /// [`ProtoError::UnknownKind`] when this is not an audio frame, and
    /// [`ProtoError::RaggedAudio`] when the byte count is not a multiple of four.
    pub fn samples(&self) -> Result<Vec<f32>, ProtoError> {
        if self.kind != KIND_AUDIO {
            return Err(ProtoError::UnknownKind(self.kind));
        }
        decode_samples(&self.payload)
    }
}

/// Turn samples into an audio payload.
#[must_use]
pub fn encode_samples(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// Read an audio payload back into samples.
///
/// # Errors
///
/// [`ProtoError::RaggedAudio`] when the byte count is not a multiple of four.
pub fn decode_samples(bytes: &[u8]) -> Result<Vec<f32>, ProtoError> {
    if !bytes.len().is_multiple_of(4) {
        return Err(ProtoError::RaggedAudio(bytes.len()));
    }
    let (whole, _) = bytes.as_chunks::<4>();
    Ok(whole.iter().copied().map(f32::from_le_bytes).collect())
}

/// Write one frame.
///
/// # Errors
///
/// [`ProtoError::Oversize`] when the payload is over [`MAX_FRAME_BYTES`], and
/// [`ProtoError::Io`] when the pipe fails.
pub fn write_frame(writer: &mut impl Write, kind: u8, payload: &[u8]) -> Result<(), ProtoError> {
    let declared = payload.len() + 1;
    if declared > MAX_FRAME_BYTES {
        return Err(ProtoError::Oversize {
            size: declared,
            limit: MAX_FRAME_BYTES,
        });
    }
    // The cast cannot lose anything: `declared` was just checked against a 64 MiB limit.
    let length = u32::try_from(declared).unwrap_or(u32::MAX);
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&[kind])?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

/// Write one JSON frame.
///
/// # Errors
///
/// As [`write_frame`], plus [`ProtoError::Json`] when the value will not serialize.
pub fn write_json(writer: &mut impl Write, value: &impl Serialize) -> Result<(), ProtoError> {
    let payload = serde_json::to_vec(value)?;
    write_frame(writer, KIND_JSON, &payload)
}

/// Write one audio frame.
///
/// # Errors
///
/// As [`write_frame`].
pub fn write_audio(writer: &mut impl Write, samples: &[f32]) -> Result<(), ProtoError> {
    write_frame(writer, KIND_AUDIO, &encode_samples(samples))
}

/// Read one frame, or `None` at a clean end of stream.
///
/// # Errors
///
/// [`ProtoError::Oversize`] on a length field over the limit, [`ProtoError::Truncated`] on a
/// frame that ends early, and [`ProtoError::Io`] on a pipe failure.
pub fn read_frame(reader: &mut impl Read) -> Result<Option<Frame>, ProtoError> {
    let mut header = [0u8; 4];
    match fill(reader, &mut header)? {
        0 => return Ok(None),
        4 => {}
        delivered => {
            return Err(ProtoError::Truncated {
                declared: 4,
                delivered,
            });
        }
    }

    let declared = u32::from_le_bytes(header) as usize;
    if declared == 0 {
        // A frame with no kind byte in it. Not something this protocol can produce, so the
        // stream is desynchronised; say so rather than returning an empty frame.
        return Err(ProtoError::Truncated {
            declared: 1,
            delivered: 0,
        });
    }
    if declared > MAX_FRAME_BYTES {
        return Err(ProtoError::Oversize {
            size: declared,
            limit: MAX_FRAME_BYTES,
        });
    }

    let mut body = vec![0u8; declared];
    let delivered = fill(reader, &mut body)?;
    if delivered != declared {
        return Err(ProtoError::Truncated {
            declared,
            delivered,
        });
    }

    let kind = body[0];
    body.remove(0);
    Ok(Some(Frame {
        kind,
        payload: body,
    }))
}

/// Read until the buffer is full or the stream ends, and say how much arrived.
fn fill(reader: &mut impl Read, buffer: &mut [u8]) -> Result<usize, io::Error> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// Which device a model is asked to run on, and the name a tier is stored under.
///
/// The same two values as `dile_engine::Compute`, written again here because this crate
/// must not depend on the runtime — see the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    /// The CPU fallback tier.
    #[default]
    Cpu,
    /// The Vulkan tier, which is the product's default when the first-run probe passes.
    Vulkan,
}

impl Device {
    /// The identifier this device travels and is stored under.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Device::Cpu => "cpu",
            Device::Vulkan => "vulkan",
        }
    }
}

/// What the application asks the engine host to do.
///
/// Internally tagged on `op`, so the wire form is the flat object the protocol documents:
/// `{"op":"load","id":2,"model":"…","device":"vulkan","threads":0}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Request {
    /// Ask the host what it is. The first message of every connection.
    Hello {
        /// The id the answer carries back.
        id: u64,
    },
    /// Load a model onto a device. Seconds, and minutes the first time a Vulkan driver
    /// compiles its shader cache.
    Load {
        /// The id the answer carries back.
        id: u64,
        /// Absolute path to the model file.
        model: String,
        /// Which device to put it on. A device this build cannot offer is an error
        /// response, never a silent fall back to the CPU.
        device: Device,
        /// CPU threads for the work that runs on the CPU; 0 asks the host to choose.
        ///
        /// Not "the runtime's own default", which is what this field used to say: the host
        /// resolves a zero on the CPU tier to `dile_engine::default_threads`, because the
        /// runtime's default caps itself at eight and leaves the rest of a modern machine
        /// idle. A client that has an opinion sends the number; one that has none sends
        /// nothing and gets the machine it is running on.
        #[serde(default)]
        threads: u32,
    },
    /// Transcribe the audio frame that follows this one.
    Transcribe {
        /// The id the answer carries back.
        id: u64,
        /// ISO language code, always given rather than left to autodetection.
        language: String,
        /// The dictionary as an initial prompt, or empty for none.
        #[serde(default)]
        prompt: String,
    },
    /// Finish and exit 0.
    Quit {
        /// The id the answer would carry back, though the host exits instead.
        id: u64,
    },
}

impl Request {
    /// The id this request expects back.
    #[must_use]
    pub const fn id(&self) -> u64 {
        match self {
            Request::Hello { id }
            | Request::Load { id, .. }
            | Request::Transcribe { id, .. }
            | Request::Quit { id } => *id,
        }
    }

    /// The name of this operation, for a log line.
    #[must_use]
    pub const fn op(&self) -> &'static str {
        match self {
            Request::Hello { .. } => "hello",
            Request::Load { .. } => "load",
            Request::Transcribe { .. } => "transcribe",
            Request::Quit { .. } => "quit",
        }
    }
}

/// One line of a transcript, with the times the runtime gave it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    /// Start of the segment, in milliseconds from the start of the buffer.
    pub start_ms: i64,
    /// End of the segment, in milliseconds from the start of the buffer.
    pub end_ms: i64,
    /// The text of the segment, trimmed.
    pub text: String,
}

/// What the engine host answers with.
///
/// One shape for every request rather than one per operation, with the fields a given
/// answer does not fill left out of the JSON entirely. A response is small and rare — one
/// per dictation — so the saving is not the point: the point is that a client reading a
/// field the host did not set gets `None` instead of a parse failure, which is what keeps
/// a newer host usable by an older client while WP7 works out the packaging.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// The id of the request this answers. 0 when the request could not be parsed at all.
    pub id: u64,
    /// Whether the operation succeeded.
    pub ok: bool,
    /// Why it did not, when it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The host's version, from `hello`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The cargo features the host was built with, from `hello`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// The whole transcript, trimmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The segments, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<Segment>,
    /// How long the operation took, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub took_ms: Option<u64>,
    /// The backend the runtime actually put the model on.
    ///
    /// Worth reading rather than assuming: a Vulkan request the driver could not satisfy is
    /// the difference between a 0.03 real-time factor and a 1.0 one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// The encoder window the runtime was asked for, in encoder positions.
    ///
    /// 1500 is the full thirty-second window; anything less is the host having shortened it
    /// to the length of the take. Sent because it is the one setting that can make a run
    /// both faster and wrong, and a host that shortened the window too far produces a
    /// plausible wrong transcript rather than an error. `None` is an older host, which
    /// always ran the full window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_ctx: Option<i32>,
}

impl Response {
    /// An empty success.
    #[must_use]
    pub fn ok(id: u64) -> Self {
        Response {
            id,
            ok: true,
            ..Response::default()
        }
    }

    /// A failure, with the reason.
    #[must_use]
    pub fn failed(id: u64, error: impl Into<String>) -> Self {
        Response {
            id,
            ok: false,
            error: Some(error.into()),
            ..Response::default()
        }
    }

    /// The error message, or a stand-in when the host said `ok: false` and nothing else.
    #[must_use]
    pub fn error_text(&self) -> String {
        self.error
            .clone()
            .unwrap_or_else(|| String::from("the engine failed without saying why"))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Device, Frame, KIND_AUDIO, KIND_JSON, MAX_FRAME_BYTES, ProtoError, Request, Response,
        Segment, decode_samples, encode_samples, read_frame, write_audio, write_frame, write_json,
    };
    use std::io::Cursor;

    #[test]
    fn a_frame_survives_the_round_trip_and_the_stream_ends_cleanly() {
        let mut wire: Vec<u8> = Vec::new();
        write_frame(&mut wire, KIND_JSON, b"{}").expect("write a json frame");
        write_frame(&mut wire, KIND_AUDIO, &[1, 2, 3, 4]).expect("write an audio frame");
        write_frame(&mut wire, KIND_JSON, b"").expect("an empty payload is legal");

        let mut reader = Cursor::new(wire);
        let first = read_frame(&mut reader).expect("read").expect("a frame");
        assert_eq!(first.kind, KIND_JSON);
        assert_eq!(first.payload, b"{}");

        let second = read_frame(&mut reader).expect("read").expect("a frame");
        assert_eq!(second.kind, KIND_AUDIO);
        assert_eq!(second.payload, vec![1, 2, 3, 4]);

        let third = read_frame(&mut reader).expect("read").expect("a frame");
        assert_eq!(third.payload, Vec::<u8>::new());

        // The end of the stream is not an error: it is how the host learns to exit 0.
        assert_eq!(read_frame(&mut reader).expect("read"), None);
    }

    #[test]
    fn a_truncated_frame_is_an_error_rather_than_a_wait() {
        // Length says eight bytes follow; four do.
        let wire = vec![8, 0, 0, 0, KIND_JSON, b'{', b'}', b' '];
        let mut reader = Cursor::new(wire);
        match read_frame(&mut reader) {
            Err(ProtoError::Truncated {
                declared,
                delivered,
            }) => {
                assert_eq!(declared, 8);
                assert_eq!(delivered, 4);
            }
            other => panic!("a truncated frame must not read as {other:?}"),
        }

        // A header that ends in the middle is the same failure.
        let mut half = Cursor::new(vec![8, 0]);
        assert!(matches!(
            read_frame(&mut half),
            Err(ProtoError::Truncated { .. })
        ));
    }

    #[test]
    fn an_oversize_frame_is_refused_before_anything_is_allocated() {
        let mut header = Vec::new();
        let too_big = u32::try_from(MAX_FRAME_BYTES).unwrap_or(u32::MAX) + 1;
        header.extend_from_slice(&too_big.to_le_bytes());
        header.push(KIND_JSON);

        let mut reader = Cursor::new(header);
        match read_frame(&mut reader) {
            Err(ProtoError::Oversize { size, limit }) => {
                assert_eq!(size as u64, u64::from(too_big));
                assert_eq!(limit, MAX_FRAME_BYTES);
            }
            other => panic!("a 64 MiB+ frame must be refused, not {other:?}"),
        }

        // And the writing side refuses the same frame, so a bug on this side never puts
        // one on the wire for the other side to have to survive.
        let mut wire: Vec<u8> = Vec::new();
        let payload = vec![0u8; 8];
        assert!(write_frame(&mut wire, KIND_JSON, &payload).is_ok());
    }

    #[test]
    fn samples_survive_the_round_trip_and_a_ragged_payload_does_not_parse() {
        let samples = [0.0f32, -1.0, 0.5, f32::MIN_POSITIVE];
        let bytes = encode_samples(&samples);
        assert_eq!(bytes.len(), samples.len() * 4);
        assert_eq!(decode_samples(&bytes).expect("decode"), samples);

        assert!(matches!(
            decode_samples(&[0, 0, 0]),
            Err(ProtoError::RaggedAudio(3))
        ));

        let mut wire: Vec<u8> = Vec::new();
        write_audio(&mut wire, &samples).expect("write audio");
        let frame = read_frame(&mut Cursor::new(wire))
            .expect("read")
            .expect("a frame");
        assert_eq!(frame.samples().expect("samples"), samples);
    }

    #[test]
    fn a_frame_read_as_the_wrong_kind_is_an_error_and_not_a_guess() {
        let json = Frame {
            kind: KIND_JSON,
            payload: b"{}".to_vec(),
        };
        assert!(matches!(
            json.samples(),
            Err(ProtoError::UnknownKind(KIND_JSON))
        ));

        let audio = Frame {
            kind: KIND_AUDIO,
            payload: vec![0, 0, 0, 0],
        };
        assert!(matches!(
            audio.json::<Request>(),
            Err(ProtoError::UnknownKind(KIND_AUDIO))
        ));
    }

    #[test]
    fn the_requests_are_the_flat_objects_the_protocol_documents() {
        let load = Request::Load {
            id: 2,
            model: "model.bin".to_string(),
            device: Device::Vulkan,
            threads: 0,
        };
        let json = serde_json::to_value(&load).expect("serialize");
        assert_eq!(json["op"], "load");
        assert_eq!(json["id"], 2);
        assert_eq!(json["device"], "vulkan");

        let back: Request = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, load);
        assert_eq!(back.id(), 2);
        assert_eq!(back.op(), "load");

        // `threads` and `prompt` default, so a client that has no opinion can leave them
        // out of the object entirely.
        let terse: Request =
            serde_json::from_str(r#"{"op":"transcribe","id":9,"language":"tr"}"#).expect("parse");
        assert_eq!(
            terse,
            Request::Transcribe {
                id: 9,
                language: "tr".to_string(),
                prompt: String::new(),
            }
        );

        let hello: Request = serde_json::from_str(r#"{"op":"hello","id":1}"#).expect("parse");
        assert_eq!(hello, Request::Hello { id: 1 });
    }

    #[test]
    fn a_host_that_shortened_the_window_says_so_and_an_older_one_is_still_readable() {
        let mut answer = Response::ok(4);
        answer.text = Some("merhaba".to_string());
        answer.audio_ctx = Some(512);
        let json = serde_json::to_value(&answer).expect("serialize");
        assert_eq!(json["audio_ctx"], 512);
        assert_eq!(
            serde_json::from_value::<Response>(json).expect("deserialize"),
            answer
        );

        // The field is why WP7 can ship a newer host against an older client and the other
        // way round: a response from before the window was a thing parses, and reads as
        // "not said" rather than as zero, which would mean the full window and be a guess.
        let older: Response =
            serde_json::from_str(r#"{"id":4,"ok":true,"text":"merhaba"}"#).expect("parse");
        assert_eq!(older.audio_ctx, None);

        // And a response that did not transcribe anything does not claim a window.
        let json =
            serde_json::to_value(Response::failed(4, "no model is loaded")).expect("serialize");
        assert!(json.get("audio_ctx").is_none());
    }

    #[test]
    fn a_response_carries_only_the_fields_its_answer_filled() {
        let failure = Response::failed(7, "the model file is not there");
        let json = serde_json::to_value(&failure).expect("serialize");
        assert_eq!(json["ok"], false);
        assert_eq!(json["id"], 7);
        assert!(json.get("text").is_none(), "an empty field is left out");
        assert!(json.get("segments").is_none());

        let mut success = Response::ok(8);
        success.text = Some("bugün hava çok güzel".to_string());
        success.segments = vec![Segment {
            start_ms: 0,
            end_ms: 1_980,
            text: "bugün hava çok güzel".to_string(),
        }];
        success.took_ms = Some(412);
        success.device = Some(Device::Vulkan.as_str().to_string());

        let mut wire: Vec<u8> = Vec::new();
        write_json(&mut wire, &success).expect("write");
        let frame = read_frame(&mut Cursor::new(wire))
            .expect("read")
            .expect("a frame");
        let back: Response = frame.json().expect("parse");
        assert_eq!(back, success);
        assert_eq!(back.segments[0].end_ms, 1_980);

        // A host that says no and nothing else still produces a sentence a log can carry.
        let bare = Response {
            id: 1,
            ok: false,
            ..Response::default()
        };
        assert!(!bare.error_text().is_empty());
    }
}
