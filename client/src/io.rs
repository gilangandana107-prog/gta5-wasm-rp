// muguet - io.rs
// Constantin Clerc (c22dev) - 0.1

use crate::mem_base_len;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::LazyLock;
use std::time::Duration;
use wasmtime::SharedMemory;

static STARTED: AtomicBool = AtomicBool::new(false);
const MAX_OPEN: usize = 1024;

const VIRTUAL_STUB_RPFS: &[&str] = &[
    // Stripped x64/audio/sfx/*.rpf packs
    "x64/audio/sfx/CUTSCENE_MASTERED_ONLY.rpf",
    "x64/audio/sfx/POLICE_SCANNER.rpf",
    "x64/audio/sfx/PROLOGUE.rpf",
    "x64/audio/sfx/RADIO_01_CLASS_ROCK.rpf",
    "x64/audio/sfx/RADIO_02_POP.rpf",
    "x64/audio/sfx/RADIO_03_HIPHOP_NEW.rpf",
    "x64/audio/sfx/RADIO_04_PUNK.rpf",
    "x64/audio/sfx/RADIO_05_TALK_01.rpf",
    "x64/audio/sfx/RADIO_06_COUNTRY.rpf",
    "x64/audio/sfx/RADIO_07_DANCE_01.rpf",
    "x64/audio/sfx/RADIO_08_MEXICAN.rpf",
    "x64/audio/sfx/RADIO_09_HIPHOP_OLD.rpf",
    "x64/audio/sfx/RADIO_11_TALK_02.rpf",
    "x64/audio/sfx/RADIO_12_REGGAE.rpf",
    "x64/audio/sfx/RADIO_13_JAZZ.rpf",
    "x64/audio/sfx/RADIO_14_DANCE_02.rpf",
    "x64/audio/sfx/RADIO_15_MOTOWN.rpf",
    "x64/audio/sfx/RADIO_16_SILVERLAKE.rpf",
    "x64/audio/sfx/RADIO_17_FUNK.rpf",
    "x64/audio/sfx/RADIO_18_90S_ROCK.rpf",
    "x64/audio/sfx/RADIO_19_USER.rpf",
    "x64/audio/sfx/RADIO_ADVERTS.rpf",
    "x64/audio/sfx/RADIO_NEWS.rpf",
    "x64/audio/sfx/SS_AC.rpf",
    "x64/audio/sfx/SS_DE.rpf",
    "x64/audio/sfx/SS_FF.rpf",
    "x64/audio/sfx/SS_GM.rpf",
    "x64/audio/sfx/SS_NP.rpf",
    "x64/audio/sfx/SS_QR.rpf",
    "x64/audio/sfx/SS_ST.rpf",
    "x64/audio/sfx/SS_UZ.rpf",
    "x64/audio/sfx/S_FULL_AMB_F.rpf",
    "x64/audio/sfx/S_FULL_AMB_M.rpf",
    "x64/audio/sfx/S_FULL_GAN.rpf",
    "x64/audio/sfx/S_FULL_SER.rpf",
    // Stripped cutscene & prologue RPF archives referenced in common/*.meta & images.meta
    "x64/anim/cutscene/cuts_ah_1.rpf",
    "x64/anim/cutscene/cuts_ah_2.rpf",
    "x64/anim/cutscene/cuts_ah_3a.rpf",
    "x64/anim/cutscene/cuts_ah_3b.rpf",
    "x64/anim/cutscene/cuts_armenian_1.rpf",
    "x64/anim/cutscene/cuts_armenian_2.rpf",
    "x64/anim/cutscene/cuts_armenian_3.rpf",
    "x64/anim/cutscene/cuts_bs_1.rpf",
    "x64/anim/cutscene/cuts_bs_2a.rpf",
    "x64/anim/cutscene/cuts_bs_2b.rpf",
    "x64/anim/cutscene/cuts_bss_1.rpf",
    "x64/anim/cutscene/cuts_car_steal_1.rpf",
    "x64/anim/cutscene/cuts_car_steal_2.rpf",
    "x64/anim/cutscene/cuts_car_steal_3.rpf",
    "x64/anim/cutscene/cuts_car_steal_4.rpf",
    "x64/anim/cutscene/cuts_car_steal_5.rpf",
    "x64/anim/cutscene/cuts_chinese_1.rpf",
    "x64/anim/cutscene/cuts_chinese_2.rpf",
    "x64/anim/cutscene/cuts_choice.rpf",
    "x64/anim/cutscene/cuts_drf.rpf",
    "x64/anim/cutscene/cuts_exile_1.rpf",
    "x64/anim/cutscene/cuts_exile_2.rpf",
    "x64/anim/cutscene/cuts_exile_3.rpf",
    "x64/anim/cutscene/cuts_fame_or_shame.rpf",
    "x64/anim/cutscene/cuts_family_1.rpf",
    "x64/anim/cutscene/cuts_family_2.rpf",
    "x64/anim/cutscene/cuts_family_3.rpf",
    "x64/anim/cutscene/cuts_family_4.rpf",
    "x64/anim/cutscene/cuts_family_5.rpf",
    "x64/anim/cutscene/cuts_family_6.rpf",
    "x64/anim/cutscene/cuts_fbi_1.rpf",
    "x64/anim/cutscene/cuts_fbi_2.rpf",
    "x64/anim/cutscene/cuts_fbi_3.rpf",
    "x64/anim/cutscene/cuts_fbi_4.rpf",
    "x64/anim/cutscene/cuts_fbi_5.rpf",
    "x64/anim/cutscene/cuts_fin_c_ext.rpf",
    "x64/anim/cutscene/cuts_finale_1a.rpf",
    "x64/anim/cutscene/cuts_finale_1c.rpf",
    "x64/anim/cutscene/cuts_finale_2.rpf",
    "x64/anim/cutscene/cuts_finale_c2.rpf",
    "x64/anim/cutscene/cuts_finale_ext.rpf",
    "x64/anim/cutscene/cuts_franklin_0.rpf",
    "x64/anim/cutscene/cuts_franklin_1.rpf",
    "x64/anim/cutscene/cuts_franklin_2.rpf",
    "x64/anim/cutscene/cuts_jewellery_heist_1.rpf",
    "x64/anim/cutscene/cuts_jewellery_heist_2_arrest_fail.rpf",
    "x64/anim/cutscene/cuts_jewellery_heist_2_finale.rpf",
    "x64/anim/cutscene/cuts_jewellery_heist_2a.rpf",
    "x64/anim/cutscene/cuts_jewellery_heist_2b.rpf",
    "x64/anim/cutscene/cuts_josh_1.rpf",
    "x64/anim/cutscene/cuts_josh_2.rpf",
    "x64/anim/cutscene/cuts_josh_3.rpf",
    "x64/anim/cutscene/cuts_josh_4.rpf",
    "x64/anim/cutscene/cuts_lamar_1.rpf",
    "x64/anim/cutscene/cuts_lester_1.rpf",
    "x64/anim/cutscene/cuts_lsdh_1.rpf",
    "x64/anim/cutscene/cuts_lsdh_2.rpf",
    "x64/anim/cutscene/cuts_lsdhs.rpf",
    "x64/anim/cutscene/cuts_martin_1.rpf",
    "x64/anim/cutscene/cuts_michael_1.rpf",
    "x64/anim/cutscene/cuts_michael_2.rpf",
    "x64/anim/cutscene/cuts_michael_3.rpf",
    "x64/anim/cutscene/cuts_mp.rpf",
    "x64/anim/cutscene/cuts_oddjobs.rpf",
    "x64/anim/cutscene/cuts_prologue.rpf",
    "x64/anim/cutscene/cuts_rampages.rpf",
    "x64/anim/cutscene/cuts_random_peds.rpf",
    "x64/anim/cutscene/cuts_rural_bank_heist_2.rpf",
    "x64/anim/cutscene/cuts_rural_bank_heist_setup.rpf",
    "x64/anim/cutscene/cuts_solomon_1.rpf",
    "x64/anim/cutscene/cuts_solomon_2.rpf",
    "x64/anim/cutscene/cuts_solomon_3.rpf",
    "x64/anim/cutscene/cuts_solomon_5.rpf",
    "x64/anim/cutscene/cuts_test.rpf",
    "x64/anim/cutscene/cuts_trevor_1.rpf",
    "x64/anim/cutscene/cuts_trevor_2.rpf",
    "x64/anim/cutscene/cuts_trevor_5.rpf",
    "x64/anim/cutscene/cuts_trevor_drive.rpf",
    "x64/anim/cutscene/cuts_tv.rpf",
    "x64/levels/gta5/destruction/interior/des_prologue_door.rpf",
    "x64/levels/gta5/vehicles_mp.rpf",
    "x64/levels/gta5/_prologue/prologue.rpf",
    "x64/levels/gta5/_prologue/prologue_metadata.rpf",
    "x64/levels/gta5/props/prologue/v_prologue.rpf",
];

pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("MUGUET_DATA").unwrap_or_default())
}

pub fn augmented_manifest_bytes() -> Option<&'static [u8]> {
    static MANIFEST: LazyLock<Option<Vec<u8>>> = LazyLock::new(|| {
        let root = data_dir();
        let raw = std::fs::read(root.join("manifest.json")).ok()?;
        let mut v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
        let stub_size = std::fs::metadata(root.join("x64/anim/filters.rpf"))
            .map(|m| m.len())
            .unwrap_or(4096);
        if let Some(files) = v.get_mut("files").and_then(|f| f.as_array_mut()) {
            let mut present: HashSet<String> = files
                .iter()
                .filter_map(|item| item.get(0).and_then(|p| p.as_str()).map(str::to_string))
                .collect();
            let mut added = 0usize;
            for &rpf in VIRTUAL_STUB_RPFS {
                if present.insert(rpf.to_string()) {
                    files.push(serde_json::json!([rpf, stub_size, 1790644442u64]));
                    added += 1;
                }
                let lower = rpf.to_ascii_lowercase();
                if present.insert(lower.clone()) {
                    files.push(serde_json::json!([lower, stub_size, 1790644442u64]));
                    added += 1;
                }
            }
            if added > 0 {
                eprintln!("[io] injected {added} virtual 4KB RPF7 stub entries into manifest.json");
            }
        }
        serde_json::to_vec(&v).ok().or(Some(raw))
    });
    MANIFEST.as_deref()
}

fn manifest_files() -> Vec<(String, u64)> {
    let bytes = augmented_manifest_bytes().expect("manifest.json");
    let v: serde_json::Value = serde_json::from_slice(bytes).expect("manifest json");
    v["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| (f[0].as_str().unwrap_or("").to_string(), f[1].as_u64().unwrap_or(0)))
        .collect()
}

fn atomic_i32(mem: &SharedMemory, addr: u64) -> &AtomicI32 {
    let (base, len) = mem_base_len(mem);
    assert!((addr as usize) + 4 <= len && addr % 4 == 0);
    unsafe { &*(base.add(addr as usize) as *const AtomicI32) }
}

pub fn start(mem: &SharedMemory, table: u64) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let mem = mem.clone();
    std::thread::Builder::new()
        .name("io".into())
        .spawn(move || serve_loop(mem, table))
        .expect("spawn io thread");
    eprintln!("[io] native I/O thread started (table=0x{table:x})");
}

fn serve_loop(mem: SharedMemory, table: u64) {
    let files = manifest_files();
    let root = data_dir();
    let mut handles: HashMap<i32, File> = HashMap::new();
    let mut served: u64 = 0;
    let mut bytes: u64 = 0;
    loop {
        let seen = atomic_i32(&mem, table).load(Ordering::SeqCst);
        let n = atomic_i32(&mem, table + 4).load(Ordering::SeqCst).max(0) as u64;
        for slot in 0..n {
            let w = table + 64 + 64 * slot;
            if atomic_i32(&mem, w).compare_exchange(1, 3, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                continue;
            }
            let word = |k: u64| atomic_i32(&mem, w + 4 * k).load(Ordering::SeqCst) as u32 as u64;
            let id = word(1) as i32;
            let off = word(2) | (word(3) << 32);
            let dst = word(4) | (word(5) << 32);
            let mut len = word(6);
            let result: i64 = match files.get(id as usize) {
                None => -1,
                Some((path, size)) => {
                    if off >= *size {
                        0
                    } else {
                        len = len.min(size - off);

                        if !handles.contains_key(&id) {
                            if handles.len() >= MAX_OPEN { handles.clear(); }
                            let opened = File::open(root.join(path)).or_else(|e| {
                                if path.to_ascii_lowercase().ends_with(".rpf") {
                                    File::open(root.join("x64/anim/filters.rpf"))
                                } else {
                                    Err(e)
                                }
                            });
                            match opened {
                                Ok(f) => { handles.insert(id, f); }
                                Err(e) => eprintln!("[io] open {path}: {e}"),
                            }
                        }
                        let (base, mlen) = mem_base_len(&mem);
                        let Some(f) = handles.get(&id) else {
                            atomic_i32(&mem, w + 28).store(-1, Ordering::SeqCst);
                            atomic_i32(&mem, w).store(2, Ordering::SeqCst);
                            let _ = mem.atomic_notify(w, 1);
                            continue;
                        };
                        if (dst + len) as usize > mlen {
                            -1
                        } else {
                            crate::heapcheck::note(dst, len, "file read");
                            let buf = unsafe { std::slice::from_raw_parts_mut(base.add(dst as usize), len as usize) };
                            match f.read_exact_at(buf, off) {
                                Ok(()) => len as i64,
                                Err(e) => { eprintln!("[io] read {path}@{off}+{len}: {e}"); -1 }
                            }
                        }
                    }
                }
            };
            served += 1;
            if result > 0 { bytes += result as u64; }
            if served % 2000 == 0 {
                eprintln!("[io] {served} reads, {} MB", bytes >> 20);
            }
            atomic_i32(&mem, w + 28).store(result as i32, Ordering::SeqCst);
            atomic_i32(&mem, w).store(2, Ordering::SeqCst);
            let _ = mem.atomic_notify(w, 1);
        }

        let _ = mem.atomic_wait32(table, seen as u32, Some(Duration::from_millis(20)));
    }
}
