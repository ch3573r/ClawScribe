//! Shared recoverable model transfers. A reservation lives until its worker has
//! closed the file; cancellation never removes another worker's reservation.
use anyhow::{anyhow, bail, Result};
use reqwest::{header, Client, StatusCode};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{fs, io::AsyncWriteExt, sync::watch};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
#[error("Download cancelled by user")]
pub(crate) struct Cancelled;

#[derive(Default)]
pub(crate) struct Downloads(Arc<Mutex<Registry>>);
#[derive(Default)]
struct Registry {
    active: HashMap<String, Arc<Transfer>>,
    generation: u64,
}
struct Transfer {
    token: CancellationToken,
    done: watch::Sender<bool>,
}
pub(crate) struct Reservation {
    registry: Arc<Mutex<Registry>>,
    name: String,
    transfer: Arc<Transfer>,
}
impl Downloads {
    pub fn generation(&self) -> u64 {
        self.0.lock().unwrap().generation
    }
    pub fn start(&self, name: &str) -> Result<Reservation> {
        let mut active = self.0.lock().unwrap();
        if active.active.contains_key(name) {
            bail!(
                "A model operation is already in progress; wait for it to finish before retrying"
            );
        }
        let transfer = Arc::new(Transfer {
            token: CancellationToken::new(),
            done: watch::channel(false).0,
        });
        active.active.insert(name.into(), transfer.clone());
        active.generation = active.generation.wrapping_add(1);
        Ok(Reservation {
            registry: self.0.clone(),
            name: name.into(),
            transfer,
        })
    }
    pub fn is_active(&self, name: &str) -> bool {
        self.0.lock().unwrap().active.contains_key(name)
    }
    pub async fn cancel(&self, name: &str) -> Result<()> {
        let mut done = {
            let active = self.0.lock().unwrap();
            let Some(transfer) = active.active.get(name) else {
                return Ok(());
            };
            transfer.token.cancel();
            transfer.done.subscribe()
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while !*done.borrow_and_update() {
                done.changed()
                    .await
                    .map_err(|_| anyhow!("Download worker ended before cleanup"))?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .map_err(|_| anyhow!("Download cancellation is still pending; wait before retrying"))?
    }
}
impl Reservation {
    pub fn token(&self) -> &CancellationToken {
        &self.transfer.token
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap();
        registry.active.remove(&self.name);
        registry.generation = registry.generation.wrapping_add(1);
        self.transfer.done.send_replace(true);
    }
}

pub(crate) fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".partial");
    PathBuf::from(name)
}

pub(crate) fn client() -> Result<Client> {
    Ok(Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(3600))
        .build()?)
}

async fn http<T>(
    token: &CancellationToken,
    request: impl std::future::Future<Output = std::result::Result<T, reqwest::Error>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = token.cancelled() => Err(Cancelled.into()),
        result = tokio::time::timeout(Duration::from_secs(30), request) => {
            result.map_err(|_| anyhow!("Model server stopped responding; retry to resume"))?
                .map_err(|e| anyhow!("Model transfer interrupted: {}", e.without_url()))
        },
    }
}

/// Validate all byte-range coordinates before appending. Approximate catalog
/// sizes are for presentation only and never prove a transfer is complete.
fn range_total(response: &reqwest::Response, offset: u64) -> Result<u64> {
    let value = response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| anyhow!("Model server omitted Content-Range"))?;
    let value = value
        .strip_prefix("bytes ")
        .ok_or_else(|| anyhow!("Invalid model byte range"))?;
    let (range, total) = value
        .split_once('/')
        .ok_or_else(|| anyhow!("Invalid model byte range"))?;
    let total: u64 = total
        .parse()
        .map_err(|_| anyhow!("Invalid model byte total"))?;
    if response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        if range != "*" || total != offset || total == 0 {
            bail!("Model resume size does not match the server; retry with a fresh partial file");
        }
    } else {
        let (start, end) = range
            .split_once('-')
            .ok_or_else(|| anyhow!("Invalid model byte range"))?;
        let start: u64 = start.parse()?;
        let end: u64 = end.parse()?;
        if start != offset
            || end.checked_add(1) != Some(total)
            || start > end
            || response
                .content_length()
                .is_some_and(|len| len != total - start)
        {
            bail!("Model server returned an inconsistent resume range");
        }
    }
    Ok(total)
}

pub(crate) async fn download_file(
    client: &Client,
    url: &str,
    path: &Path,
    token: &CancellationToken,
    mut progress: impl FnMut(u64, u64),
) -> Result<u64> {
    let operation = async {
        let partial = partial_path(path);
        let final_size = fs::metadata(path).await.ok().map(|m| m.len());
        // HEAD avoids redownloading completed files on servers ignoring Range.
        let remote_size = match http(
            token,
            client
                .head(url)
                .header(header::ACCEPT_ENCODING, "identity")
                .send(),
        )
        .await
        {
            Ok(response) if response.status().is_success() => response
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|n| *n > 0),
            Err(error) if error.is::<Cancelled>() => return Err(error),
            _ => None,
        };
        if final_size.is_some() && final_size == remote_size {
            let size = final_size.unwrap();
            // A completed final file wins over a leftover partial from a crash.
            if fs::try_exists(&partial).await? {
                fs::remove_file(&partial).await?;
            }
            progress(size, size);
            return Ok(size);
        }
        // Preserve legacy partial files written directly to the final filename.
        if final_size.is_some() && !fs::try_exists(&partial).await? {
            fs::rename(path, &partial).await?;
        }
        let mut offset = fs::metadata(&partial).await.ok().map_or(0, |m| m.len());
        if remote_size.is_some_and(|size| offset > size) {
            offset = 0;
        }
        let mut request = client.get(url).header(header::ACCEPT_ENCODING, "identity");
        if offset > 0 {
            request = request.header(header::RANGE, format!("bytes={offset}-"));
        }
        let mut response = http(token, request.send()).await?;
        if response.status() == StatusCode::RANGE_NOT_SATISFIABLE
            && range_total(&response, offset).is_err()
        {
            // A stale partial or a server without usable Range support must be
            // recoverable through Retry without deleting other model files.
            offset = 0;
            response = http(
                token,
                client
                    .get(url)
                    .header(header::ACCEPT_ENCODING, "identity")
                    .send(),
            )
            .await?;
        }
        let total = match response.status() {
            StatusCode::PARTIAL_CONTENT => range_total(&response, offset)?,
            StatusCode::RANGE_NOT_SATISFIABLE if offset > 0 => range_total(&response, offset)?,
            StatusCode::OK => {
                offset = 0; // Range ignored: restart only this incomplete file.
                response
                    .content_length()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| anyhow!("Model server omitted its file size"))?
            }
            status => bail!("Model download failed with HTTP {}", status.as_u16()),
        };
        if remote_size.is_some_and(|size| size != total) {
            bail!("Model changed during download; retry");
        }
        if response.status() != StatusCode::RANGE_NOT_SATISFIABLE {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(offset == 0)
                .append(offset > 0)
                .open(&partial)
                .await?;
            progress(offset, total);
            let mut last_progress = std::time::Instant::now();
            while let Some(bytes) = http(token, response.chunk()).await? {
                if offset.saturating_add(bytes.len() as u64) > total {
                    bail!("Model response exceeded its declared size");
                }
                file.write_all(&bytes).await?;
                // Tokio file writes run on its blocking pool. Complete each
                // write before the next cancellable network wait; dropping a
                // pending write could otherwise overlap the retry's writer.
                file.flush().await?;
                offset += bytes.len() as u64;
                if last_progress.elapsed() >= Duration::from_millis(200) {
                    progress(offset, total);
                    last_progress = std::time::Instant::now();
                }
            }
            file.flush().await?;
            file.sync_all().await?;
            if offset != total {
                bail!("Incomplete model download; retry to resume");
            }
        }
        if token.is_cancelled() {
            return Err(Cancelled.into());
        }
        // Windows cannot replace a destination with rename. A valid completed
        // file returned above; any remaining destination is an invalid legacy file.
        if fs::try_exists(path).await? {
            fs::remove_file(path).await?;
        }
        fs::rename(&partial, path).await?;
        progress(total, total);
        Ok(total)
    };
    operation.await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    async fn server(replies: Vec<&'static str>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 1024];
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(String::from_utf8(request).unwrap());
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
            requests
        });
        (url, task)
    }
    const HEAD: &str = "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n";

    #[tokio::test]
    async fn completed_file_is_preserved_without_get() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model");
        fs::write(&path, b"abcdef").await.unwrap();
        let (url, task) = server(vec![HEAD]).await;
        download_file(
            &client().unwrap(),
            &url,
            &path,
            &CancellationToken::new(),
            |_, _| {},
        )
        .await
        .unwrap();
        let requests = task.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("HEAD "));
        assert_eq!(fs::read(path).await.unwrap(), b"abcdef");
    }

    #[tokio::test]
    async fn partial_file_resumes_and_is_promoted_only_after_completion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model");
        fs::write(partial_path(&path), b"abc").await.unwrap();
        let (url, task) = server(vec![HEAD, "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nConnection: close\r\n\r\ndef"]).await;
        download_file(
            &client().unwrap(),
            &url,
            &path,
            &CancellationToken::new(),
            |_, _| {},
        )
        .await
        .unwrap();
        assert!(task.await.unwrap()[1]
            .to_lowercase()
            .contains("range: bytes=3-"));
        assert_eq!(fs::read(&path).await.unwrap(), b"abcdef");
        assert!(!partial_path(&path).exists());
    }

    #[tokio::test]
    async fn ignored_range_restarts_only_the_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model");
        fs::write(partial_path(&path), b"old").await.unwrap();
        let (url, task) = server(vec![
            HEAD,
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef",
        ])
        .await;
        download_file(
            &client().unwrap(),
            &url,
            &path,
            &CancellationToken::new(),
            |_, _| {},
        )
        .await
        .unwrap();
        task.await.unwrap();
        assert_eq!(fs::read(&path).await.unwrap(), b"abcdef");
    }

    #[tokio::test]
    async fn invalid_range_and_truncated_body_cannot_become_available() {
        for reply in [
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 2-4/6\r\nConnection: close\r\n\r\ndef",
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nConnection: close\r\n\r\nd",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("model");
            fs::write(partial_path(&path), b"abc").await.unwrap();
            let (url, task) = server(vec![HEAD, reply]).await;
            assert!(download_file(&client().unwrap(), &url, &path, &CancellationToken::new(), |_, _| {}).await.is_err());
            task.await.unwrap();
            assert!(!path.exists());
            assert!(fs::read(partial_path(&path)).await.unwrap().starts_with(b"abc"));
        }
    }

    #[tokio::test]
    async fn complete_partial_accepts_exact_416_and_stale_range_restarts() {
        for complete in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("model");
            fs::write(
                partial_path(&path),
                if complete {
                    b"abcdef".as_slice()
                } else {
                    b"abc".as_slice()
                },
            )
            .await
            .unwrap();
            let mut replies = vec![HEAD, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nContent-Range: bytes */6\r\nConnection: close\r\n\r\n"];
            if !complete {
                replies.push(
                    "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef",
                );
            }
            let (url, task) = server(replies).await;
            download_file(
                &client().unwrap(),
                &url,
                &path,
                &CancellationToken::new(),
                |_, _| {},
            )
            .await
            .unwrap();
            assert_eq!(task.await.unwrap().len(), if complete { 2 } else { 3 });
            assert_eq!(fs::read(path).await.unwrap(), b"abcdef");
        }
    }

    #[tokio::test]
    async fn cancellation_reserves_model_until_worker_cleanup_and_is_per_model() {
        let downloads = Arc::new(Downloads::default());
        let reservation = downloads.start("first").unwrap();
        let other = downloads.start("second").unwrap();
        let registry = downloads.clone();
        let cancelling = tokio::spawn(async move { registry.cancel("first").await });
        reservation.token().cancelled().await;
        assert!(downloads.start("first").is_err());
        assert!(!other.token().is_cancelled());
        assert!(!cancelling.is_finished());
        drop(reservation);
        cancelling.await.unwrap().unwrap();
        assert!(downloads.start("first").is_ok());
    }

    #[tokio::test]
    async fn cancellation_during_body_keeps_written_prefix_ready_for_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            for reply in [HEAD, "HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabc"] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let mut bytes = [0; 1024];
                    let count = socket.read(&mut bytes).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&bytes[..count]);
                }
                socket.write_all(reply.as_bytes()).await.unwrap();
                if reply != HEAD {
                    std::future::pending::<()>().await;
                }
            }
        });
        let token = CancellationToken::new();
        let cancel_after_write = async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while fs::metadata(partial_path(&path))
                    .await
                    .map_or(true, |m| m.len() < 3)
                {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            token.cancel();
        };
        let client = client().unwrap();
        let (result, ()) = tokio::join!(
            download_file(&client, &url, &path, &token, |_, _| {}),
            cancel_after_write
        );
        task.abort();
        assert!(result.unwrap_err().is::<Cancelled>());
        assert!(!path.exists());
        assert_eq!(fs::read(partial_path(&path)).await.unwrap(), b"abc");
        let (url, task) = server(vec![HEAD, "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nConnection: close\r\n\r\ndef"]).await;
        download_file(&client, &url, &path, &CancellationToken::new(), |_, _| {})
            .await
            .unwrap();
        task.await.unwrap();
        assert_eq!(fs::read(path).await.unwrap(), b"abcdef");
    }

    #[tokio::test]
    async fn cancellation_interrupts_stalled_http_and_keeps_partial_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model");
        fs::write(partial_path(&path), b"abc").await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let token = CancellationToken::new();
        let cancel = token.clone();
        let task = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            cancel.cancel();
            std::future::pending::<()>().await;
        });
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            download_file(&client().unwrap(), &url, &path, &token, |_, _| {}),
        )
        .await
        .unwrap();
        task.abort();
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert_eq!(fs::read(partial_path(&path)).await.unwrap(), b"abc");
    }
}
