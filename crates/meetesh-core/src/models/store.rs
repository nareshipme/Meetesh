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

/// What a running download is doing. `Verifying` is the (usually short) checksum step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadProgress {
    Bytes { downloaded: u64, total: u64 },
    Verifying,
}

/// Feed a file into `hasher`, stopping early (as `Error::Cancelled`) if `cancel` is set.
fn hash_file_into(hasher: &mut Sha256, path: &Path, cancel: &AtomicBool) -> Result<()> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        hasher.update(&buf[..n]);
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut h = Sha256::new();
    hash_file_into(&mut h, path, &AtomicBool::new(false))?;
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

    /// The main file of a model (what loaders are pointed at).
    pub fn path(&self, e: &ModelEntry) -> PathBuf {
        self.dir.join(&e.file_name)
    }

    /// The folder holding all of a model's files (the models directory itself for a loose file).
    pub fn folder(&self, e: &ModelEntry) -> PathBuf {
        match e.folder() {
            "" => self.dir.clone(),
            sub => self.dir.join(sub),
        }
    }

    /// Cheap check (sizes only). Checksums are verified once, at download time. A model is installed
    /// only when every one of its files is.
    pub fn is_installed(&self, e: &ModelEntry) -> bool {
        files(e).all(|f| self.file_installed(&f))
    }

    fn file_path(&self, f: &FileSpec) -> PathBuf {
        self.dir.join(f.file_name)
    }

    fn part_path(&self, f: &FileSpec) -> PathBuf {
        self.dir.join(format!("{}.part", f.file_name))
    }

    fn file_installed(&self, f: &FileSpec) -> bool {
        fs::metadata(self.file_path(f)).map(|m| m.len() == f.size_bytes).unwrap_or(false)
    }

    pub fn delete(&self, e: &ModelEntry) -> Result<()> {
        for f in files(e) {
            for p in [self.file_path(&f), self.part_path(&f)] {
                if p.exists() {
                    fs::remove_file(p)?;
                }
            }
        }
        Ok(())
    }

    /// Bytes already downloaded for an unfinished download (0 if none). Lets the UI show "Paused at 42%".
    pub fn partial_bytes(&self, e: &ModelEntry) -> u64 {
        if self.is_installed(e) {
            return 0;
        }
        files(e)
            .map(|f| {
                if self.file_installed(&f) {
                    f.size_bytes
                } else {
                    fs::metadata(self.part_path(&f)).map(|m| m.len().min(f.size_bytes)).unwrap_or(0)
                }
            })
            .sum()
    }

    /// Throw away an unfinished download.
    pub fn discard_partial(&self, e: &ModelEntry) -> Result<()> {
        for f in files(e) {
            let p = self.part_path(&f);
            if p.exists() {
                fs::remove_file(p)?;
            }
        }
        Ok(())
    }

    /// Download (or resume) a model: its main file first, then its companions. Progress is reported
    /// over the whole model. A finished file is never fetched again, so a resume continues at the
    /// file that was interrupted.
    pub fn download(&self, e: &ModelEntry, mut on_progress: impl FnMut(DownloadProgress), cancel: &AtomicBool) -> Result<PathBuf> {
        let total = e.total_bytes();
        let mut done = 0u64;
        for f in files(e) {
            self.download_file(&e.id, &f, done, total, &mut on_progress, cancel)?;
            done += f.size_bytes;
        }
        on_progress(DownloadProgress::Bytes { downloaded: total, total });
        Ok(self.path(e))
    }

    /// One file. `before` bytes of the model are already done and `total` is the whole model, so
    /// progress is for the model, not the file.
    ///
    /// The checksum is computed while the bytes arrive, so there is no slow second pass at 100%.
    /// Resuming first re-reads the partial file to continue the hash (reported as `Verifying`).
    /// If `cancel` is set the partial file is kept, so the next call resumes ("pause"). A checksum
    /// mismatch deletes the file.
    fn download_file(
        &self,
        id: &str,
        f: &FileSpec,
        before: u64,
        total: u64,
        on_progress: &mut impl FnMut(DownloadProgress),
        cancel: &AtomicBool,
    ) -> Result<()> {
        let final_path = self.file_path(f);
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if self.file_installed(f) {
            on_progress(DownloadProgress::Bytes { downloaded: before + f.size_bytes, total });
            return Ok(());
        }
        let part = self.part_path(f);
        let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if have > f.size_bytes {
            fs::remove_file(&part)?;
            have = 0;
        }

        // Continue the hash over what is already on disk.
        let mut hasher = Sha256::new();
        if have > 0 {
            on_progress(if have == f.size_bytes {
                DownloadProgress::Verifying
            } else {
                DownloadProgress::Bytes { downloaded: before + have, total }
            });
            hash_file_into(&mut hasher, &part, cancel)?;
        }

        if have < f.size_bytes {
            let mut req = self.agent.get(f.url);
            if have > 0 {
                req = req.set("Range", &format!("bytes={have}-"));
            }
            let resp = match req.call() {
                Ok(r) => r,
                Err(ureq::Error::Status(416, _)) => {
                    // our partial file is unusable; start over
                    let _ = fs::remove_file(&part);
                    return self.download_file(id, f, before, total, on_progress, cancel);
                }
                Err(err) => return Err(Error::Other(format!("download failed: {err}"))),
            };
            // 206: server honoured the range, append. 200: it sent everything, restart.
            let resumed = resp.status() == 206;
            let mut file = if resumed {
                OpenOptions::new().append(true).open(&part)?
            } else {
                have = 0;
                hasher = Sha256::new();
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
                hasher.update(&buf[..n]);
                have += n as u64;
                since_report += n;
                if since_report >= 256 * 1024 {
                    on_progress(DownloadProgress::Bytes { downloaded: before + have, total });
                    since_report = 0;
                }
            }
            file.flush()?;
        }

        let size = fs::metadata(&part)?.len();
        if size != f.size_bytes {
            // keep the partial file: it may just be a dropped connection, and a retry resumes
            return Err(Error::Other(format!("download incomplete ({size} of {} bytes)", f.size_bytes)));
        }
        if !hex(&hasher.finalize()).eq_ignore_ascii_case(f.sha256) {
            let _ = fs::remove_file(&part);
            return Err(Error::Other(format!("checksum mismatch for {id} ({}); the file was discarded", f.file_name)));
        }
        fs::rename(&part, &final_path)?;
        Ok(())
    }
}

/// One file of a model, whether the main file or a companion.
struct FileSpec<'a> {
    file_name: &'a str,
    url: &'a str,
    sha256: &'a str,
    size_bytes: u64,
}

/// The main file first, then the companions.
fn files(e: &ModelEntry) -> impl Iterator<Item = FileSpec<'_>> {
    std::iter::once(FileSpec { file_name: &e.file_name, url: &e.url, sha256: &e.sha256, size_bytes: e.size_bytes }).chain(
        e.companions.iter().map(|c| FileSpec { file_name: &c.file_name, url: &c.url, sha256: &c.sha256, size_bytes: c.size_bytes }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Companion, Role};
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
            engine: Default::default(),
            companions: vec![],
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
        let p = store
            .download(&e, |p| if let DownloadProgress::Bytes { downloaded, total } = p { last = (downloaded, total) }, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(fs::read(&p).unwrap(), b);
        assert_eq!(last, (700_000, 700_000));
        assert!(store.is_installed(&e));
        assert!(!dir.join("test.bin.part").exists());
        // already installed: no network needed
        assert!(store.download(&e, |_| {}, &AtomicBool::new(false)).is_ok());
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
        let p = store.download(&e, |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(p).unwrap(), b);
        // (the test server counts a body after writing it, so give the last count a moment to land)
        for _ in 0..200 {
            if served.load(Ordering::Relaxed) >= 300_000 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
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
        let err = store.download(&e, |_| {}, &AtomicBool::new(false)).unwrap_err();
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
        let r = store.download(
            &e,
            |p| if let DownloadProgress::Bytes { downloaded, .. } = p { if downloaded > 400_000 { cancel.store(true, Ordering::Relaxed) } },
            &cancel,
        );
        assert!(matches!(r, Err(Error::Cancelled)));
        let partial = fs::metadata(dir.join("test.bin.part")).unwrap().len();
        assert!(partial > 0 && partial < 2_000_000);
        let p = store.download(&e, |_| {}, &AtomicBool::new(false)).unwrap();
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
        let p = store(&dir).download(&e, |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(p).unwrap(), b);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reports_partial_size_and_can_discard_it() {
        let b = body(1_000);
        let (url, _) = serve(b.clone());
        let e = entry(&url, &b);
        let dir = tmp("partial");
        fs::create_dir_all(&dir).unwrap();
        let st = store(&dir);
        assert_eq!(st.partial_bytes(&e), 0);
        fs::write(dir.join("test.bin.part"), &b[..400]).unwrap();
        assert_eq!(st.partial_bytes(&e), 400);
        st.discard_partial(&e).unwrap();
        assert_eq!(st.partial_bytes(&e), 0);
        st.discard_partial(&e).unwrap(); // idempotent
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_complete_partial_file_is_verified_not_redownloaded() {
        let b = body(300_000);
        let (url, served) = serve(b.clone());
        let e = entry(&url, &b);
        let dir = tmp("complete-part");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("test.bin.part"), &b).unwrap(); // interrupted right before the rename
        let mut saw_verifying = false;
        let p = store(&dir).download(&e, |p| saw_verifying |= p == DownloadProgress::Verifying, &AtomicBool::new(false)).unwrap();
        assert!(saw_verifying);
        assert_eq!(fs::read(p).unwrap(), b);
        assert_eq!(served.load(Ordering::Relaxed), 0, "nothing should be fetched");
        // and a corrupt complete partial is rejected
        let mut bad = b.clone();
        bad[10] ^= 1;
        fs::remove_file(dir.join("test.bin")).unwrap();
        fs::write(dir.join("test.bin.part"), &bad).unwrap();
        let err = store(&dir).download(&e, |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(err.to_string().contains("checksum"), "{err}");
        fs::remove_dir_all(dir).ok();
    }

    /// A folder model: a main file plus two companions, each from its own server.
    fn folder_entry(bodies: &[Vec<u8>; 3]) -> (ModelEntry, [Arc<AtomicU64>; 3]) {
        let served: Vec<_> = bodies.iter().map(|b| serve(b.clone())).collect();
        let mut e = entry(&served[0].0, &bodies[0]);
        e.file_name = "folder/main.bin".into();
        e.companions = (1..3)
            .map(|i| {
                let c = entry(&served[i].0, &bodies[i]);
                Companion { file_name: format!("folder/extra{i}.bin"), url: c.url, sha256: c.sha256, size_bytes: c.size_bytes }
            })
            .collect();
        (e, [served[0].1.clone(), served[1].1.clone(), served[2].1.clone()])
    }

    #[test]
    fn a_model_made_of_several_files_downloads_into_its_folder_with_whole_model_progress() {
        let bodies = [body(200_000), body(50_000), body(7_000)];
        let (e, served) = folder_entry(&bodies);
        let dir = tmp("folder");
        let st = store(&dir);
        assert!(!st.is_installed(&e));
        let mut last = None;
        let mut monotonic = true;
        let main = st
            .download(
                &e,
                |p| {
                    if let DownloadProgress::Bytes { downloaded, total } = p {
                        monotonic &= last.map_or(true, |l| downloaded >= l);
                        assert_eq!(total, 257_000);
                        last = Some(downloaded);
                    }
                },
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(monotonic, "progress went backwards");
        assert_eq!(last, Some(257_000));
        assert_eq!(main, dir.join("folder/main.bin"));
        assert_eq!(st.folder(&e), dir.join("folder"));
        assert_eq!(fs::read(dir.join("folder/extra2.bin")).unwrap(), bodies[2]);
        assert!(st.is_installed(&e));
        assert_eq!(e.total_bytes(), 257_000);
        // (the test server counts a body after it has been written, so wait for the last count)
        let settled = |s: &Arc<AtomicU64>, n: usize| {
            for _ in 0..200 {
                if s.load(Ordering::Relaxed) == n as u64 {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("server counted {} bytes, expected {n}", s.load(Ordering::Relaxed));
        };
        (0..3).for_each(|i| settled(&served[i], bodies[i].len()));

        // A missing companion makes the model not installed, and only that file is fetched again.
        fs::remove_file(dir.join("folder/extra1.bin")).unwrap();
        assert!(!st.is_installed(&e));
        assert_eq!(st.partial_bytes(&e), 200_000 + 7_000);
        st.download(&e, |_| {}, &AtomicBool::new(false)).unwrap();
        settled(&served[1], 2 * bodies[1].len()); // fetched again
        assert_eq!(served[0].load(Ordering::Relaxed), bodies[0].len() as u64, "the main file must not be fetched again");
        assert_eq!(served[2].load(Ordering::Relaxed), bodies[2].len() as u64);
        assert!(st.is_installed(&e));

        // Deleting removes every file.
        st.delete(&e).unwrap();
        assert!(!dir.join("folder/main.bin").exists() && !dir.join("folder/extra1.bin").exists() && !dir.join("folder/extra2.bin").exists());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_corrupt_companion_is_rejected_and_the_model_stays_not_installed() {
        let bodies = [body(10_000), body(5_000), body(3_000)];
        let (mut e, _) = folder_entry(&bodies);
        e.companions[1].sha256 = "0".repeat(64);
        let dir = tmp("folder-bad");
        let st = store(&dir);
        let err = st.download(&e, |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(err.to_string().contains("checksum") && err.to_string().contains("extra2.bin"), "{err}");
        assert!(!st.is_installed(&e));
        fs::remove_dir_all(dir).ok();
    }
}
