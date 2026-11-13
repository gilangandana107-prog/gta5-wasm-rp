# GTA5-Wasm-RP — Cross-Platform Roleplay Framework (iOS · Android · PC)

Kerangka kerja *clean-room* open-source ala **FiveM** untuk menjalankan server & client Roleplay lintas platform (**iOS Metal**, **Android Vulkan**, dan **PC/macOS**) di atas arsitektur WebAssembly (`game.wasm`), lengkap dengan **JavaScript Server Scripting (ES2023+)** dan **Cloud Build GitHub Actions**.

> **Status Proyek:** Arsip Eksperimen / Fondasi Tahap 1 (MVP).
>
> **Catatan Perangkat Keras:** Karena aset asli GTA V membutuhkan ruang penyimpanan ~60–100 GB dan RAM perangkat minimal **8 GB** (misal iPhone 15 Pro ke atas / iPad M-series), repositori ini disimpan sebagai fondasi referensi sambil pengembangan aktif dialihkan terlebih dahulu ke portingan game yang jauh lebih ringan untuk perangkat penyimpanan 128 GB / RAM 6 GB.

> **Peringatan Legal / Hak Cipta:** Repositori ini **100% murni kode jaringan, runtime JavaScript, dan alat patcher mandiri**. Repositori ini **tidak menyertakan** aset game, tekstur, audio, ataupun file `game.wasm` milik Rockstar Games / Take-Two Interactive.

---

## Arsitektur Sistem

```text
[ JavaScript RP Scripts (server.js) ]  <-- Gaya FiveM: RegisterCommand, on, onNet, emitNet
                 │
                 ▼
[ Dedicated RP Server (rp-server) ]    <-- Rust UDP + Reliable ACK + OneSync Spatial Culling (300m)
                 │  (UDP Port 47520)
     ┌───────────┴───────────┬───────────────────────┐
     ▼                       ▼                       ▼
[ iOS Client ]        [ Android Client ]       [ PC / macOS Client ]
(Metal + AOT/JIT)      (Vulkan + JIT)          (DX12/Vulkan/Metal)
     └───────────┬───────────┴───────────────────────┘
                 ▼
    [ rp-client-core + rp-wasm-patcher ]
    Mengekspor fungsi C++ internal dari Custom Name Section `game.wasm`
    menjadi `mp:<Command>` & memanggilnya tiap frame di `GtaThread::Update`.
```

---

## Struktur Workspace

| Folder / Crate | Deskripsi |
| :--- | :--- |
| `crates/rp-proto` | Protokol jaringan biner 30Hz ultra-ringan (`PlayerSyncState` 81 byte) + kanal *Reliable UDP* untuk RPC, Chat, NetEvent, dan HUD. |
| `crates/rp-wasm-patcher` | Alat inspeksi simbol C++ dan *auto-patcher* `game.wasm` (termasuk stub `CMovieMgr::PreloadMovie`) tanpa *reverse engineering* heksadesimal manual. |
| `crates/rp-lite-trimmer` | Pemangkas *streaming* ZIP64 yang menyusutkan arsip ~20 GB menjadi paket **Sandbox/RP Lite (~10–11 GB)** serta menyinkronkan `data/manifest.json` & `dlcList.xml`. |
| `crates/rp-js-runtime` | Mesin *scripting* **JavaScript (Boa ES2023+)** murni Rust untuk membuat fitur Roleplay di sisi server. |
| `crates/rp-server` | Dedicated server lintas platform dengan *Spatial Culling* (hemat baterai & RAM HP) serta pemuat otomatis `fxmanifest.json`. |
| `crates/rp-client-core` | Pustaka client bebas-OS (`aarch64-apple-ios`, `aarch64-linux-android`, Windows, macOS, Linux) untuk antrean RPC, *streaming* kendaraan/animasi, dan *NPC Traffic Disabler*. |
| `resources/roleplay-core` | Contoh *gamemode* Roleplay berbahasa Indonesia dalam JavaScript (`/car`, `/tp`, `/job`, `/anim`, `/me`, `/do`, `/teriak`, `/ooc`, `/dompet`, `/transfer`, `/cuaca`). |
| `.github/workflows/` | Pipeline **GitHub Actions Cloud Build** untuk kompilasi otomatis Linux, Windows, macOS, iOS, dan Android di cloud. |

---

## Contoh API JavaScript Server (`resources/roleplay-core/server.js`)

```javascript
on("playerJoining", (source, name, platform) => {
    SetPlayerCash(source, 5000);
    SetPlayerBank(source, 25000);
    DisableNpcTraffic(source, true); // Hemat RAM & naikkan FPS di iOS/Android
    SendChatMessage(source, "[KOTA]", `Selamat datang, ${name} (${platform})!`);
});

RegisterCommand("car", (source, args) => {
    const model = args[0] || "sultan";
    const [x, y, z] = GetPlayerCoords(source);
    SpawnVehicleForPlayer(source, model, x + 2.5, y + 2.5, z + 0.5, 0.0, true);
});
```

---

## Cloud Build (Tanpa Beban Kompilasi Lokal)

Seluruh kompilasi dan pengujian dijalankan otomatis oleh **GitHub Actions** melalui `.github/workflows/ci-build.yml` setiap kali ada *push* ke branch `main`:
- **Linux (`ubuntu-latest`):** Menjalankan `cargo test --workspace`, mem-*build* `rp-server`, `rp-wasm-patcher`, serta pustaka `aarch64-linux-android`.
- **Windows (`windows-latest`):** Mem-*build* `rp-server.exe` dan `rp-wasm-patcher.exe`.
- **Apple (`macos-latest`):** Mem-*build* binary macOS Apple Silicon serta pustaka `aarch64-apple-ios`.

---

## Lisensi

Dirilis di bawah lisensi ganda **MIT OR Apache-2.0**.
