//! High-speed HTTP/1.1 Keep-Alive + Range Asset Sync Server for Zero-Extract Client Downloads.
//!
//! Serves only whitelisted files listed in `<sync_dir>/sync-index.json` over persistent TCP
//! connections (default port `47521`) with optional `RP_SYNC_TOKEN` authentication.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_SYNC_PORT: u16 = 47521;
const MAX_CONCURRENT_CONNS: usize = 64;

struct SyncState {
    root_dir: PathBuf,
    index_bytes: Vec<u8>,
    allowed_files: HashMap<String, u64>,
    required_token: Option<String>,
    active_conns: AtomicUsize,
}

pub fn spawn_if_configured() -> Result<()> {
    let Some(dir_str) = std::env::var("RP_SYNC_DIR").ok().filter(|s| !s.trim().is_empty()) else {
        return Ok(());
    };
    let root_dir = PathBuf::from(&dir_str);
    let index_path = root_dir.join("sync-index.json");
    if !index_path.is_file() {
        eprintln!(
            "[rp-sync] Skipping HTTP sync server: {} not found",
            index_path.display()
        );
        return Ok(());
    }

    let index_bytes = std::fs::read(&index_path)
        .with_context(|| format!("Failed reading {}", index_path.display()))?;
    let parsed: serde_json::Value =
        serde_json::from_slice(&index_bytes).context("Invalid JSON in sync-index.json")?;
    let mut allowed_files = HashMap::new();
    if let Some(arr) = parsed.get("files").and_then(|v| v.as_array()) {
        for item in arr {
            if let (Some(p), Some(sz)) = (
                item.get("path").and_then(|v| v.as_str()),
                item.get("size").and_then(|v| v.as_u64()),
            ) {
                allowed_files.insert(p.to_string(), sz);
            }
        }
    }

    let port = std::env::var("RP_SYNC_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(DEFAULT_SYNC_PORT);
    let required_token = std::env::var("RP_SYNC_TOKEN")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let listener = TcpListener::bind(("0.0.0.0", port))
        .with_context(|| format!("Failed binding HTTP sync port {port}"))?;
    println!(
        "[rp-sync] Zero-extract asset server listening on TCP 0.0.0.0:{} ({} files, auth={})",
        port,
        allowed_files.len(),
        if required_token.is_some() { "token" } else { "open" }
    );

    let state = Arc::new(SyncState {
        root_dir,
        index_bytes,
        allowed_files,
        required_token,
        active_conns: AtomicUsize::new(0),
    });

    std::thread::Builder::new()
        .name("rp-sync-accept".into())
        .spawn(move || {
            for incoming in listener.incoming() {
                let Ok(stream) = incoming else { continue };
                if state.active_conns.load(Ordering::Relaxed) >= MAX_CONCURRENT_CONNS {
                    let _ = send_simple_response(
                        &stream,
                        "503 Service Unavailable",
                        b"busy",
                        true,
                    );
                    continue;
                }
                state.active_conns.fetch_add(1, Ordering::Relaxed);
                let st = Arc::clone(&state);
                std::thread::spawn(move || {
                    let _ = handle_connection(stream, &st);
                    st.active_conns.fetch_sub(1, Ordering::Relaxed);
                });
            }
        })?;

    Ok(())
}

fn handle_connection(stream: TcpStream, state: &SyncState) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));

    let mut reader = BufReader::with_capacity(8192, stream.try_clone()?);
    let mut writer = stream;
    let mut line = String::new();
    let mut chunk_buf = vec![0u8; 256 << 10]; // 256 KiB streaming chunks

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break; // Clean EOF
        }
        let req_line = line.trim_end_matches(&['\r', '\n'][..]);
        if req_line.is_empty() {
            continue;
        }
        let mut parts = req_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let raw_target = parts.next().unwrap_or("");
        let http_ver = parts.next().unwrap_or("HTTP/1.1");

        let mut header_token: Option<String> = None;
        let mut range_header: Option<String> = None;
        let mut content_length: Option<u64> = None;
        let mut expect_continue = false;
        let mut close_conn = http_ver.eq_ignore_ascii_case("HTTP/1.0");

        loop {
            let mut hline = String::new();
            let hn = reader.read_line(&mut hline)?;
            if hn == 0 {
                return Ok(());
            }
            let trimmed = hline.trim_end_matches(&['\r', '\n'][..]);
            if trimmed.is_empty() {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                let k_low = k.trim().to_ascii_lowercase();
                let v_trim = v.trim();
                match k_low.as_str() {
                    "x-rp-sync-token" => header_token = Some(v_trim.to_string()),
                    "range" => range_header = Some(v_trim.to_string()),
                    "content-length" => content_length = v_trim.parse::<u64>().ok(),
                    "expect" => {
                        if v_trim.to_ascii_lowercase().contains("100-continue") {
                            expect_continue = true;
                        }
                    }
                    "connection" => {
                        if v_trim.to_ascii_lowercase().contains("close") {
                            close_conn = true;
                        } else if v_trim.to_ascii_lowercase().contains("keep-alive") {
                            close_conn = false;
                        }
                    }
                    _ => {}
                }
            }
        }

        if method != "GET" && method != "HEAD" && method != "PUT" {
            send_simple_response(&writer, "405 Method Not Allowed", b"method not allowed", true)?;
            break;
        }

        let (raw_path, query) = raw_target
            .split_once('?')
            .unwrap_or((raw_target, ""));

        if let Some(expected) = &state.required_token {
            let query_token = query.split('&').find_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                (k == "token").then(|| percent_decode(v))
            });
            let provided = header_token.as_deref().or(query_token.as_deref());
            if provided != Some(expected.as_str()) {
                send_simple_response(&writer, "401 Unauthorized", b"invalid sync token", true)?;
                break;
            }
        } else if method == "PUT" {
            send_simple_response(&writer, "403 Forbidden", b"token required for PUT", true)?;
            break;
        }

        let decoded_path = percent_decode(raw_path);
        if let Some(client_file) = decoded_path.strip_prefix("/client/") {
            let valid_name = !client_file.is_empty()
                && client_file.len() <= 64
                && client_file
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
                && !client_file.starts_with('.');
            if !valid_name {
                send_simple_response(&writer, "400 Bad Request", b"invalid filename", true)?;
                break;
            }
            let builds_dir = state
                .root_dir
                .parent()
                .unwrap_or(&state.root_dir)
                .join("client-builds");
            std::fs::create_dir_all(&builds_dir)?;
            let target_file = builds_dir.join(client_file);

            if method == "PUT" {
                let len = content_length.unwrap_or(0);
                if len == 0 || len > (1 << 30) {
                    send_simple_response(&writer, "400 Bad Request", b"bad content-length", true)?;
                    break;
                }
                if expect_continue {
                    writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
                    writer.flush()?;
                }
                let part = builds_dir.join(format!("{client_file}.part"));
                let mut f = File::create(&part)?;
                let mut rem = len;
                while rem > 0 {
                    let cap = chunk_buf.len().min(rem as usize);
                    let n = reader.read(&mut chunk_buf[..cap])?;
                    if n == 0 {
                        return Ok(());
                    }
                    f.write_all(&chunk_buf[..n])?;
                    rem -= n as u64;
                }
                f.flush()?;
                std::fs::rename(&part, &target_file)?;
                send_simple_response(&writer, "201 Created", b"uploaded\n", close_conn)?;
                if close_conn {
                    break;
                }
                continue;
            }

            let Ok(mut f) = File::open(&target_file) else {
                send_simple_response(&writer, "404 Not Found", b"build not found", close_conn)?;
                if close_conn {
                    break;
                }
                continue;
            };
            let fsize = f.metadata()?.len();
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"{client_file}\"\r\nContent-Length: {fsize}\r\nConnection: {}\r\n\r\n",
                if close_conn { "close" } else { "keep-alive" }
            );
            writer.write_all(hdr.as_bytes())?;
            if method == "GET" {
                let mut rem = fsize;
                while rem > 0 {
                    let cap = chunk_buf.len().min(rem as usize);
                    let n = f.read(&mut chunk_buf[..cap])?;
                    if n == 0 {
                        break;
                    }
                    writer.write_all(&chunk_buf[..n])?;
                    rem -= n as u64;
                }
            }
            writer.flush()?;
            if close_conn {
                break;
            }
            continue;
        }
        if decoded_path == "/sync-index.json" {
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n",
                state.index_bytes.len(),
                if close_conn { "close" } else { "keep-alive" }
            );
            writer.write_all(hdr.as_bytes())?;
            if method == "GET" {
                writer.write_all(&state.index_bytes)?;
            }
            writer.flush()?;
            if close_conn {
                break;
            }
            continue;
        }

        let rel = decoded_path
            .strip_prefix("/files/")
            .or_else(|| decoded_path.strip_prefix('/'))
            .unwrap_or("");
        let Some(&expected_size) = state.allowed_files.get(rel) else {
            send_simple_response(&writer, "404 Not Found", b"not found", close_conn)?;
            if close_conn {
                break;
            }
            continue;
        };

        let disk_path: PathBuf = state.root_dir.join(rel);
        let Ok(mut file) = File::open(&disk_path) else {
            send_simple_response(&writer, "404 Not Found", b"missing file", close_conn)?;
            if close_conn {
                break;
            }
            continue;
        };

        let (status, start, end) = if let Some(r) = range_header.as_deref() {
            if let Some((s, e)) = parse_range(r, expected_size) {
                ("206 Partial Content", s, e)
            } else {
                send_simple_response(&writer, "416 Range Not Satisfiable", b"bad range", close_conn)?;
                if close_conn {
                    break;
                }
                continue;
            }
        } else if expected_size == 0 {
            ("200 OK", 0, 0)
        } else {
            ("200 OK", 0, expected_size - 1)
        };

        let send_len = if expected_size == 0 {
            0
        } else {
            end - start + 1
        };

        let mut hdr = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nContent-Length: {send_len}\r\nConnection: {}\r\n",
            if close_conn { "close" } else { "keep-alive" }
        );
        if status.starts_with("206") && expected_size > 0 {
            hdr.push_str(&format!(
                "Content-Range: bytes {start}-{end}/{expected_size}\r\n"
            ));
        }
        hdr.push_str("\r\n");
        writer.write_all(hdr.as_bytes())?;

        if method == "GET" && send_len > 0 {
            if start > 0 {
                file.seek(SeekFrom::Start(start))?;
            }
            let mut remaining = send_len;
            while remaining > 0 {
                let to_read = chunk_buf.len().min(remaining as usize);
                let got = file.read(&mut chunk_buf[..to_read])?;
                if got == 0 {
                    return Ok(());
                }
                writer.write_all(&chunk_buf[..got])?;
                remaining -= got as u64;
            }
        }
        writer.flush()?;
        if close_conn {
            break;
        }
    }
    Ok(())
}

fn send_simple_response(
    mut stream: &TcpStream,
    status: &str,
    body: &[u8],
    close: bool,
) -> std::io::Result<()> {
    let hdr = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n",
        body.len(),
        if close { "close" } else { "keep-alive" }
    );
    stream.write_all(hdr.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn parse_range(header: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let spec = header.trim().strip_prefix("bytes=")?;
    let (start_s, end_s) = spec.split_once('-')?;
    let start: u64 = start_s.trim().parse().ok()?;
    if start >= total {
        return None;
    }
    let end = if end_s.trim().is_empty() {
        total - 1
    } else {
        end_s.trim().parse::<u64>().ok()?.min(total - 1)
    };
    (start <= end).then_some((start, end))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

