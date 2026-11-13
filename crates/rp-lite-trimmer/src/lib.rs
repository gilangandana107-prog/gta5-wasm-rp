//! Streaming ZIP64 Sandbox/Roleplay Asset Trimmer.
//!
//! Reads a Wasm-layout archive (`data/manifest.json` + `b/<build>/game.wasm`), strips out
//! single-player-only bloat (licensed radio tracks, story dialogue banks, Bink `.bik` videos,
//! mission cutscene animations, and North Yankton prologue snow assets), and writes a clean
//! ZIP64 archive (~10-11.5 GB) with a synchronized `data/manifest.json` and `dlcList.xml`.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Seek, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const SKIPPED_DLC_TAGS: &[&str] = &["env_test", "verityradio"];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TrimReport {
    pub original_bytes: u64,
    pub kept_bytes: u64,
    pub saved_bytes: u64,
    pub kept_files: usize,
    pub stripped_files: usize,
    pub saved_by_category: BTreeMap<String, u64>,
    pub kept_by_top_folder: BTreeMap<String, u64>,
    pub wasm_patched: bool,
}

/// Determines whether a `data/<rel_path>` file is unnecessary for Los Santos Sandbox/Roleplay.
/// Returns `Some(category_label)` if safe to strip, or `None` if essential (map, vehicles, weapons, core SFX).
pub fn classify_strippable_data_file(rel_data_path: &str) -> Option<&'static str> {
    let p = rel_data_path.trim_start_matches('/').to_ascii_lowercase();

    // 1. Optional test / custom radio DLC packs
    if SKIPPED_DLC_TAGS.iter().any(|tag| p.contains(tag)) {
        return Some("optional_dlc_radio");
    }

    // 2. Offline Bink video files (.bik: fake TV cartoons, cinema, Rockstar intro logos)
    // Safe because `CMovieMgr::PreloadMovie` is patched to return 0!
    if p.starts_with("movies/") || p.ends_with(".bik") {
        return Some("offline_bink_movies");
    }

    // 3. Audio banks: strip licensed Radio & Singleplayer Story Dialogue, KEEP engines/guns/sirens/ambience!
    if let Some(sfx_rest) = p.strip_prefix("sfx/") {
        let file_stem = sfx_rest.rsplit('/').next().unwrap_or(sfx_rest);
        if file_stem.starts_with("radio_") || sfx_rest.contains("radio") {
            return Some("radio_music_stations");
        }
        if file_stem.starts_with("ss_")
            || file_stem.starts_with("s_full_")
            || file_stem.starts_with("cutscene")
            || file_stem.starts_with("prologue")
        {
            return Some("story_mission_dialogue");
        }
    }

    // 4. Story-mode cutscene animation dictionaries
    if p.starts_with("anim/cutscene") || p.contains("/cutscene/") {
        return Some("story_cutscene_anims");
    }

    // 5. North Yankton snow prologue map (only used in Story Mission #1)
    if p.contains("/prologue/") || p.contains("prologue_") || p.ends_with("prologue.rpf") {
        return Some("north_yankton_prologue");
    }

    None
}

fn find_archive_root_prefix<R: Read + Seek>(zip: &ZipArchive<R>) -> Result<String> {
    let manifest = zip
        .file_names()
        .filter(|n| {
            let norm = n.replace('\\', "/");
            norm == "data/manifest.json" || norm.ends_with("/data/manifest.json")
        })
        .min_by_key(|n| n.len())
        .context("Archive does not contain data/manifest.json (not a valid Wasm game archive)")?;

    let norm = manifest.replace('\\', "/");
    Ok(norm[..norm.len() - "data/manifest.json".len()].to_string())
}

/// Fast header-only analysis (<1 second, zero extraction!) to preview exact GB savings.
pub fn analyze_archive<R: Read + Seek>(reader: R) -> Result<TrimReport> {
    let mut zip = ZipArchive::new(reader).context("Invalid ZIP archive")?;
    let prefix = find_archive_root_prefix(&zip)?;
    let mut report = TrimReport::default();

    for idx in 0..zip.len() {
        let entry = zip.by_index_raw(idx)?;
        if entry.is_dir() {
            continue;
        }
        let full_name = entry.name().replace('\\', "/");
        let Some(rel) = full_name.strip_prefix(&prefix) else {
            continue;
        };
        let size = entry.size();
        report.original_bytes += size;

        if let Some(data_rel) = rel.strip_prefix("data/") {
            if let Some(cat) = classify_strippable_data_file(data_rel) {
                report.stripped_files += 1;
                report.saved_bytes += size;
                *report.saved_by_category.entry(cat.to_string()).or_insert(0) += size;
                continue;
            }
            let top = data_rel.split('/').next().unwrap_or("root");
            *report
                .kept_by_top_folder
                .entry(format!("data/{top}"))
                .or_insert(0) += size;
            report.kept_files += 1;
            report.kept_bytes += size;
            continue;
        }

        if let Some(build_rel) = rel.strip_prefix("b/") {
            let inner = build_rel.split_once('/').map(|(_, r)| r).unwrap_or("");
            if inner == "game.wasm" || inner.starts_with("shaders/") {
                *report
                    .kept_by_top_folder
                    .entry("b/<build>".to_string())
                    .or_insert(0) += size;
                report.kept_files += 1;
                report.kept_bytes += size;
                continue;
            }
        }

        report.stripped_files += 1;
        report.saved_bytes += size;
        *report
            .saved_by_category
            .entry("non_game_extras".to_string())
            .or_insert(0) += size;
    }

    Ok(report)
}

/// Streams input `.zip` directly into a clean ZIP64 Lite `.zip` without extracting 20 GB to disk!
pub fn repack_lite_archive<R: Read + Seek, W: Write + Seek>(
    reader: R,
    writer: W,
    patch_wasm: bool,
) -> Result<TrimReport> {
    let mut zip_in = ZipArchive::new(reader).context("Invalid input ZIP archive")?;
    let prefix = find_archive_root_prefix(&zip_in)?;

    let has_shaders = zip_in.file_names().any(|n| {
        let norm = n.replace('\\', "/");
        norm.starts_with(&format!("{prefix}b/")) && norm.ends_with("/shaders/index.json")
    });
    if !has_shaders {
        bail!("Archive is missing b/<build>/shaders/index.json");
    }

    let mut zip_out = ZipWriter::new(writer);
    let opts_stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true);
    let opts_deflated = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(true);

    let mut report = TrimReport::default();
    let mut kept_data_paths: HashSet<String> = HashSet::new();
    let mut deferred_manifest_json: Option<Vec<u8>> = None;
    let mut copy_buf = vec![0u8; 1 << 20]; // 1 MiB streaming buffer

    for idx in 0..zip_in.len() {
        let mut entry = zip_in.by_index(idx)?;
        if entry.is_dir() {
            continue;
        }
        let full_name = entry.name().replace('\\', "/");
        let Some(rel) = full_name.strip_prefix(&prefix).map(str::to_string) else {
            continue;
        };
        let size = entry.size();
        report.original_bytes += size;

        // Hold `data/manifest.json` until the end so we prune all stripped file entries cleanly
        if rel == "data/manifest.json" {
            let mut raw = Vec::with_capacity(size as usize);
            entry.read_to_end(&mut raw)?;
            deferred_manifest_json = Some(raw);
            continue;
        }

        if let Some(data_rel) = rel.strip_prefix("data/") {
            if let Some(cat) = classify_strippable_data_file(data_rel) {
                report.stripped_files += 1;
                report.saved_bytes += size;
                *report.saved_by_category.entry(cat.to_string()).or_insert(0) += size;
                continue;
            }

            // Clean `common/data/dlcList.xml` of skipped radio/test DLCs
            if data_rel.eq_ignore_ascii_case("common/data/dlclist.xml") {
                let mut xml = String::new();
                entry.read_to_string(&mut xml)?;
                let filtered = xml
                    .lines()
                    .filter(|line| {
                        let lower = line.to_ascii_lowercase();
                        !SKIPPED_DLC_TAGS.iter().any(|tag| lower.contains(tag))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
                zip_out.start_file(&rel, opts_deflated)?;
                zip_out.write_all(filtered.as_bytes())?;
                kept_data_paths.insert(data_rel.to_string());
                report.kept_files += 1;
                report.kept_bytes += filtered.len() as u64;
                continue;
            }

            zip_out.start_file(&rel, opts_stored)?;
            let written = stream_copy(&mut entry, &mut zip_out, &mut copy_buf)?;
            kept_data_paths.insert(data_rel.to_string());
            report.kept_files += 1;
            report.kept_bytes += written;
            continue;
        }

        if let Some(build_rel) = rel.strip_prefix("b/") {
            let inner = build_rel.split_once('/').map(|(_, r)| r).unwrap_or("");
            if inner == "game.wasm" {
                let mut wasm_bytes = Vec::with_capacity(size as usize);
                entry.read_to_end(&mut wasm_bytes)?;
                let final_wasm = if patch_wasm && !rp_wasm_patcher::is_already_patched(&wasm_bytes)
                {
                    report.wasm_patched = true;
                    rp_wasm_patcher::patch_game_wasm(&wasm_bytes, &[])?
                } else {
                    wasm_bytes
                };
                zip_out.start_file(&rel, opts_deflated)?;
                zip_out.write_all(&final_wasm)?;
                report.kept_files += 1;
                report.kept_bytes += final_wasm.len() as u64;
                continue;
            } else if inner.starts_with("shaders/") {
                zip_out.start_file(&rel, opts_stored)?;
                let written = stream_copy(&mut entry, &mut zip_out, &mut copy_buf)?;
                report.kept_files += 1;
                report.kept_bytes += written;
                continue;
            }
        }

        // Strip browser JS workers / unrelated wrapper files
        report.stripped_files += 1;
        report.saved_bytes += size;
        *report
            .saved_by_category
            .entry("non_game_extras".to_string())
            .or_insert(0) += size;
    }

    // Rewrite cleaned `data/manifest.json` matching only kept `data/*` files
    let raw_manifest = deferred_manifest_json.context("Missing data/manifest.json")?;
    let mut manifest_val: serde_json::Value =
        serde_json::from_slice(&raw_manifest).context("Malformed data/manifest.json")?;
    if let Some(files_arr) = manifest_val.get_mut("files").and_then(|v| v.as_array_mut()) {
        files_arr.retain(|item| {
            item.get(0)
                .and_then(|p| p.as_str())
                .is_some_and(|p| kept_data_paths.contains(p.trim_start_matches('/')))
        });
    }
    let cleaned_manifest = serde_json::to_vec(&manifest_val)?;
    zip_out.start_file("data/manifest.json", opts_deflated)?;
    zip_out.write_all(&cleaned_manifest)?;
    report.kept_files += 1;
    report.kept_bytes += cleaned_manifest.len() as u64;

    zip_out.finish()?;
    Ok(report)
}

fn stream_copy<R: Read, W: Write>(src: &mut R, dst: &mut W, buf: &mut [u8]) -> Result<u64> {
    let mut total = 0u64;
    loop {
        let n = src.read(buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
        total += n as u64;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_lite_repacker_strips_bloat_and_updates_manifest() {
        let mut mock_zip_buf = Cursor::new(Vec::new());
        {
            let mut zw = ZipWriter::new(&mut mock_zip_buf);
            let opts = SimpleFileOptions::default();

            let manifest = serde_json::json!({
                "files": [
                    ["levels/gta5/city.rpf", 10, 0],
                    ["sfx/vehicles.rpf", 10, 0],
                    ["sfx/radio_01_class_rock.rpf", 100, 0],
                    ["sfx/ss_michael.rpf", 80, 0],
                    ["movies/tv_cartoon.bik", 60, 0],
                    ["levels/gta5/prologue/snow.rpf", 50, 0]
                ]
            });
            zw.start_file("WrapperFolder/data/manifest.json", opts).unwrap();
            zw.write_all(manifest.to_string().as_bytes()).unwrap();

            zw.start_file("WrapperFolder/data/common/data/dlcList.xml", opts)
                .unwrap();
            zw.write_all(b"<Paths>\n<Item>dlcpacks:/mpheist/</Item>\n<Item>dlcpacks:/verityradio/</Item>\n</Paths>\n")
                .unwrap();

            for (p, len) in [
                ("WrapperFolder/data/levels/gta5/city.rpf", 10usize),
                ("WrapperFolder/data/sfx/vehicles.rpf", 10),
                ("WrapperFolder/data/sfx/radio_01_class_rock.rpf", 100),
                ("WrapperFolder/data/sfx/ss_michael.rpf", 80),
                ("WrapperFolder/data/movies/tv_cartoon.bik", 60),
                ("WrapperFolder/data/levels/gta5/prologue/snow.rpf", 50),
                ("WrapperFolder/b/1/shaders/index.json", 5),
            ] {
                zw.start_file(p, opts).unwrap();
                zw.write_all(&vec![0xAB; len]).unwrap();
            }
            zw.finish().unwrap();
        }

        mock_zip_buf.set_position(0);
        let mut out_zip_buf = Cursor::new(Vec::new());
        let report = repack_lite_archive(&mut mock_zip_buf, &mut out_zip_buf, false).unwrap();

        assert_eq!(report.stripped_files, 4); // radio, ss_michael, .bik, prologue
        assert!(report.saved_bytes >= 290);

        out_zip_buf.set_position(0);
        let mut result_zip = ZipArchive::new(out_zip_buf).unwrap();
        let mut mf_str = String::new();
        result_zip
            .by_name("data/manifest.json")
            .unwrap()
            .read_to_string(&mut mf_str)
            .unwrap();
        let mf_json: serde_json::Value = serde_json::from_str(&mf_str).unwrap();
        let remaining = mf_json["files"].as_array().unwrap();
        assert_eq!(remaining.len(), 2); // Only city.rpf & vehicles.rpf remain!
    }
}

