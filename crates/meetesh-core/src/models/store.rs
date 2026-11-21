//! Downloads models with resume, progress, cancel and checksum verification.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use crate::models::ModelEntry;
use crate::{Error, Result};

pub struct ModelStore {
    dir: PathBuf,
    agent: ureq::Agent,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

impl ModelStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        // Honour HTTPS_PROXY / HTTP_PROXY, since many corporate networks require it.
        let agent = ureq::AgentBuilder::new().try_proxy_from_env(true).build();
        ModelStore { dir: dir.into(), agent }
    }

    /// Use a specific HTTP agent (custom proxy or certificates, or no proxy in tests).
    pub fn with_agent(dir: impl Into<PathBuf>, agent: ureq::Agent) -> Self {
        ModelStore { dir: dir.into(), agent }
    }

    pub fn path(&self, e: &ModelEntry) -> PathBuf {
        self.dir.join(&e.file_name)
    }

    fn part_path(&self, e: &ModelEntry) -> PathBuf {
        self.dir.join(format!("{}.part", e.file_name))
    }

    /// Cheap check (size only). The checksum is verified once, at download time.
    pub fn is_installed(&self, e: &ModelEntry) -> bool {
        fs::metadata(self.path(e)).map(|m| m.len() == e.size_bytes).unwrap_or(false)
    }

    pub fn delete(&self, e: &ModelEntry) -> Result<()> {
        for p in [self.path(e), self.part_path(e)] {
            if p.exists() {
                fs::remove_file(p)?;
            }
        }
        Ok(())
    }

    /// Download (or resume) a model. `on_progress(downloaded, total)` is called as bytes arrive.
    /// On cancel the partial file is kept so the next call resumes. A checksum mismatch deletes it.
    pub fn download(&self, e: &ModelEntry, mut on_progress: impl FnMut(u64, u64), cancel: &AtomicBool) -> Result<PathBuf> {
        fs::create_dir_all(&self.dir)?;
        let final_path = self.path(e);
        if self.is_installed(e) {
            on_progress(e.size_bytes, e.size_bytes);
            return Ok(final_path);
        }
        let part = self.part_path(e);
        let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if have > e.size_bytes {
            fs::remove_file(&part)?;
            have = 0;
        }

        if have < e.size_bytes {
            let mut req = self.agent.get(&e.url);
            if have > 0 {
                req = req.set("Range", &format!("bytes={have}-"));
            }
            let resp = match req.call() {
                Ok(r) => r,
                Err(ureq::Error::Status(416, _)) => {
                    // our partial file is unusable; start over
                    let _ = fs::remove_file(&part);
                    return self.download(e, on_progress, cancel);
                }
                Err(err) => return Err(Error::Other(format!("download failed: {err}"))),
            };
            // 206: server honoured the range, append. 200: it sent everything, restart.
            let resumed = resp.status() == 206;
            let mut file = if resumed {
                OpenOptions::new().append(true).open(&part)?
            } else {
                have = 0;
                File::create(&part)?
            };
            let mut reader = resp.into_reader();
            let mut buf = vec![0u8; 64 * 1024];
            let mut since_report = 0usize;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    file.flush()?;
                    return Err(Error::Cancelled);
                }
                let n = reader.read(&mut buf).map_err(|err| Error::Other(format!("download interrupted: {err}")))?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n])?;
                have += n as u64;
                since_report += n;
                if since_report >= 256 * 1024 {
                    on_progress(have, e.size_bytes);
                    since_report = 0;
                }
            }
            file.flush()?;
        }

        let size = fs::metadata(&part)?.len();
        if size != e.size_bytes {
            // keep the partial file: it may just be a dropped connection, and a retry resumes
            return Err(Error::Other(format!("download incomplete ({size} of {} bytes)", e.size_bytes)));
        }
        if !sha256_file(&part)?.eq_ignore_ascii_case(&e.sha256) {
            let _ = fs::remove_file(&part);
            return Err(Error::Other(format!("checksum mismatch for {}; the file was discarded", e.id)));
        }
        fs::rename(&part, &final_path)?;
        on_progress(e.size_bytes, e.size_bytes);
        Ok(final_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Role;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    /// Minimal HTTP server with Range support. Counts body bytes served.
    fn serve(body: Vec<u8>) -> (String, Arc<AtomicU64>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
        let served = Arc::new(AtomicU64::new(0));
        let counter = served.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut range_from = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range_from = v.trim().trim_end_matches('-').parse::<usize>().ok();
                    }
                }
                let (status, start) = match range_from {
                    Some(s) if s < body.len() => ("206 Partial Content", s),
                    Some(_) => ("416 Range Not Satisfiable", 0),
                    None => ("200 OK", 0),
                };
                let data = if status.starts_with("416") { &[][..] } else { &body[start..] };
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len());
                let _ = stream.write_all(head.as_bytes());
                if stream.write_all(data).is_ok() {
                    counter.fetch_add(data.len() as u64, Ordering::Relaxed);
                }
            }
        });
        (url, served)
    }

    fn entry(url: &str, body: &[u8]) -> ModelEntry {
        let mut h = Sha256::new();
        h.update(body);
        ModelEntry {
            id: "test".into(),
            role: Role::Asr,
            name: "Test".into(),
            description: String::new(),
            file_name: "test.bin".into(),
            url: url.into(),
            sha256: hex(&h.finalize()),
            size_bytes: body.len() as u64,
            license: "MIT".into(),
            language: String::new(),
            min_ram_mb: 0,
        }
    }

    fn store(dir: &Path) -> ModelStore {
        ModelStore::with_agent(dir, ureq::AgentBuilder::new().build()) // direct: never via an env proxy
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meetesh-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn body(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    #[test]
    fn downloads_and_verifies() {
        let b = body(700_000);
        let (url, _) = serve(b.clone());
        let e = entry(&url, &b);
        let dir = tmp("full");
        let store = store(&dir);
        let mut last = (0, 0);
        let p = store.download(&e, |d, t| last = (d, t), &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b);
        assert_eq!(last, (700_000, 700_000));
        assert!(store.is_installed(&e));
        assert!(!dir.join("test.bin.part").exists());
        // already installed: no network needed
        assert!(store.download(&e, |_, _| {}, &AtomicBool::new(false)).is_ok());
        store.delete(&e).unwrap();
        assert!(!store.is_installed(&e));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn resumes_a_partial_download_without_refetching() {
        let b = body(900_000);
        let (url, served) = serve(b.clone());
        let e = entry(&url, &b);
        let dir = tmp("resume");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("test.bin.part"), &b[..600_000]).unwrap();
        let store = store(&dir);
        let p = store.download(&e, |_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(p).unwrap(), b);
        assert_eq!(served.load(Ordering::Relaxed), 300_000, "only the missing tail may be transferred");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn corrupt_download_is_rejected_and_removed() {
        let b = body(100_000);
        let mut corrupt = b.clone();
        corrupt[500] ^= 0xFF;
        let (url, _) = serve(corrupt);
        let e = entry(&url, &b); // checksum of the good bytes
        let dir = tmp("bad");
        let store = store(&dir);
        let err = store.download(&e, |_, _| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(err.to_string().contains("checksum"), "{err}");
        assert!(!store.is_installed(&e));
        assert!(!dir.join("test.bin.part").exists());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn cancel_keeps_partial_file_and_next_call_finishes() {
        let b = body(2_000_000);
        let (url, _) = serve(b.clone());
        let e = entry(&url, &b);
        let dir = tmp("cancel");
        let store = store(&dir);
        let cancel = AtomicBool::new(false);
        let r = store.download(&e, |d, _| if d > 400_000 { cancel.store(true, Ordering::Relaxed) }, &cancel);
        assert!(matches!(r, Err(Error::Cancelled)));
        let partial = fs::metadata(dir.join("test.bin.part")).unwrap().len();
        assert!(partial > 0 && partial < 2_000_000);
        let p = store.download(&e, |_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(p).unwrap(), b);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn oversized_partial_file_is_discarded() {
        let b = body(50_000);
        let (url, _) = serve(b.clone());
        let e = entry(&url, &b);
        let dir = tmp("big");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("test.bin.part"), vec![1u8; 80_000]).unwrap();
        let p = store(&dir).download(&e, |_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(p).unwrap(), b);
        fs::remove_dir_all(dir).ok();
    }
}
