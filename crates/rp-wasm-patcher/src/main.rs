use anyhow::{bail, Context, Result};
use rp_wasm_patcher::{inspect_wasm_symbols, patch_game_wasm};
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--inspect") => {
            let input = args.get(2).context("Usage: rp-wasm-patcher --inspect <game.wasm>")?;
            let bytes = std::fs::read(input).with_context(|| format!("Failed to read {input}"))?;
            let report = inspect_wasm_symbols(&bytes)?;
            println!("=== Wasm Symbol Inspection Report ===");
            println!("Total named C++ functions : {}", report.total_named_functions);
            println!("__tls_base global index   : {:?}", report.tls_base_global);
            println!("Matched RP Commands       : {}", report.matched_commands.len());
            println!(
                "Total RAGE Command* found : {} (available for future RP expansion!)",
                report.discovered_rage_commands.len()
            );
            if args.get(3).map(String::as_str) == Some("--dump-all") {
                for (idx, sig) in &report.discovered_rage_commands {
                    println!("  [{idx}] {sig}");
                }
            }
            Ok(())
        }
        Some("--patch") => {
            let input = args
                .get(2)
                .context("Usage: rp-wasm-patcher --patch <in.wasm> <out.wasm>")?;
            let output = args
                .get(3)
                .context("Usage: rp-wasm-patcher --patch <in.wasm> <out.wasm>")?;
            if Path::new(input) == Path::new(output) {
                bail!("Input and output paths must be different");
            }
            let bytes = std::fs::read(input).with_context(|| format!("Failed to read {input}"))?;
            let patched = patch_game_wasm(&bytes, &[])?;
            std::fs::write(output, &patched)
                .with_context(|| format!("Failed to write {output}"))?;
            println!(
                "[rp-wasm-patcher] Successfully patched {} -> {} ({} bytes)",
                input,
                output,
                patched.len()
            );
            Ok(())
        }
        _ => {
            println!("RP Wasm Patcher & RAGE Command Inspector");
            println!("Usage:");
            println!("  rp-wasm-patcher --inspect <game.wasm> [--dump-all]");
            println!("  rp-wasm-patcher --patch <game.wasm> <game.patched.wasm>");
            Ok(())
        }
    }
}

