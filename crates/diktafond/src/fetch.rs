//! Resumable HTTPS downloader for model files, ported from Handy's
//! (github.com/cjpais/Handy, MIT) download.rs semantics: a `.partial` file
//! with Range resume, the manifest's sha256 as the trust anchor, and an
//! atomic rename at the end.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A body chunk must arrive at least this often or the attempt is abandoned.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(not(test))]
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];
#[cfg(test)]
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(10),
    Duration::from_millis(10),
    Duration::from_millis(10),
];

pub struct RemoteFile {
    pub url: String,
    pub size: u64,
    /// Lowercase hex sha256 of the complete file. This is what makes resumed
    /// and mirrored downloads safe at all.
    pub sha256: String,
}

/// Marks failures a retry cannot fix: a wrong manifest hash on freshly
/// downloaded bytes, or a 4xx from the server.
#[derive(Debug)]
struct Permanent;

impl std::fmt::Display for Permanent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not recoverable by retrying")
    }
}

/// Download `file` to `dest`, resuming any `.partial` from an earlier attempt
/// and retrying transient failures. `progress` receives
/// `(downloaded_bytes, total_bytes)` as chunks arrive; the value can go
/// backwards when an attempt restarts from scratch.
pub fn fetch(file: &RemoteFile, dest: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building fetch runtime")?;
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()?;
    runtime.block_on(async {
        let mut last_error = None;
        for delay in std::iter::once(Duration::ZERO).chain(RETRY_DELAYS) {
            tokio::time::sleep(delay).await;
            match fetch_once(&client, file, dest, progress).await {
                Ok(()) => return Ok(()),
                Err(e) if e.downcast_ref::<Permanent>().is_some() => return Err(e),
                Err(e) => {
                    eprintln!("download of {} failed: {e:#}", file.url);
                    last_error = Some(e);
                }
            }
        }
        Err(last_error.expect("at least one attempt ran"))
    })
}

async fn fetch_once(
    client: &reqwest::Client,
    file: &RemoteFile,
    dest: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    let partial = partial_path(dest);
    let offset = match fs::metadata(&partial) {
        // A full-size partial is a crash between download and finalize; verify
        // it instead of re-requesting (a Range at EOF would 416-loop).
        Ok(meta) if meta.len() == file.size => return verify_and_finalize(file, &partial, dest),
        Ok(meta) if meta.len() > file.size => {
            fs::remove_file(&partial)?;
            0
        }
        Ok(meta) => meta.len(),
        Err(_) => 0,
    };

    let mut request = client.get(&file.url);
    if offset > 0 {
        request = request.header("Range", format!("bytes={offset}-"));
    }
    let mut response = request.send().await.context("sending request")?;

    let status = response.status().as_u16();
    let (mut out, mut downloaded) = match status {
        206 if offset > 0 => {
            // A malformed or mismatched Content-Range poisons the partial;
            // discarding it lets the retry recover with a plain 200.
            let start = match content_range_start(&response) {
                Ok(start) => start,
                Err(e) => {
                    fs::remove_file(&partial)?;
                    return Err(e);
                }
            };
            if start != offset {
                fs::remove_file(&partial)?;
                bail!("server resumed at byte {start}, expected {offset}");
            }
            (fs::File::options().append(true).open(&partial)?, offset)
        }
        // Fresh download, or the server ignored the Range header: start over.
        200 => (fs::File::create(&partial)?, 0),
        // Our partial confused the server; only the hash can bless it.
        416 if offset > 0 => {
            return verify_and_finalize(file, &partial, dest)
                .context("range not satisfiable and the partial failed verification");
        }
        _ => {
            let error = anyhow::anyhow!("unexpected HTTP status {status}");
            return Err(if (400..500).contains(&status) {
                error.context(Permanent)
            } else {
                error
            });
        }
    };

    progress(downloaded, file.size);
    loop {
        let chunk = match tokio::time::timeout(STALL_TIMEOUT, response.chunk())
            .await
            .context("download stalled")?
            .context("reading body")?
        {
            Some(chunk) => chunk,
            None => break,
        };
        if downloaded + chunk.len() as u64 > file.size {
            // An untrusted mirror must not be able to fill the disk.
            drop(out);
            fs::remove_file(&partial)?;
            bail!("server sent more than the expected {} bytes", file.size);
        }
        out.write_all(&chunk)?;
        downloaded += chunk.len() as u64;
        progress(downloaded, file.size);
    }
    drop(out);

    if downloaded != file.size {
        bail!("download ended at {downloaded} of {} bytes", file.size);
    }
    // A mismatch on bytes we just downloaded in full means the manifest and
    // the source disagree; retrying would download the whole file again for
    // the same result.
    verify_and_finalize(file, &partial, dest).map_err(|e| e.context(Permanent))
}

fn verify_and_finalize(file: &RemoteFile, partial: &Path, dest: &Path) -> Result<()> {
    let actual = sha256_of(partial)?;
    if actual != file.sha256.to_lowercase() {
        fs::remove_file(partial)?;
        bail!("sha256 mismatch: expected {}, got {actual}", file.sha256);
    }
    fs::rename(partial, dest).context("moving verified download into place")?;
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    dest.with_file_name(name)
}

/// Parse the start offset out of `Content-Range: bytes <start>-<end>/<total>`.
fn content_range_start(response: &reqwest::Response) -> Result<u64> {
    let header = response
        .headers()
        .get("Content-Range")
        .context("206 without Content-Range")?
        .to_str()?;
    header
        .strip_prefix("bytes ")
        .and_then(|rest| rest.split('-').next())
        .and_then(|start| start.parse().ok())
        .with_context(|| format!("unparseable Content-Range {header:?}"))
}

/// Minimal HTTP/1.1 test server shared by the fetch and manifest tests.
#[cfg(test)]
pub(crate) mod test_server {
    use std::io::{BufRead, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[derive(Clone, Copy)]
    pub enum Behavior {
        Normal,
        IgnoreRange,
        WrongContentRange,
        RangeNotSatisfiable,
        Oversend,
        NotFound,
    }

    /// Serves one file at any path; records the Range offset of every request.
    pub fn serve(body: Vec<u8>, behavior: Behavior) -> (String, Arc<Mutex<Vec<Option<u64>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://127.0.0.1:{}/file.bin",
            listener.local_addr().unwrap().port()
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = v.trim().trim_end_matches('-').parse::<u64>().ok();
                    }
                }
                log.lock().unwrap().push(range);
                let len = body.len();
                let (head, payload): (String, &[u8]) = match (behavior, range) {
                    (Behavior::Normal, Some(off)) => (
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {off}-{}/{len}\r\nContent-Length: {}\r\n",
                            len - 1,
                            len as u64 - off
                        ),
                        &body[off as usize..],
                    ),
                    (Behavior::WrongContentRange, Some(_)) => (
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-{}/{len}\r\nContent-Length: {len}\r\n",
                            len - 1
                        ),
                        &body[..],
                    ),
                    (Behavior::RangeNotSatisfiable, Some(_)) => (
                        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n".into(),
                        &[][..],
                    ),
                    (Behavior::NotFound, _) => (
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n".into(),
                        &[][..],
                    ),
                    (Behavior::Oversend, _) => (
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n", len + 64),
                        // Payload extended below.
                        &body[..],
                    ),
                    _ => (
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {len}\r\n"),
                        &body[..],
                    ),
                };
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(b"Connection: close\r\n\r\n");
                let _ = stream.write_all(payload);
                if matches!(behavior, Behavior::Oversend) {
                    let _ = stream.write_all(&[0u8; 64]);
                }
            }
        });
        (url, requests)
    }

    pub fn test_body() -> Vec<u8> {
        (0..100_000u32).flat_map(|i| i.to_le_bytes()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::{Behavior, serve, test_body};
    use super::*;

    fn remote(url: String, body: &[u8]) -> RemoteFile {
        RemoteFile {
            url,
            size: body.len() as u64,
            sha256: hex::encode(Sha256::digest(body)),
        }
    }

    fn dest_in_tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dkt-fetch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("model.bin")
    }

    #[test]
    fn downloads_fresh_file() {
        let body = test_body();
        let (url, requests) = serve(body.clone(), Behavior::Normal);
        let dest = dest_in_tempdir("fresh");
        let mut peak = 0;
        fetch(&remote(url, &body), &dest, &mut |done, total| {
            assert!(done >= peak && done <= total);
            peak = done;
        })
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body);
        assert_eq!(peak, body.len() as u64);
        assert_eq!(*requests.lock().unwrap(), vec![None]);
        assert!(!partial_path(&dest).exists());
    }

    #[test]
    fn resumes_from_partial() {
        let body = test_body();
        let (url, requests) = serve(body.clone(), Behavior::Normal);
        let dest = dest_in_tempdir("resume");
        fs::write(partial_path(&dest), &body[..12_345]).unwrap();
        fetch(&remote(url, &body), &dest, &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body);
        assert_eq!(*requests.lock().unwrap(), vec![Some(12_345)]);
    }

    #[test]
    fn restarts_clean_when_server_ignores_range() {
        let body = test_body();
        let (url, requests) = serve(body.clone(), Behavior::IgnoreRange);
        let dest = dest_in_tempdir("ignore-range");
        fs::write(partial_path(&dest), vec![0xAB; 9_999]).unwrap();
        fetch(&remote(url, &body), &dest, &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body);
        assert_eq!(*requests.lock().unwrap(), vec![Some(9_999)]);
    }

    #[test]
    fn discards_partial_on_wrong_content_range_then_recovers() {
        let body = test_body();
        let (url, requests) = serve(body.clone(), Behavior::WrongContentRange);
        let dest = dest_in_tempdir("wrong-range");
        fs::write(partial_path(&dest), &body[..5_000]).unwrap();
        fetch(&remote(url, &body), &dest, &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body);
        // First attempt: rejected mid-handshake, partial discarded. Retry has
        // no partial, sends no Range, and the plain 200 succeeds.
        assert_eq!(*requests.lock().unwrap(), vec![Some(5_000), None]);
    }

    #[test]
    fn discards_garbage_partial_on_416_then_recovers() {
        let body = test_body();
        let (url, requests) = serve(body.clone(), Behavior::RangeNotSatisfiable);
        let dest = dest_in_tempdir("garbage-416");
        fs::write(partial_path(&dest), vec![0xCD; 4_242]).unwrap();
        fetch(&remote(url, &body), &dest, &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body);
        assert_eq!(*requests.lock().unwrap(), vec![Some(4_242), None]);
    }

    #[test]
    fn accepts_full_valid_partial_without_any_request() {
        let body = test_body();
        let (url, requests) = serve(body.clone(), Behavior::Normal);
        let dest = dest_in_tempdir("full-partial");
        fs::write(partial_path(&dest), &body).unwrap();
        fetch(&remote(url, &body), &dest, &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), body);
        assert!(requests.lock().unwrap().is_empty());
    }

    #[test]
    fn rejects_oversending_server() {
        let body = test_body();
        let (url, _) = serve(body.clone(), Behavior::Oversend);
        let dest = dest_in_tempdir("oversend");
        let err = fetch(&remote(url, &body), &dest, &mut |_, _| {}).unwrap_err();
        assert!(err.to_string().contains("more than the expected"), "{err}");
        assert!(!dest.exists());
        assert!(!partial_path(&dest).exists());
    }

    #[test]
    fn rejects_hash_mismatch_without_retrying() {
        let body = test_body();
        let (url, requests) = serve(body.clone(), Behavior::Normal);
        let dest = dest_in_tempdir("bad-hash");
        let mut file = remote(url, &body);
        file.sha256 = "0".repeat(64);
        let err = fetch(&file, &dest, &mut |_, _| {}).unwrap_err();
        assert!(format!("{err:#}").contains("sha256 mismatch"), "{err:#}");
        assert!(!dest.exists());
        assert!(!partial_path(&dest).exists());
        // A wrong manifest hash is permanent; retries would re-download for
        // the same result.
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn rejects_missing_file_without_retrying() {
        let body = test_body();
        let (url, requests) = serve(body.clone(), Behavior::NotFound);
        let dest = dest_in_tempdir("not-found");
        let err = fetch(&remote(url, &body), &dest, &mut |_, _| {}).unwrap_err();
        assert!(format!("{err:#}").contains("404"), "{err:#}");
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}
