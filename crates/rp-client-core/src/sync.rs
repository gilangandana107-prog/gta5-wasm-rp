//! Zero-extract direct-to-disk HTTP/1.1 Keep-Alive + Range Asset Sync Engine.
//!
//! Shared across iOS, Android, macOS, Windows, and Linux clients so players can download
//! trimmed Roleplay assets directly into `game/` with 0x `.zip` extraction overhead and
//! automatic byte-level resume on Wi-Fi/mobile data drops.

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_SYNC_PORT: u16 = 47521;
const WORKER_THREADS: usize = 8;
const SKIPPED_DIRS: &[&str] = &["env_test", "verityradio"];

static CANCEL_FLAG: AtomicBool = AtomicBool::new(false);

pub fn request_cancel() {
    CANCEL_FLAG.store(true, Ordering::SeqCst);
}

struct SyncFileEntry {
    path: String,
    size: u64,
}

fn parse_sync_index(bytes: &[u8]) -> Result<Vec<SyncFileEntry>> {
    let val: serde_json::Value =
        serde_json::from_slice(bytes).context("Format sync-index.json tidak valid")?;
    let arr = val
        .get("files")
        .and_then(|v| v.as_array())
        .context("Field 'files' tidak ditemukan di sync-index.json")?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        if let (Some(p), Some(sz)) = (
            item.get("path").and_then(|v| v.as_str()),
            item.get("size").and_then(|v| v.as_u64()),
        ) {
            out.push(SyncFileEntry {
                path: p.to_string(),
                size: sz,
            });
        }
    }
    Ok(out)
}

pub fn parse_endpoint(raw: &str) -> Result<(String, u16)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("Alamat server kosong");
    }
    let no_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    let host_port = no_scheme.split('/').next().unwrap_or(no_scheme).trim();
    if let Some((h, p_str)) = host_port.rsplit_once(':') {
        if let Ok(p) = p_str.parse::<u16>() {
            let sync_port = if p == rp_proto::DEFAULT_PORT {
                DEFAULT_SYNC_PORT
            } else {
                p
            };
            return Ok((h.to_string(), sync_port));
        }
    }
    Ok((host_port.to_string(), DEFAULT_SYNC_PORT))
}

pub fn sync_game_data(
    server_input: &str,
    token: Option<&str>,
    dest: &Path,
    keep_code: bool,
    is_already_ready: bool,
    progress: impl Fn(u64, u64) + Send + Sync,
) -> Result<()> {
    CANCEL_FLAG.store(false, Ordering::SeqCst);
    let (host, port) = parse_endpoint(server_input)?;
    let clean_token = token
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| option_env!("RP_DEFAULT_SYNC_TOKEN").filter(|s| !s.is_empty()));

    let mut bootstrap = KeepAliveConn::connect(&host, port, clean_token)?;
    let index_bytes = bootstrap
        .fetch_bytes("/sync-index.json")
        .context("Gagal mengambil daftar file kota dari server")?;
    let index_files = parse_sync_index(&index_bytes)?;

    let wanted = |rel: &str| -> bool {
        if !(rel.starts_with("data/") || rel.starts_with("b/")) || rel.ends_with('/') {
            return false;
        }
        if rel.split('/').any(|c| {
            c.is_empty()
                || c == "."
                || c == ".."
                || SKIPPED_DIRS.contains(&c.to_ascii_lowercase().as_str())
        }) {
            return false;
        }
        if let Some(build) = rel.strip_prefix("b/") {
            let inner = build.split_once('/').map(|(_, r)| r).unwrap_or("");
            return inner.starts_with("shaders/") || (keep_code && inner == "game.wasm");
        }
        true
    };

    let mut regular_jobs: Vec<(String, u64)> = Vec::new();
    let mut manifest_job: Option<(String, u64)> = None;
    let mut total_bytes = 0u64;

    for entry in index_files {
        let rel = entry.path.replace('\\', "/");
        if !wanted(&rel) {
            continue;
        }
        total_bytes += entry.size;
        if rel == "data/manifest.json" {
            manifest_job = Some((rel, entry.size));
        } else {
            regular_jobs.push((rel, entry.size));
        }
    }

    if manifest_job.is_none() {
        bail!("Server sync tidak memiliki data/manifest.json");
    }

    std::fs::create_dir_all(dest)?;

    let final_manifest_path = dest.join("data/manifest.json");
    if final_manifest_path.exists() && !is_already_ready {
        let _ = std::fs::remove_file(&final_manifest_path);
    }

    let mut pending_jobs: Vec<(String, u64)> = Vec::with_capacity(regular_jobs.len());
    let mut initial_done = 0u64;

    for (rel, sz) in regular_jobs {
        let target = dest.join(&rel);
        if let Ok(meta) = std::fs::metadata(&target) {
            if meta.is_file() && meta.len() == sz {
                initial_done += sz;
                continue;
            }
        }
        let part = dest.join(format!("{rel}.part"));
        if let Ok(pmeta) = std::fs::metadata(&part) {
            if pmeta.is_file() && pmeta.len() < sz {
                initial_done += pmeta.len();
            }
        }
        pending_jobs.push((rel, sz));
    }

    progress(initial_done, total_bytes);

    let done_atomic = Arc::new(AtomicU64::new(initial_done));
    let job_cursor = Arc::new(AtomicUsize::new(0));
    let first_err: Arc<Mutex<Option<anyhow::Error>>> = Arc::new(Mutex::new(None));
    let last_report_ms = Arc::new(AtomicU64::new(0));
    let t_start = Instant::now();

    let jobs_arc = Arc::new(pending_jobs);
    let worker_count = WORKER_THREADS.min(jobs_arc.len().max(1));

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let jobs = Arc::clone(&jobs_arc);
            let cursor = Arc::clone(&job_cursor);
            let done = Arc::clone(&done_atomic);
            let err_slot = Arc::clone(&first_err);
            let last_ms = Arc::clone(&last_report_ms);
            let host_ref = host.as_str();
            let prog_ref = &progress;

            scope.spawn(move || {
                let mut conn: Option<KeepAliveConn> = None;
                let mut copy_buf = vec![0u8; 256 << 10];

                loop {
                    if CANCEL_FLAG.load(Ordering::Relaxed) || err_slot.lock().unwrap().is_some() {
                        break;
                    }
                    let idx = cursor.fetch_add(1, Ordering::Relaxed);
                    if idx >= jobs.len() {
                        break;
                    }
                    let (ref rel, expected_size) = jobs[idx];

                    let res = download_one_with_retries(
                        &mut conn,
                        host_ref,
                        port,
                        clean_token,
                        dest,
                        rel,
                        expected_size,
                        &mut copy_buf,
                        &done,
                        total_bytes,
                        &t_start,
                        &last_ms,
                        prog_ref,
                    );

                    if let Err(e) = res {
                        let mut guard = err_slot.lock().unwrap();
                        if guard.is_none() {
                            *guard = Some(e);
                        }
                        break;
                    }
                }
            });
        }
    });

    if CANCEL_FLAG.load(Ordering::SeqCst) {
        bail!("Download dijeda (progres tersimpan, klik lagi kapan saja untuk lanjut)");
    }
    if let Some(e) = first_err.lock().unwrap().take() {
        return Err(e);
    }

    // Download `data/manifest.json` LAST as atomic readiness commit marker!
    let (mf_rel, mf_size) = manifest_job.unwrap();
    let mut mf_conn: Option<KeepAliveConn> = Some(bootstrap);
    let mut mf_buf = vec![0u8; 128 << 10];
    download_one_with_retries(
        &mut mf_conn,
        &host,
        port,
        clean_token,
        dest,
        &mf_rel,
        mf_size,
        &mut mf_buf,
        &done_atomic,
        total_bytes,
        &t_start,
        &last_report_ms,
        &progress,
    )?;

    tidy_synced_lists(&dest.join("data"))?;
    progress(total_bytes, total_bytes);
    Ok(())
}

fn tidy_synced_lists(data: &Path) -> Result<()> {
    let dlc = data.join("common/data/dlcList.xml");
    if let Ok(text) = std::fs::read_to_string(&dlc) {
        let kept: Vec<&str> = text
            .lines()
            .filter(|l| !SKIPPED_DIRS.iter().any(|d| l.to_ascii_lowercase().contains(d)))
            .collect();
        std::fs::write(&dlc, kept.join("\n") + "\n")?;
    }
    let manifest_path = data.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path)?).context("manifest.json")?;
    let present: HashSet<PathBuf> = walk_files(data);
    if let Some(files) = manifest.get_mut("files").and_then(|f| f.as_array_mut()) {
        files.retain(|f| {
            f.get(0)
                .and_then(|p| p.as_str())
                .is_some_and(|p| present.contains(&data.join(p.trim_start_matches('/'))))
        });
    }
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
    Ok(())
}

fn walk_files(dir: &Path) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p);
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn download_one_with_retries(
    conn_slot: &mut Option<KeepAliveConn>,
    host: &str,
    port: u16,
    token: Option<&str>,
    dest: &Path,
    rel: &str,
    expected_size: u64,
    copy_buf: &mut [u8],
    done: &AtomicU64,
    total_bytes: u64,
    t_start: &Instant,
    last_report_ms: &AtomicU64,
    progress: &(impl Fn(u64, u64) + Send + Sync),
) -> Result<()> {
    let final_path = dest.join(rel);
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if expected_size == 0 {
        File::create(&final_path)?;
        return Ok(());
    }

    let part_path = dest.join(format!("{rel}.part"));
    let mut attempt = 0u32;

    loop {
        if CANCEL_FLAG.load(Ordering::Relaxed) {
            bail!("Download dijeda");
        }
        attempt += 1;
        let existing_offset = std::fs::metadata(&part_path)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .unwrap_or(0);

        if existing_offset == expected_size {
            std::fs::rename(&part_path, &final_path)?;
            return Ok(());
        }
        let resume_from = if existing_offset < expected_size {
            existing_offset
        } else {
            let _ = std::fs::remove_file(&part_path);
            0
        };

        if conn_slot.is_none() {
            match KeepAliveConn::connect(host, port, token) {
                Ok(c) => *conn_slot = Some(c),
                Err(e) if attempt < 5 => {
                    std::thread::sleep(Duration::from_millis(250 * attempt as u64));
                    let _ = e;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        let conn = conn_slot.as_mut().unwrap();
        let res = conn.stream_file_to_part(
            rel,
            resume_from,
            expected_size,
            &part_path,
            copy_buf,
            |delta| {
                let cur = done.fetch_add(delta, Ordering::Relaxed) + delta;
                let now_ms = t_start.elapsed().as_millis() as u64;
                let prev_ms = last_report_ms.load(Ordering::Relaxed);
                if now_ms.saturating_sub(prev_ms) >= 120
                    && last_report_ms
                        .compare_exchange(prev_ms, now_ms, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    progress(cur.min(total_bytes), total_bytes);
                }
            },
        );

        match res {
            Ok(()) => {
                std::fs::rename(&part_path, &final_path)
                    .with_context(|| format!("Gagal menyimpan {}", final_path.display()))?;
                return Ok(());
            }
            Err(e) => {
                *conn_slot = None;
                let msg = format!("{e:#}");
                if msg.contains("401") || attempt >= 5 {
                    return Err(e.context(format!("Gagal mengunduh {rel}")));
                }
                std::thread::sleep(Duration::from_millis(300 * attempt as u64));
            }
        }
    }
}

struct KeepAliveConn {
    host_hdr: String,
    token: Option<String>,
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl KeepAliveConn {
    fn connect(host: &str, port: u16, token: Option<&str>) -> Result<Self> {
        let addr = (host, port)
            .to_socket_addrs()
            .with_context(|| format!("Tidak bisa resolve {host}:{port}"))?
            .next()
            .with_context(|| format!("Alamat {host}:{port} tidak ditemukan"))?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
            .with_context(|| format!("Gagal terhubung ke server sync {host}:{port}"))?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(20)));
        let writer = stream.try_clone()?;
        Ok(Self {
            host_hdr: format!("{host}:{port}"),
            token: token.map(String::from),
            reader: BufReader::with_capacity(64 << 10, stream),
            writer,
        })
    }

    fn fetch_bytes(&mut self, uri_path: &str) -> Result<Vec<u8>> {
        let mut req = format!(
            "GET {uri_path} HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\n",
            self.host_hdr
        );
        if let Some(t) = &self.token {
            req.push_str(&format!("X-RP-Sync-Token: {t}\r\n"));
        }
        req.push_str("\r\n");
        self.writer.write_all(req.as_bytes())?;
        self.writer.flush()?;

        let (status, len) = self.read_response_headers()?;
        if status == 401 {
            bail!("401 Unauthorized: Token sync salah atau kosong");
        }
        if status != 200 {
            bail!("HTTP {status} saat mengambil {uri_path}");
        }
        let mut body = vec![0u8; len as usize];
        self.reader.read_exact(&mut body)?;
        Ok(body)
    }

    fn stream_file_to_part(
        &mut self,
        rel: &str,
        resume_from: u64,
        expected_size: u64,
        part_path: &Path,
        buf: &mut [u8],
        mut on_bytes: impl FnMut(u64),
    ) -> Result<()> {
        let encoded = encode_uri_path(rel);
        let mut req = format!(
            "GET /files/{encoded} HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\n",
            self.host_hdr
        );
        if let Some(t) = &self.token {
            req.push_str(&format!("X-RP-Sync-Token: {t}\r\n"));
        }
        if resume_from > 0 {
            req.push_str(&format!("Range: bytes={resume_from}-\r\n"));
        }
        req.push_str("\r\n");
        self.writer.write_all(req.as_bytes())?;
        self.writer.flush()?;

        let (status, content_len) = self.read_response_headers()?;
        if status == 401 {
            bail!("401 Unauthorized: Token sync salah");
        }

        let mut file = if status == 206 && resume_from > 0 {
            OpenOptions::new().create(true).append(true).open(part_path)?
        } else if status == 200 {
            File::create(part_path)?
        } else {
            bail!("HTTP {status} pada {rel}");
        };

        let mut remaining = content_len;
        while remaining > 0 {
            if CANCEL_FLAG.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            let want = buf.len().min(remaining as usize);
            let n = self.reader.read(&mut buf[..want])?;
            if n == 0 {
                bail!("koneksi terputus di tengah file {rel}");
            }
            file.write_all(&buf[..n])?;
            remaining -= n as u64;
            on_bytes(n as u64);
        }
        file.flush()?;
        let final_len = file.metadata()?.len();
        if final_len != expected_size {
            bail!("ukuran file {rel} tidak sesuai ({final_len} != {expected_size})");
        }
        Ok(())
    }

    fn read_response_headers(&mut self) -> Result<(u16, u64)> {
        let mut status_line = String::new();
        let n = self.reader.read_line(&mut status_line)?;
        if n == 0 {
            bail!("server menutup koneksi");
        }
        let mut parts = status_line.split_whitespace();
        let _http = parts.next();
        let status: u16 = parts
            .next()
            .context("header status HTTP kosong")?
            .parse()
            .context("kode status HTTP tidak valid")?;

        let mut content_len: Option<u64> = None;
        loop {
            let mut line = String::new();
            let hn = self.reader.read_line(&mut line)?;
            if hn == 0 {
                bail!("EOF saat membaca header");
            }
            let trimmed = line.trim_end_matches(&['\r', '\n'][..]);
            if trimmed.is_empty() {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_len = v.trim().parse::<u64>().ok();
                }
            }
        }
        Ok((status, content_len.unwrap_or(0)))
    }
}

fn encode_uri_path(rel: &str) -> String {
    let mut out = String::with_capacity(rel.len() + 8);
    for &b in rel.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn test_parse_endpoint_maps_udp_port_to_sync_port() {
        assert_eq!(
            parse_endpoint("203.0.113.10").unwrap(),
            ("203.0.113.10".to_string(), 47521)
        );
        assert_eq!(
            parse_endpoint("203.0.113.10:47520").unwrap(),
            ("203.0.113.10".to_string(), 47521)
        );
        assert_eq!(
            parse_endpoint("http://127.0.0.1:19090").unwrap(),
            ("127.0.0.1".to_string(), 19090)
        );
    }

    #[test]
    fn test_zero_extract_sync_and_range_resume() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let city_bytes = b"LOS_SANTOS_MAP_CHUNK_DATA_0123456789".to_vec();
        let shader_bytes = b"{\"shaders\":[]}".to_vec();
        let manifest_bytes = br#"{"files":[["levels/gta5/city.rpf",36,0]]}"#.to_vec();

        let index_json = serde_json::json!({
            "version": 1,
            "total_bytes": city_bytes.len() + shader_bytes.len() + manifest_bytes.len(),
            "file_count": 3,
            "files": [
                {"path": "data/levels/gta5/city.rpf", "size": city_bytes.len()},
                {"path": "b/1/shaders/index.json", "size": shader_bytes.len()},
                {"path": "data/manifest.json", "size": manifest_bytes.len()}
            ]
        })
        .to_string()
        .into_bytes();

        let srv_handle = std::thread::spawn(move || {
            for incoming in listener.incoming().take(12) {
                let Ok(stream) = incoming else { break };
                let idx_c = index_json.clone();
                let city_c = city_bytes.clone();
                let sh_c = shader_bytes.clone();
                let mf_c = manifest_bytes.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut writer = stream;
                    loop {
                        let mut req = String::new();
                        if reader.read_line(&mut req).unwrap_or(0) == 0 || req.trim().is_empty() {
                            break;
                        }
                        let uri = req.split_whitespace().nth(1).unwrap_or("").to_string();
                        let mut tok = String::new();
                        let mut range_start = None::<usize>;
                        loop {
                            let mut h = String::new();
                            if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                                break;
                            }
                            if let Some((k, v)) = h.split_once(':') {
                                if k.trim().eq_ignore_ascii_case("x-rp-sync-token") {
                                    tok = v.trim().to_string();
                                } else if k.trim().eq_ignore_ascii_case("range") {
                                    if let Some(r) = v.trim().strip_prefix("bytes=") {
                                        range_start = r.trim_end_matches('-').parse().ok();
                                    }
                                }
                            }
                        }
                        if tok != "secret123" {
                            let _ = writer.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 4\r\n\r\nope");
                            continue;
                        }
                        let payload: &[u8] = match uri.as_str() {
                            "/sync-index.json" => &idx_c,
                            "/files/data/levels/gta5/city.rpf" => &city_c,
                            "/files/b/1/shaders/index.json" => &sh_c,
                            "/files/data/manifest.json" => &mf_c,
                            _ => {
                                let _ = writer.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                                continue;
                            }
                        };
                        if let Some(st) = range_start {
                            let slice = &payload[st..];
                            let hdr = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\n\r\n",
                                slice.len()
                            );
                            let _ = writer.write_all(hdr.as_bytes());
                            let _ = writer.write_all(slice);
                        } else {
                            let hdr = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                                payload.len()
                            );
                            let _ = writer.write_all(hdr.as_bytes());
                            let _ = writer.write_all(payload);
                        }
                        let _ = writer.flush();
                    }
                });
            }
        });

        let tmp = std::env::temp_dir().join(format!("rp_sync_test_{port}"));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("data/levels/gta5")).unwrap();
        // Pre-seed a partial `.part` file to verify HTTP 206 Range resume!
        std::fs::write(
            tmp.join("data/levels/gta5/city.rpf.part"),
            b"LOS_SANTOS_MAP_",
        )
        .unwrap();

        // Wrong token must fail with 401
        let bad = sync_game_data(
            &format!("127.0.0.1:{port}"),
            Some("wrong"),
            &tmp,
            false,
            false,
            |_, _| {},
        );
        assert!(bad.is_err());

        // Valid token resumes `.part` and commits `data/manifest.json`!
        sync_game_data(
            &format!("127.0.0.1:{port}"),
            Some("secret123"),
            &tmp,
            false,
            false,
            |_, _| {},
        )
        .unwrap();

        assert_eq!(
            std::fs::read(tmp.join("data/levels/gta5/city.rpf")).unwrap(),
            b"LOS_SANTOS_MAP_CHUNK_DATA_0123456789"
        );
        assert!(tmp.join("data/manifest.json").is_file());
        assert!(tmp.join("b/1/shaders/index.json").is_file());

        let _ = std::fs::remove_dir_all(&tmp);
        drop(srv_handle);
    }
}

