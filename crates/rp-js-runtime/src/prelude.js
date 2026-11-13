// FiveM-style JavaScript Server Runtime Prelude
// Provides `RegisterCommand`, `on`, `onNet`, `emitNet`, Player Economy, and GTA V Native RPCs.

globalThis.__rp_commands = new Map();
globalThis.__rp_events = new Map();
globalThis.__rp_net_events = new Map();

function __callHost(payload) {
    const raw = __rp_host_call(JSON.stringify(payload));
    return JSON.parse(raw);
}

globalThis.console = {
    log: (...args) => __callHost({ op: "log", level: "INFO", message: args.map(String).join(" ") }),
    warn: (...args) => __callHost({ op: "log", level: "WARN", message: args.map(String).join(" ") }),
    error: (...args) => __callHost({ op: "log", level: "ERROR", message: args.map(String).join(" ") }),
};

// --- FiveM-compatible Event & Command Registration ---

globalThis.RegisterCommand = function (commandName, handler) {
    const key = String(commandName).toLowerCase();
    globalThis.__rp_commands.set(key, handler);
    __callHost({ op: "registerCommand", name: key });
};

globalThis.on = function (eventName, handler) {
    const key = String(eventName);
    if (!globalThis.__rp_events.has(key)) {
        globalThis.__rp_events.set(key, []);
    }
    globalThis.__rp_events.get(key).push(handler);
};

globalThis.onNet = function (eventName, handler) {
    const key = String(eventName);
    if (!globalThis.__rp_net_events.has(key)) {
        globalThis.__rp_net_events.set(key, []);
    }
    globalThis.__rp_net_events.get(key).push(handler);
};

globalThis.emitNet = function (eventName, targetId, data = {}) {
    __callHost({
        op: "emitNet",
        eventName: String(eventName),
        targetId: Number(targetId), // -1 = broadcast to all players
        jsonPayload: JSON.stringify(data),
    });
};

// --- Player Queries & Economy / Roleplay State ---

globalThis.GetPlayers = function () {
    return __callHost({ op: "getPlayers" }).players || [];
};

globalThis.GetPlayerName = function (playerId) {
    return __callHost({ op: "getPlayerInfo", playerId: Number(playerId) }).name || "Unknown";
};

globalThis.GetPlayerPlatform = function (playerId) {
    return __callHost({ op: "getPlayerInfo", playerId: Number(playerId) }).platform || "Pc";
};

globalThis.GetPlayerCoords = function (playerId) {
    return __callHost({ op: "getPlayerInfo", playerId: Number(playerId) }).pos || [0, 0, 72];
};

globalThis.GetPlayerCash = function (playerId) {
    return __callHost({ op: "getPlayerInfo", playerId: Number(playerId) }).cash || 0;
};

globalThis.SetPlayerCash = function (playerId, amount) {
    __callHost({ op: "setEconomy", playerId: Number(playerId), cash: Math.floor(Number(amount)) });
};

globalThis.GetPlayerBank = function (playerId) {
    return __callHost({ op: "getPlayerInfo", playerId: Number(playerId) }).bank || 0;
};

globalThis.SetPlayerBank = function (playerId, amount) {
    __callHost({ op: "setEconomy", playerId: Number(playerId), bank: Math.floor(Number(amount)) });
};

globalThis.GetPlayerJob = function (playerId) {
    return __callHost({ op: "getPlayerInfo", playerId: Number(playerId) }).job || "Warga Sipil";
};

globalThis.SetPlayerJob = function (playerId, jobName) {
    __callHost({ op: "setJob", playerId: Number(playerId), job: String(jobName) });
};

// --- Chat & Proximity Roleplay ---

globalThis.SendChatMessage = function (targetId, prefix, text, color = [255, 255, 255]) {
    __callHost({
        op: "sendChat",
        targetId: Number(targetId), // -1 = broadcast global
        prefix: String(prefix),
        text: String(text),
        color: [Number(color[0] ?? 255), Number(color[1] ?? 255), Number(color[2] ?? 255)],
    });
};

globalThis.BroadcastProximityChat = function (sourceId, radiusMeters, prefix, text, color = [194, 162, 218]) {
    __callHost({
        op: "proximityChat",
        sourceId: Number(sourceId),
        radius: Number(radiusMeters),
        prefix: String(prefix),
        text: String(text),
        color: [Number(color[0] ?? 194), Number(color[1] ?? 162), Number(color[2] ?? 218)],
    });
};

// --- GTA V World & Entity Control (Cross-Platform iOS / Android / PC) ---

globalThis.DisableNpcTraffic = function (targetId, disabled = true) {
    __callHost({
        op: "disableNpcTraffic",
        targetId: Number(targetId),
        disabled: Boolean(disabled),
    });
};

globalThis.TeleportPlayer = function (targetId, x, y, z, heading = null) {
    __callHost({
        op: "teleport",
        targetId: Number(targetId),
        pos: [Number(x), Number(y), Number(z)],
        heading: heading !== null ? Number(heading) : null,
    });
};

globalThis.SpawnVehicleForPlayer = function (targetId, modelName, x, y, z, heading = 0.0, warpIn = true) {
    __callHost({
        op: "spawnVehicle",
        targetId: Number(targetId),
        modelName: String(modelName),
        pos: [Number(x), Number(y), Number(z)],
        heading: Number(heading),
        warpIn: Boolean(warpIn),
    });
};

globalThis.PlayPlayerAnim = function (targetId, animDict, animClip, flags = 49) {
    __callHost({
        op: "playAnim",
        targetId: Number(targetId),
        dict: String(animDict),
        clip: String(animClip),
        flags: Number(flags),
    });
};

globalThis.ClearPlayerAnim = function (targetId) {
    __callHost({ op: "clearAnim", targetId: Number(targetId) });
};

globalThis.GivePlayerWeapon = function (targetId, weaponName, ammo = 250) {
    __callHost({
        op: "giveWeapon",
        targetId: Number(targetId),
        weaponName: String(weaponName),
        ammo: Number(ammo),
    });
};

globalThis.SetWorldWeather = function (targetId, weatherName) {
    __callHost({
        op: "setWeather",
        targetId: Number(targetId),
        weather: String(weatherName),
    });
};

globalThis.InvokeClientNative = function (targetId, commandName, args = []) {
    __callHost({
        op: "invokeNative",
        targetId: Number(targetId),
        command: String(commandName),
        args,
    });
};

// --- Internal Dispatchers Invoked by Rust Server ---

globalThis.__rp_dispatch_command = function (sourceId, commandName, argsArray) {
    const fn = globalThis.__rp_commands.get(String(commandName).toLowerCase());
    if (!fn) return false;
    fn(sourceId, argsArray);
    return true;
};

globalThis.__rp_dispatch_event = function (eventName, argsArray) {
    const list = globalThis.__rp_events.get(String(eventName));
    if (!list) return;
    for (const fn of list) {
        fn(...argsArray);
    }
};

globalThis.__rp_dispatch_net_event = function (eventName, sourceId, jsonPayload) {
    const list = globalThis.__rp_net_events.get(String(eventName));
    if (!list) return;
    let parsed = null;
    try {
        parsed = JSON.parse(jsonPayload);
    } catch (_) {
        parsed = jsonPayload;
    }
    for (const fn of list) {
        fn(sourceId, parsed);
    }
};

