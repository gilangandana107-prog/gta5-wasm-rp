// ============================================================================
// IndoCross RP — Starter Gamemode (JavaScript Server Resource)
// Jalan lintas platform untuk Client iOS, Android, dan PC!
// ============================================================================

const SPAWN_LOCATIONS = {
    bandara: [-1037.0, -2737.0, 20.17, 330.0],
    polisi: [425.1, -979.5, 30.71, 90.0],
    rs: [298.6, -584.5, 43.26, 70.0],
    legion: [215.8, -810.1, 30.73, 160.0],
    pantai: [-1393.2, -1020.5, 7.39, 120.0],
};

const RP_ANIMS = {
    angkat_tangan: ["missminuteman_1ig_2", "handsup_base", 49],
    duduk: ["amb@world_human_picnic@male@base", "base", 1],
    hormat: ["mp_player_int_uppersalute", "mp_player_int_salute", 49],
    joget: ["anim@amb@nightclub@dancers@solomun_entourage@", "mi_dance_facedj_17_v1_female^1", 1],
};

const VALID_JOBS = {
    warga: "Warga Sipil",
    polisi: "Kepolisian LSPD",
    medis: "EMS Rumah Sakit",
    mekanik: "Mekanik Bengkel",
    taksi: "Supir Taksi",
};

// Event otomatis saat pemain baru masuk (baik dari iPhone, Android, maupun PC)
on("playerJoining", (source, name, platform) => {
    console.log(`Pemain masuk: ${name} (ID: ${source}, Device: ${platform})`);

    // Modal awal warga baru
    SetPlayerCash(source, 5000);
    SetPlayerBank(source, 25000);
    SetPlayerJob(source, "Warga Sipil");

    // Matikan AI NPC & trafik lokal biar RAM iOS/Android hemat & FPS stabil!
    DisableNpcTraffic(source, true);

    // Spawn di Legion Square (pusat kota RP)
    const [x, y, z, h] = SPAWN_LOCATIONS.legion;
    TeleportPlayer(source, x, y, z, h);

    SendChatMessage(source, "[KOTA]", `Selamat datang di Los Santos, ${name} (${platform})!`, [80, 220, 120]);
    SendChatMessage(source, "[INFO]", "Ketik /bantuan untuk melihat daftar perintah Roleplay.", [180, 220, 255]);
});

// /bantuan — Daftar perintah
RegisterCommand("bantuan", (source) => {
    SendChatMessage(source, "[RP]", "/me [aksi] | /do [kondisi] | /ooc [pesan] | /teriak [pesan]", [255, 215, 0]);
    SendChatMessage(source, "[FITUR]", "/car [mobil] | /tp [bandara/polisi/rs/legion/pantai] | /anim [angkat_tangan/duduk/hormat/joget/stop]", [255, 215, 0]);
    SendChatMessage(source, "[EKONOMI]", "/dompet | /transfer [id] [jumlah] | /job [warga/polisi/medis/mekanik/taksi]", [255, 215, 0]);
});

// /me — Aksi karakter proximity (radius 20 meter)
RegisterCommand("me", (source, args) => {
    if (args.length === 0) return SendChatMessage(source, "[INFO]", "Gunakan: /me [aksi karakter]");
    const name = GetPlayerName(source);
    BroadcastProximityChat(source, 20.0, "*", `${name} ${args.join(" ")}`, [194, 162, 218]);
});

// /do — Deskripsi situasi sekitar (radius 20 meter)
RegisterCommand("do", (source, args) => {
    if (args.length === 0) return SendChatMessage(source, "[INFO]", "Gunakan: /do [deskripsi situasi]");
    const name = GetPlayerName(source);
    BroadcastProximityChat(source, 20.0, "**", `${args.join(" ")} (( ${name} ))`, [130, 190, 250]);
});

// /teriak — Berteriak jarak jauh (radius 45 meter)
RegisterCommand("teriak", (source, args) => {
    if (args.length === 0) return;
    const name = GetPlayerName(source);
    BroadcastProximityChat(source, 45.0, `${name} berteriak:`, `${args.join(" ").toUpperCase()}!`, [255, 140, 100]);
});

// /ooc — Chat Out of Character global
RegisterCommand("ooc", (source, args) => {
    if (args.length === 0) return;
    const name = GetPlayerName(source);
    const platform = GetPlayerPlatform(source);
    SendChatMessage(-1, `[OOC | ${platform}] ${name}:`, args.join(" "), [180, 180, 180]);
});

// /car [nama_model] — Spawn kendaraan GTA V bawaan (aman untuk RAM HP!)
RegisterCommand("car", (source, args) => {
    const model = (args[0] || "sultan").toLowerCase();
    const [x, y, z] = GetPlayerCoords(source);
    SpawnVehicleForPlayer(source, model, x + 2.5, y + 2.5, z + 0.5, 0.0, true);
    SendChatMessage(source, "[GARASI]", `Memanggil kendaraan '${model}'...`, [100, 255, 160]);
});

// /tp [lokasi] — Pindah ke lokasi populer
RegisterCommand("tp", (source, args) => {
    const key = (args[0] || "").toLowerCase();
    const target = SPAWN_LOCATIONS[key];
    if (!target) {
        const list = Object.keys(SPAWN_LOCATIONS).join(", ");
        return SendChatMessage(source, "[TP]", `Lokasi tersedia: ${list}`, [255, 180, 100]);
    }
    TeleportPlayer(source, target[0], target[1], target[2], target[3]);
    SendChatMessage(source, "[TP]", `Berhasil pindah ke ${key.toUpperCase()}.`, [100, 255, 160]);
});

// /anim [nama] — Animasi Roleplay
RegisterCommand("anim", (source, args) => {
    const key = (args[0] || "").toLowerCase();
    if (key === "stop") {
        ClearPlayerAnim(source);
        return;
    }
    const anim = RP_ANIMS[key];
    if (!anim) {
        const list = Object.keys(RP_ANIMS).join(", ");
        return SendChatMessage(source, "[ANIM]", `Pilihan: ${list}, stop`, [255, 180, 100]);
    }
    PlayPlayerAnim(source, anim[0], anim[1], anim[2]);
});

// /job [pekerjaan] — Ganti pekerjaan RP (+ perlengkapan khusus Polisi)
RegisterCommand("job", (source, args) => {
    const key = (args[0] || "").toLowerCase();
    const label = VALID_JOBS[key];
    if (!label) {
        return SendChatMessage(source, "[DISNAKER]", "Pilihan: warga, polisi, medis, mekanik, taksi", [255, 180, 100]);
    }
    SetPlayerJob(source, label);
    if (key === "polisi") {
        GivePlayerWeapon(source, "combatpistol", 120);
        GivePlayerWeapon(source, "stungun", 100);
        GivePlayerWeapon(source, "nightstick", 1);
    }
    SendChatMessage(source, "[DISNAKER]", `Pekerjaan kamu sekarang: ${label}`, [100, 220, 255]);
});

// /dompet & /transfer — Sistem ekonomi warga
RegisterCommand("dompet", (source) => {
    const cash = GetPlayerCash(source);
    const bank = GetPlayerBank(source);
    const job = GetPlayerJob(source);
    SendChatMessage(source, "[DOMPET]", `Pekerjaan: ${job} | Tunai: $${cash} | Bank: $${bank}`, [120, 255, 120]);
});

RegisterCommand("transfer", (source, args) => {
    const targetId = Number(args[0]);
    const amount = Math.floor(Number(args[1]));
    if (!targetId || !amount || amount <= 0) {
        return SendChatMessage(source, "[BANK]", "Gunakan: /transfer [ID Pemain] [Jumlah]", [255, 120, 120]);
    }
    const myBank = GetPlayerBank(source);
    if (myBank < amount) {
        return SendChatMessage(source, "[BANK]", "Saldo bank kamu tidak mencukupi!", [255, 90, 90]);
    }
    const targetPlayers = GetPlayers();
    if (!targetPlayers.includes(targetId)) {
        return SendChatMessage(source, "[BANK]", `Pemain dengan ID ${targetId} sedang tidak online.`, [255, 90, 90]);
    }

    SetPlayerBank(source, myBank - amount);
    SetPlayerBank(targetId, GetPlayerBank(targetId) + amount);

    const senderName = GetPlayerName(source);
    const targetName = GetPlayerName(targetId);
    SendChatMessage(source, "[BANK]", `Berhasil transfer $${amount} ke ${targetName} (ID ${targetId}).`, [100, 255, 140]);
    SendChatMessage(targetId, "[BANK]", `Menerima transfer $${amount} dari ${senderName} (ID ${source}).`, [100, 255, 140]);
});

// /cuaca [cerah/hujan/badai/kabut]
RegisterCommand("cuaca", (source, args) => {
    const map = {
        cerah: "EXTRASUNNY",
        berawan: "CLOUDS",
        hujan: "RAIN",
        badai: "THUNDER",
        kabut: "FOGGY",
    };
    const w = map[(args[0] || "cerah").toLowerCase()] || "CLEAR";
    SetWorldWeather(-1, w);
    SendChatMessage(-1, "[BMKG]", `Cuaca kota berubah menjadi ${w}.`, [135, 206, 250]);
});

