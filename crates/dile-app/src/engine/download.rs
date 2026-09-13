//! Fetching a model, once, and being able to prove it arrived whole.
//!
//! `docs/PROJECT.md` §3, Distribution: Hugging Face only, pinned by commit sha, sha256
//! embedded in the app, `.part` then rename, consent before any download, manual-download
//! instructions written from day one. Handy's issue #1579 — forty-seven comments — happened
//! purely because that last line was never written, and the rest of this list is what keeps
//! a half-gigabyte file from becoming a permanently broken installation.
//!
//! **Three rules, and each of them is one failure that has happened to somebody:**
//!
//! 1. **Nothing is written to the final name until the hash matches.** A truncated model is
//!    a model the runtime loads and then fails on in a way that reads like a broken GPU.
//!    The download lands in `<file>.part`, the whole of it is hashed, and only then is it
//!    renamed. A mismatch deletes the `.part` — resuming a file that is already wrong would
//!    never terminate.
//! 2. **An interrupted download resumes.** A `.part` of *n* bytes asks for `Range: bytes=n-`
//!    and appends. A server that answers 200 instead of 206 does not support ranges, so the
//!    `.part` is truncated and the transfer starts over rather than producing a file that is
//!    the first *n* bytes twice.
//! 3. **Progress is reported, but not thirty times a second.** [`MIN_PROGRESS_GAP`] throttles
//!    it to four a second, because the tooltip behind it is a Windows shell call.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// The smallest gap between two progress reports.
///
/// Four a second. The number is not about the cost of the callback but about what is on the
/// other end of it: a tray tooltip, which is a shell call, and eventually a panel that has
/// to repaint. Nobody reads a percentage faster than this.
const MIN_PROGRESS_GAP: Duration = Duration::from_millis(250);

/// How much of the file is read at a time, and therefore how often the transfer can notice
/// that it has been asked to stop.
const CHUNK: usize = 64 * 1024;

/// Anything that can go wrong fetching a model.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DownloadError {
    /// The request failed, or the connection broke mid-transfer.
    #[error("the download failed: {0}")]
    Transport(String),
    /// The server answered with something other than 200 or 206.
    #[error("the model host answered {0}")]
    Status(u16),
    /// The application asked for the transfer to stop, and it did.
    #[error("the download was stopped")]
    Stopped,
    /// Reading, writing or renaming failed.
    #[error("the model file could not be written: {0}")]
    Io(#[from] io::Error),
    /// The bytes arrived and are not the model.
    ///
    /// The most important error in this module: the `.part` is deleted with it, so the next
    /// attempt starts over instead of resuming a file that can never become right.
    #[error("the download hashed to {found}, not {expected}; it was discarded")]
    Checksum {
        /// What the file hashed to.
        found: String,
        /// What the model table says it must hash to.
        expected: String,
    },
}

/// What a finished download did, for the log line that follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fetched {
    /// Bytes that came over the network in this call. Zero when the file was already there.
    pub transferred: u64,
    /// Bytes that were already in the `.part` when this call started.
    pub resumed_from: u64,
}

/// Fetch `url` into `destination`, resuming a `.part` beside it if there is one.
///
/// `expected_size` is only used to report progress — the file is judged by its hash, never
/// by its length. `progress` is called with `(downloaded, total)` at most four times a
/// second, and `keep_going` is asked between chunks so that a shutdown does not have to wait
/// for half a gigabyte.
///
/// # Errors
///
/// Every variant of [`DownloadError`]. A checksum failure also removes the partial file.
pub fn fetch(
    url: &str,
    expected_sha256: &str,
    expected_size: u64,
    destination: &Path,
    progress: &mut impl FnMut(u64, u64),
    keep_going: &dyn Fn() -> bool,
) -> Result<Fetched, DownloadError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let partial = part_path(destination);

    let have = fs::metadata(&partial).map(|meta| meta.len()).unwrap_or(0);

    // A `.part` that is already the whole file. It happens: the transfer finishes, and the
    // application is closed in the second between the last byte and the rename. Asking for
    // `bytes=<size>-` would get a 416 and turn a finished download into a broken one, so the
    // request is never made — the bytes are on disk and the only question left is the hash.
    // Observed on this machine, 2026-09-13, on a 574 MB model.
    if expected_size > 0 && have >= expected_size {
        log::info!("the partial download is already the whole file; checking it");
        settle(&partial, destination, expected_sha256)?;
        return Ok(Fetched {
            transferred: 0,
            resumed_from: have,
        });
    }

    let mut request = agent().get(url);
    if have > 0 {
        request = request.header("Range", &format!("bytes={have}-"));
    }

    let response = request
        .call()
        .map_err(|error| DownloadError::Transport(error.to_string()))?;
    let status = response.status().as_u16();

    // 206 means the range was honoured and the body continues where the `.part` stopped.
    // 200 after asking for a range means it was not: the body is the whole file again, so
    // the `.part` has to go or the result would be its first bytes twice over.
    let resumed_from = match (have, status) {
        (0, 200) => 0,
        (have, 206) => have,
        (have, 200) => {
            log::info!("the model host ignored the range request; restarting the download");
            let _ = have;
            0
        }
        // The file on the other end is not the one this `.part` came from. Nothing can be
        // resumed against it, so the partial goes and the next attempt starts clean.
        (_, 416) => {
            let _ = fs::remove_file(&partial);
            return Err(DownloadError::Status(416));
        }
        (_, other) => return Err(DownloadError::Status(other)),
    };

    let mut file = if resumed_from > 0 {
        let mut file = OpenOptions::new().read(true).write(true).open(&partial)?;
        file.seek(SeekFrom::End(0))?;
        file
    } else {
        File::create(&partial)?
    };

    let mut reader = response.into_body().into_reader();
    let mut buffer = vec![0u8; CHUNK];
    let mut written = resumed_from;
    let mut transferred = 0u64;
    let mut last_report = Instant::now()
        .checked_sub(MIN_PROGRESS_GAP)
        .unwrap_or_else(Instant::now);

    loop {
        if !keep_going() {
            // The `.part` keeps everything that arrived, so the next start resumes from it.
            file.flush()?;
            return Err(DownloadError::Stopped);
        }
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            // A connection that breaks mid-transfer leaves the `.part` exactly as long as
            // what arrived, which is what the next call resumes from. Nothing to clean up.
            Err(error) => return Err(DownloadError::Transport(error.to_string())),
        };
        file.write_all(&buffer[..read])?;
        written += read as u64;
        transferred += read as u64;

        if last_report.elapsed() >= MIN_PROGRESS_GAP {
            progress(written, expected_size);
            last_report = Instant::now();
        }
    }
    file.flush()?;
    drop(file);
    progress(written, expected_size);

    settle(&partial, destination, expected_sha256)?;
    Ok(Fetched {
        transferred,
        resumed_from,
    })
}

/// Check the finished `.part` and, if it is the model, put it on its real name.
///
/// The one place a model is accepted, so that the "nothing reaches the final name until the
/// hash matches" rule has exactly one implementation.
fn settle(partial: &Path, destination: &Path, expected_sha256: &str) -> Result<(), DownloadError> {
    let found = hash_file(partial)?;
    if !found.eq_ignore_ascii_case(expected_sha256) {
        // Deleted rather than kept: a `.part` that is already wrong would be resumed for
        // ever, and every attempt would end here.
        let _ = fs::remove_file(partial);
        return Err(DownloadError::Checksum {
            found,
            expected: expected_sha256.to_string(),
        });
    }
    fs::rename(partial, destination)?;
    Ok(())
}

/// The HTTP client, configured for the one thing this module does.
///
/// `http_status_as_error(false)` is the whole reason this is not `ureq::get`: by default a
/// 206 is fine but a 416 comes back as a transport error with the status buried in a string,
/// and **206 and 416 are the two answers a resume actually has to read**. A status code is
/// not an exception here; it is the answer.
fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    )
}

/// The path of the partial download beside `destination`.
#[must_use]
pub fn part_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

/// The sha256 of a file, streamed rather than read into memory.
///
/// # Errors
///
/// The file could not be opened or read.
pub fn hash_file(path: &Path) -> Result<String, io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(hex(&hasher.finalize()))
}

/// Lower-case hexadecimal, which is the form every checksum in the model table is in.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Two hex digits per byte, written by hand rather than through a formatting crate.
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{DownloadError, Fetched, fetch, hash_file, hex, part_path};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// A directory of this test's own, removed at the end of the test that made it.
    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("dile-download-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("make a scratch directory");
        path
    }

    /// The blob every test in this module downloads: 1 MiB of something that is not zeroes,
    /// so that a file of the right length but the wrong content still fails the checksum.
    fn blob() -> Vec<u8> {
        (0..1024 * 1024)
            .map(|index: usize| {
                // A cheap non-repeating-enough pattern; the point is only that it is not
                // constant, so a wrong offset shows up as a wrong hash.
                let value = index.wrapping_mul(2_654_435_761) >> 13;
                (value & 0xff) as u8
            })
            .collect()
    }

    fn sha256_of(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex(&hasher.finalize())
    }

    /// What the test server should do with one request.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Behaviour {
        /// Serve the requested range to the end.
        Whole,
        /// Send 40 % of what was asked for, then drop the connection.
        CutAt40Percent,
        /// Answer 200 with the whole file however the request was framed — a server that
        /// does not do ranges.
        IgnoreRange,
        /// Answer 500.
        Refuse,
        /// Answer 416: the range asked for is not one this file has.
        Unsatisfiable,
    }

    /// A one-file HTTP server on a port the operating system picks.
    ///
    /// Small on purpose: this is not an HTTP implementation, it is the four behaviours a
    /// resume has to survive. It speaks HTTP/1.1, closes every connection, and reads exactly
    /// as much of the request as it needs — the method line and the `Range` header.
    struct Server {
        port: u16,
        /// The requested ranges, in order, so a test can prove the second call asked to
        /// resume rather than starting over.
        seen: Arc<Mutex<Vec<Option<u64>>>>,
        requests: Arc<AtomicU32>,
    }

    impl Server {
        fn start(body: Vec<u8>, script: Vec<Behaviour>) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind a port");
            let port = listener.local_addr().expect("the bound address").port();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let requests = Arc::new(AtomicU32::new(0));

            let thread_seen = Arc::clone(&seen);
            let thread_requests = Arc::clone(&requests);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { break };
                    let index = thread_requests.fetch_add(1, Ordering::SeqCst) as usize;
                    let behaviour = script.get(index).copied().unwrap_or(Behaviour::Whole);
                    let from = serve(stream, &body, behaviour);
                    if let Ok(mut seen) = thread_seen.lock() {
                        seen.push(from);
                    }
                    if index + 1 >= script.len() {
                        // Every scripted request has been answered. Staying alive would only
                        // keep a thread around for the rest of the suite.
                        break;
                    }
                }
            });

            Server {
                port,
                seen,
                requests,
            }
        }

        fn url(&self) -> String {
            format!("http://127.0.0.1:{}/model.bin", self.port)
        }

        fn ranges(&self) -> Vec<Option<u64>> {
            self.seen
                .lock()
                .map(|seen| seen.clone())
                .unwrap_or_default()
        }

        fn count(&self) -> u32 {
            self.requests.load(Ordering::SeqCst)
        }
    }

    /// Answer one request, and report the byte offset it asked to start from.
    fn serve(mut stream: TcpStream, body: &[u8], behaviour: Behaviour) -> Option<u64> {
        let mut reader = BufReader::new(stream.try_clone().expect("clone the socket"));
        let mut from: Option<u64> = None;
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            // Header names are case-insensitive, and a real client uses that: `ureq` writes
            // them lower-cased, so matching `Range:` exactly would silently see no range
            // request at all and this whole test would prove nothing.
            let lowered = trimmed.to_ascii_lowercase();
            if let Some(value) = lowered.strip_prefix("range: bytes=") {
                from = value.split('-').next().and_then(|n| n.parse::<u64>().ok());
            }
        }

        if behaviour == Behaviour::Refuse {
            let _ = stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return from;
        }
        if behaviour == Behaviour::Unsatisfiable {
            let _ = stream.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return from;
        }

        let start = match behaviour {
            Behaviour::IgnoreRange => 0,
            _ => from.unwrap_or(0).min(body.len() as u64) as usize,
        };
        let slice = &body[start..];

        let head = if start > 0 && behaviour != Behaviour::IgnoreRange {
            format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                slice.len(),
                start,
                body.len() - 1,
                body.len()
            )
        } else {
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                slice.len()
            )
        };
        let _ = stream.write_all(head.as_bytes());

        let send = match behaviour {
            Behaviour::CutAt40Percent => slice.len() * 2 / 5,
            _ => slice.len(),
        };
        let _ = stream.write_all(&slice[..send]);
        let _ = stream.flush();
        // The connection is dropped here. For `CutAt40Percent` that is the point: the client
        // has been promised more bytes than it is going to get, which is what a dropped
        // connection in the middle of a real download looks like.
        from
    }

    #[test]
    fn a_download_that_breaks_at_forty_percent_resumes_and_still_hashes_right() {
        let body = blob();
        let expected = sha256_of(&body);
        let server = Server::start(
            body.clone(),
            vec![Behaviour::CutAt40Percent, Behaviour::Whole],
        );

        let directory = scratch("resume");
        let destination = directory.join("model.bin");
        let mut seen_progress: Vec<(u64, u64)> = Vec::new();

        // First attempt: the server hangs up at 40 %, so this fails and leaves a `.part`.
        let first = fetch(
            &server.url(),
            &expected,
            body.len() as u64,
            &destination,
            &mut |done, total| seen_progress.push((done, total)),
            &|| true,
        );
        assert!(
            matches!(
                first,
                Err(DownloadError::Transport(_) | DownloadError::Checksum { .. })
            ),
            "a connection cut in the middle must not look like success: {first:?}"
        );
        assert!(!destination.exists(), "nothing lands on the final name yet");

        let partial = part_path(&destination);
        let have = fs::metadata(&partial)
            .expect("a .part was left behind")
            .len();
        assert!(
            have > 0 && have < body.len() as u64,
            "the .part should hold the 40 % that arrived, not {have} bytes"
        );

        // Second attempt: resumes from exactly what is on disk.
        let second = fetch(
            &server.url(),
            &expected,
            body.len() as u64,
            &destination,
            &mut |done, total| seen_progress.push((done, total)),
            &|| true,
        )
        .expect("the second attempt completes");

        assert_eq!(
            second,
            Fetched {
                transferred: body.len() as u64 - have,
                resumed_from: have,
            },
            "the second attempt must fetch only what was missing"
        );
        assert_eq!(
            server.ranges(),
            vec![None, Some(have)],
            "the first call asks for no range and the second asks to continue"
        );
        assert_eq!(server.count(), 2, "one attempt each, and no retry loop");

        // The file is whole, on its final name, with nothing left over.
        assert_eq!(fs::read(&destination).expect("read the model"), body);
        assert_eq!(hash_file(&destination).expect("hash"), expected);
        assert!(
            !partial.exists(),
            "the .part is renamed, not left beside it"
        );
        assert!(
            seen_progress
                .iter()
                .any(|(done, _)| *done == body.len() as u64),
            "the last progress report is the whole file"
        );

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_part_that_is_already_the_whole_file_is_renamed_without_asking_the_server() {
        // The case that actually happened on this machine, 2026-09-13: a 574 MB model
        // finished and the application was closed in the second before the rename. Asking
        // for `bytes=<size>-` next time would get a 416, and a finished download would look
        // like a broken one for ever. The server here is scripted to refuse every request,
        // so this passes only if no request is made at all.
        let body = blob();
        let expected = sha256_of(&body);
        let server = Server::start(body.clone(), vec![Behaviour::Refuse]);

        let directory = scratch("already-there");
        let destination = directory.join("model.bin");
        fs::write(part_path(&destination), &body).expect("leave a complete .part behind");

        let fetched = fetch(
            &server.url(),
            &expected,
            body.len() as u64,
            &destination,
            &mut |_, _| {},
            &|| true,
        )
        .expect("a complete .part settles");

        assert_eq!(
            fetched.transferred, 0,
            "nothing should come over the network"
        );
        assert_eq!(fetched.resumed_from, body.len() as u64);
        assert_eq!(server.count(), 0, "the server was never asked");
        assert_eq!(fs::read(&destination).expect("read the model"), body);
        assert!(!part_path(&destination).exists());

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_range_the_server_will_not_satisfy_throws_the_partial_away() {
        // A `.part` shorter than the file it claims to be, against a server that answers
        // 416. Nothing can be resumed against that, so the partial goes and the next attempt
        // starts from nothing rather than failing here for ever.
        let body = blob();
        let expected = sha256_of(&body);
        let server = Server::start(body.clone(), vec![Behaviour::Unsatisfiable]);

        let directory = scratch("unsatisfiable");
        let destination = directory.join("model.bin");
        let partial = part_path(&destination);
        fs::write(&partial, &body[..1024]).expect("leave a short .part behind");

        let result = fetch(
            &server.url(),
            &expected,
            body.len() as u64,
            &destination,
            &mut |_, _| {},
            &|| true,
        );
        assert!(matches!(result, Err(DownloadError::Status(416))));
        assert!(!partial.exists(), "an unresumable .part must not be kept");
        assert!(!destination.exists());

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_server_that_ignores_the_range_starts_over_instead_of_doubling_the_file() {
        let body = blob();
        let expected = sha256_of(&body);
        let server = Server::start(
            body.clone(),
            vec![Behaviour::CutAt40Percent, Behaviour::IgnoreRange],
        );

        let directory = scratch("no-ranges");
        let destination = directory.join("model.bin");
        let _ = fetch(
            &server.url(),
            &expected,
            body.len() as u64,
            &destination,
            &mut |_, _| {},
            &|| true,
        );

        let second = fetch(
            &server.url(),
            &expected,
            body.len() as u64,
            &destination,
            &mut |_, _| {},
            &|| true,
        )
        .expect("the second attempt completes");

        assert_eq!(second.resumed_from, 0, "a 200 answer cannot be appended to");
        assert_eq!(second.transferred, body.len() as u64);
        assert_eq!(fs::read(&destination).expect("read the model"), body);

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn bytes_that_are_not_the_model_are_deleted_rather_than_resumed_for_ever() {
        let body = blob();
        let server = Server::start(body.clone(), vec![Behaviour::Whole]);

        let directory = scratch("checksum");
        let destination = directory.join("model.bin");
        let wrong = sha256_of(b"a different file entirely");

        let result = fetch(
            &server.url(),
            &wrong,
            body.len() as u64,
            &destination,
            &mut |_, _| {},
            &|| true,
        );

        match result {
            Err(DownloadError::Checksum { found, expected }) => {
                assert_eq!(expected, wrong);
                assert_eq!(found, sha256_of(&body));
            }
            other => panic!("the wrong bytes must not be accepted: {other:?}"),
        }
        assert!(!destination.exists());
        assert!(
            !part_path(&destination).exists(),
            "a .part that can never become right must not be left to resume"
        );

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_refusal_is_reported_with_its_status_and_nothing_is_written() {
        let body = blob();
        let server = Server::start(body, vec![Behaviour::Refuse]);

        let directory = scratch("refused");
        let destination = directory.join("model.bin");
        let result = fetch(
            &server.url(),
            &sha256_of(b""),
            0,
            &destination,
            &mut |_, _| {},
            &|| true,
        );
        assert!(
            matches!(result, Err(DownloadError::Status(500))),
            "a 500 must be reported with its status, not written to disk: {result:?}"
        );
        assert!(!destination.exists());

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn hashing_streams_the_file_and_agrees_with_a_one_shot_hash() {
        let directory = scratch("hash");
        let path = directory.join("blob.bin");
        let body = blob();
        let mut file = fs::File::create(&path).expect("write the blob");
        file.write_all(&body).expect("write");
        drop(file);

        assert_eq!(hash_file(&path).expect("hash"), sha256_of(&body));
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");

        // And an empty file hashes to the well-known empty digest rather than to an error.
        let empty = directory.join("empty.bin");
        fs::write(&empty, b"").expect("write");
        assert_eq!(
            hash_file(&empty).expect("hash"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_partial_name_sits_beside_the_file_and_not_inside_it() {
        let path = PathBuf::from("models").join("ggml-large-v3-q5_0.bin");
        let partial = part_path(&path);
        assert_eq!(partial.parent(), path.parent());
        assert!(
            partial
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".bin.part")),
            "the extension is added, not replaced: {}",
            partial.display()
        );
    }

    #[test]
    fn a_download_can_be_stopped_between_chunks() {
        let body = blob();
        let expected = sha256_of(&body);
        let server = Server::start(body.clone(), vec![Behaviour::Whole]);

        let directory = scratch("stopped");
        let destination = directory.join("model.bin");
        let result = fetch(
            &server.url(),
            &expected,
            body.len() as u64,
            &destination,
            &mut |_, _| {},
            &|| false,
        );
        assert!(matches!(result, Err(DownloadError::Stopped)));
        assert!(!destination.exists());

        let mut leftover = Vec::new();
        if let Ok(mut file) = fs::File::open(part_path(&destination)) {
            let _ = file.read_to_end(&mut leftover);
        }
        assert!(
            leftover.len() < body.len(),
            "a stopped download must not have finished"
        );

        let _ = fs::remove_dir_all(&directory);
    }
}
