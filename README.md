# Cross-Platform GTA V Wasm Roleplay Framework (iOS · Android · PC)

Kerangka kerja *clean-room* open-source ala **FiveM** untuk menjalankan server & client Roleplay lintas platform (**iOS Metal**, **Android Vulkan**, dan **PC/macOS**) di atas arsitektur WebAssembly (`game.wasm`) dengan **JavaScript Server Scripting** dan **Cloud Build GitHub Actions** (0% beban kompilasi di laptop/Mac lokal).

> **Peringatan Legal / Hak Cipta:** Repositori ini **100% murni kode jaringan, runtime JavaScript, dan alat patcher mandiri**. Repositori ini **tidak menyertakan** aset game, tekstur, skrip internal, ataupun file `game.wasm` milik Rockstar Games / Take-Two Interactive.

---

## Fitur Utama (MVP Tahap 1)

1. **JavaScript Roleplay Engine (`crates/rp-js-runtime`)**
   - Ditenagai engine **Boa ES2023+ murni Rust** (tanpa dependensi C/C++ berat, langsung jalan di semua OS).
   - Mendukung gaya penulisan *resource* ala FiveM (`fxmanifest.json` + `server.js`):
     - `RegisterCommand(cmd, (source, args) => ...)`
     - `on("playerJoining", ...)`, `on("playerDropped", ...)`
     - `onNet(event, ...)` & `emitNet(event, targetId, payload)`
     - `SpawnVehicleForPlayer`, `TeleportPlayer`, `PlayPlayerAnim`, `GivePlayerWeapon`, `SetWorldWeather`
     - `DisableNpcTraffic` (mematikan trafik NPC lokal demi menghemat ratusan MB RAM di iPhone & Android!)
     - Ekonomi & Chat bawaan: `SetPlayerCash`, `SetPlayerBank`, `SetPlayerJob`, `SendChatMessage`, `BroadcastProximityChat`.

2. **Dedicated Server dengan *OneSync-Style Spatial Culling* (`crates/rp-server`)**
   - Hanya meneruskan paket posisi 30Hz ke pemain dalam radius **300 meter**, menghemat baterai, kuota, dan memori perangkat mobile.
   - Dilengkapi *Reliable UDP (`REL`/`ACK`)* untuk memastikan paket RPC, Chat, dan HUD tidak pernah hilang.

3. **Zero-Hex-Reverse-Engineering Wasm Patcher (`crates/rp-wasm-patcher`)**
   - Membaca *Custom `Name` Section* di `game.wasm`, mengekspor fungsi C++ internal RAGE Engine menjadi `mp:<Command>`, serta menyuntikkan callback frame `env.rp_script_tick` ke `GtaThread::Update(int)`.

4. **Cross-Platform Client Core (`crates/rp-client-core`)**
   - Pustaka inti bebas-OS yang terkompilasi langsung ke `aarch64-apple-ios`, `aarch64-linux-android`, Windows, macOS, dan Linux.

5. **100% Cloud Build di GitHub Actions (`.github/workflows/`)**
   - Tidak perlu menginstal Rust, Xcode 30 GB, atau Android NDK di Mac kamu! Cukup edit file `.js` / `.rs` ringan lalu `git push`—server Linux, Windows, dan macOS milik GitHub yang akan meng-*compile* dan menguji semuanya secara gratis.

---

## Cara Pakai Tanpa Bikin Mac Berat (Lewat GitHub Cloud)

Karena di Mac kamu sudah terpasang `git` dan `gh` (GitHub CLI), kamu cukup menjalankan:

```bash
git init
git add .
git commit -m "feat: initial cross-platform JS roleplay framework"
gh repo create v-cross-rp --public --source=. --remote=origin --push
```

Begitu ter-*push*, buka tab **Actions** di repo GitHub kamu:
- Semua *unit test* dan kompilasi binary **Linux Server**, **Windows PC**, **macOS**, **iOS ARM64**, dan **Android ARM64** langsung berjalan otomatis di cloud GitHub!
