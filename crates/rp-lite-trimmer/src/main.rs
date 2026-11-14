use anyhow::{Context, Result};
use rp_lite_trimmer::{analyze_archive, repack_lite_archive_with_sidecar, unpack_sync_dir};
use std::fs::File;
use std::io::{BufReader, BufWriter};

fn fmt_gb(bytes: u64) -> String {
    format!("{:.2} GB ({:.0} MB)", bytes as f64 / 1e9, bytes as f64 / 1e6)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--analyze") => {
            let input = args
                .get(2)
                .context("Usage: rp-lite-trimmer --analyze <game.zip>")?;
            let file = File::open(input).with_context(|| format!("Cannot open {input}"))?;
            let report = analyze_archive(BufReader::new(file))?;
            println!("=== RP Lite Asset Analysis ===");
            println!("Original size : {}", fmt_gb(report.original_bytes));
            println!("Lite RP size  : {}", fmt_gb(report.kept_bytes));
            println!("Total saved   : {}", fmt_gb(report.saved_bytes));
            println!("Kept files    : {}", report.kept_files);
            println!("Stripped files: {}", report.stripped_files);
            println!("Breakdown by stripped category:");
            for (cat, bytes) in &report.saved_by_category {
                println!("  - {cat:<26}: {}", fmt_gb(*bytes));
            }
            println!("Breakdown of kept Lite RP folders:");
            for (folder, bytes) in &report.kept_by_top_folder {
                println!("  + {folder:<26}: {}", fmt_gb(*bytes));
            }
            Ok(())
        }
        Some("--repack") => {
            let input = args
                .get(2)
                .context("Usage: rp-lite-trimmer --repack <in.zip> <out-lite.zip> [--patch-wasm]")?;
            let output = args
                .get(3)
                .context("Usage: rp-lite-trimmer --repack <in.zip> <out-lite.zip> [--patch-wasm]")?;
            let patch_wasm = args.iter().any(|a| a == "--patch-wasm");

            let in_file = File::open(input).with_context(|| format!("Cannot open {input}"))?;
            let out_file =
                File::create(output).with_context(|| format!("Cannot create {output}"))?;
            let wasm_sidecar = std::path::Path::new(output)
                .parent()
                .map(|p| p.join("game.wasm"));

            println!(
                "[rp-lite-trimmer] Streaming {} -> {} (patch_wasm={})...",
                input, output, patch_wasm
            );
            let report = repack_lite_archive_with_sidecar(
                BufReader::with_capacity(4 << 20, in_file),
                BufWriter::with_capacity(4 << 20, out_file),
                patch_wasm,
                wasm_sidecar.as_deref(),
            )?;
            println!("=== RP Lite Repack Complete ===");
            println!("Original size : {}", fmt_gb(report.original_bytes));
            println!("Lite RP size  : {}", fmt_gb(report.kept_bytes));
            println!("Total saved   : {}", fmt_gb(report.saved_bytes));
            println!("Kept files    : {}", report.kept_files);
            println!("Stripped files: {}", report.stripped_files);
            println!("Wasm patched  : {}", report.wasm_patched);
            Ok(())
        }
        Some("--unpack-sync") => {
            let input = args
                .get(2)
                .context("Usage: rp-lite-trimmer --unpack-sync <lite.zip> <sync-dir>")?;
            let out_dir = args
                .get(3)
                .context("Usage: rp-lite-trimmer --unpack-sync <lite.zip> <sync-dir>")?;
            let in_file = File::open(input).with_context(|| format!("Cannot open {input}"))?;
            println!("[rp-lite-trimmer] Unpacking {} -> {} for zero-extract client sync...", input, out_dir);
            let idx = unpack_sync_dir(
                BufReader::with_capacity(4 << 20, in_file),
                std::path::Path::new(out_dir),
            )?;
            println!("=== Sync Directory Ready ===");
            println!("Files unpacked : {}", idx.file_count);
            println!("Total size     : {}", fmt_gb(idx.total_bytes));
            println!("Manifest index : {out_dir}/sync-index.json");
            Ok(())
        }
        _ => {
            println!("RP Lite Asset Trimmer (20 GB -> ~10-11 GB Sandbox/Roleplay Packager)");
            println!("Usage:");
            println!("  rp-lite-trimmer --analyze <input.zip>");
            println!("  rp-lite-trimmer --repack <input.zip> <output-lite.zip> [--patch-wasm]");
            println!("  rp-lite-trimmer --unpack-sync <lite.zip> <sync-dir>");
            Ok(())
        }
    }
}

