# GTA5-Wasm-RP — Cross-Platform Online Roleplay Engine (iOS · Android · PC · macOS)

Kerangka kerja *clean-room* open-source ala **FiveM** untuk menjalankan **Server & Client Online Roleplay** lintas perangkat (**iPhone 15 / iOS Metal**, **Android Vulkan**, dan **PC / macOS**) di atas arsitektur WebAssembly (`game.wasm`), lengkap dengan **JavaScript Server Scripting (`boa_engine` ES2023+)**, **OneSync Spatial Culling**, dan **Wasmtime AOT Precompiler** untuk iOS standar (tanpa jailbreak / tanpa JIT).

> **Peringatan Legal / Hak Cipta:** Repositori ini **100% murni kode jaringan, runtime JavaScript, host WebAssembly, dan utilitas pemangkas mandiri**. Repositori ini **tidak menyertakan** aset game, tekstur, audio, ataupun biner `game.wasm` / `game.cwasm` milik Rockstar Games / Take-Two Interactive.

---

## Arsitektur Online Roleplay Terpadu

```text
[ JavaScript RP Resources (server.js) ]  <-- Gaya FiveM: RegisterCommand, on, onNet, emitNet
                   │
                   ▼
[ Dedicated RP Server (rp-server) ]      <-- Rust UDP Port 47520 + OneSync 300m Culling
   ├─ Paket 1..23 : Sinkronisasi 3D Penuh (Ped, Mobil, 50 Mod, Baju, Tembakan, Sirine)
   └─ Paket 28..29: Kanal Reliable Roleplay (Chat IC/OOC, HUD Ekonomi, Dynamic Native RPC)
                   │
      ┌────────────┴────────────┬────────────────────────┐
      ▼                         ▼                        ▼
[ iOS Client (.ipa) ]   [ Android Client ]      [ PC / macOS Client ]
Metal + Embedded AOT     Vulkan + Cranelift      DX12 / Vulkan / Metal
Tombol Sentuh RP/CHAT    Zero-NPC RAM Saver      Live Chat Bar (Tombol T)
```

---

## Struktur Workspace

| Folder / Crate | Deskripsi |
| :--- | :--- |
| `crates/rp-proto` | Protokol jaringan terpadu (`MAGIC = 0x47`, `PlayerSyncState` 105 byte) + kanal *Reliable Roleplay* (`PKT_RP_REL = 28` / `PKT_RP_ACK = 29`) + ping status server (`PKT_PING = 250`). |
| `crates/rp-server` | Dedicated server otoritatif lintas platform dengan *OneSync 300m Spatial Culling*, relai 3D penuh, serta pemuat otomatis skrip JavaScript `fxmanifest.json`. |
| `crates/rp-js-runtime` | Mesin *scripting* **JavaScript (Boa ES2023+)** murni Rust di sisi server. |
| `crates/rp-client-core` | Mesin sesi Roleplay bebas-OS (`RpRoleplaySession` & `WasmGameBridge`): eksekutor RPC asinkron, *Zero-NPC Mobile RAM Saver*, dan perender HUD/Chat native langsung di *render loop* GTA V (0 MB RAM HTML/CEF). |
| `crates/rp-ios-aot` | Kompilator silang *Ahead-of-Time* (`aarch64-apple-ios`) berbasis Cranelift yang menyuntikkan 9 *hook* runtime (termasuk batas RAM streaming 2.5 GB untuk iPhone 6 GB) + 206 ekspor `mp:<Command>` dan menghasilkan `game.cwasm`. |
| `crates/rp-lite-trimmer` | Pemangkas *streaming* ZIP64 hemat RAM untuk menyusutkan arsip game secara drastis (menghapus video `.bik`, dialog misi cerita, dan radio) sambil menjaga keutuhan Custom `Name` Section di `game.wasm`. |
| `crates/rp-wasm-patcher` | Alat inspeksi simbol C++ dan *patcher* mandiri `game.wasm`. |
| `client/` | Aplikasi Client Host Wasmtime + `wgpu` (iOS `.ipa` + macOS `.app` + Desktop) terintegrasi penuh dengan `rp-client-core`, tombol sentuh **RP / CHAT** di iOS, dan *typing bar* `KeyT` di PC/Mac. |
| `resources/roleplay-core` | Starter *gamemode* Roleplay berbahasa Indonesia (`/bantuan`, `/car`, `/tp`, `/job`, `/anim`, `/me`, `/do`, `/teriak`, `/ooc`, `/dompet`, `/transfer`, `/cuaca`). |
