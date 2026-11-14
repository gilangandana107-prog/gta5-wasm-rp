//! Cross-platform binary & RPC network protocol for GTA V Wasm Roleplay (iOS / Android / PC / Mac).
//!
//! Unifies 100% of Muguet's battle-tested 3D entity/vehicle/clothing/mod/combat sync (`MAGIC = 0x47`,
//! packets `1..=22`) with an authoritative FiveM-style Roleplay reliable channel (`PKT_RP_REL = 28`,
//! `PKT_RP_ACK = 29`) for JavaScript gamemodes, HUD state, proximity chat, and dynamic Wasm Native RPCs.

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 47520;
pub const MAGIC: u8 = 0x47;
pub const TIMEOUT_SECS: u64 = 8;

// Core connection & 3D entity/vehicle/combat sync packet kinds (100% compatible with mp.rs)
pub const PKT_HELLO: u8 = 1;
pub const PKT_WELCOME: u8 = 2;
pub const PKT_STATE: u8 = 3;
pub const PKT_NAME: u8 = 4;
pub const PKT_PLAYER_INFO: u8 = PKT_NAME;
pub const PKT_LEAVE: u8 = 5;
pub const PKT_BYE: u8 = 6;
pub const PKT_FULL: u8 = 7;
pub const PKT_LOOK: u8 = 8;
pub const PKT_SHOT: u8 = 9;
pub const PKT_POP: u8 = 10;
pub const PKT_KILL: u8 = 11;
pub const PKT_VLOOK: u8 = 12;
pub const PKT_TAKEN: u8 = 13;
pub const PKT_EXPLODE: u8 = 14;
pub const PKT_HIT: u8 = 15;
pub const PKT_VHIT: u8 = 16;
pub const PKT_TIME: u8 = 17;
pub const PKT_POPX: u8 = 18;
/// Peer-to-peer reliable wrapper relayed by server (`[from, seq_lo, seq_hi, kind, ...]`).
pub const PKT_REL: u8 = 19;
/// Peer-to-peer reliable ACK relayed by server (`[from, to, seq_lo, seq_hi]`).
pub const PKT_ACK: u8 = 20;
pub const PKT_KILLFEED: u8 = 21;
pub const PKT_VCTL: u8 = 22;
/// 35m proximity Opus voice packet.
pub const PKT_VOICE: u8 = 23;

// Dedicated Client <-> Authoritative RP Server reliable channel (zero collision with P2P PKT_REL)
pub const PKT_RP_REL: u8 = 28;
pub const PKT_RP_ACK: u8 = 29;

// Lightweight server browser / healthcheck ping
pub const PKT_PING: u8 = 250;
pub const PKT_PONG: u8 = 251;

// Reliable Roleplay inner message kinds (sent inside PKT_RP_REL)
pub const REL_CHAT_MSG: u8 = 30;
pub const REL_HUD_UPDATE: u8 = 31;
pub const REL_SERVER_RPC: u8 = 32;
pub const REL_CLIENT_EVENT: u8 = 33;
pub const REL_SERVER_EVENT: u8 = 34;

// Player state bitflags (exact match with mp.rs)
pub const FLAG_IN_VEHICLE: u8 = 1 << 0;
pub const FLAG_DEAD: u8 = 1 << 1;
pub const FLAG_AIMING: u8 = 1 << 2;
pub const FLAG_INSIDE: u8 = 1 << 3;
pub const FLAG_RAGDOLL: u8 = 1 << 4;
pub const FLAG_ENTERING: u8 = 1 << 5;

/// Returns true if this 3D multiplayer packet kind is relayed across peers by the server.
pub fn is_relayed_mp_packet(kind: u8) -> bool {
    matches!(
        kind,
        PKT_STATE
            | PKT_LOOK
            | PKT_SHOT
            | PKT_POP
            | PKT_KILL
            | PKT_VLOOK
            | PKT_TAKEN
            | PKT_EXPLODE
            | PKT_HIT
            | PKT_VHIT
            | PKT_TIME
            | PKT_POPX
            | PKT_REL
            | PKT_ACK
            | PKT_KILLFEED
            | PKT_VCTL
            | PKT_VOICE
    )
}

/// Returns true if this high-frequency packet should be spatially culled (OneSync 300m radius).
pub fn is_spatially_culled_packet(kind: u8) -> bool {
    matches!(kind, PKT_STATE | PKT_SHOT | PKT_POP | PKT_POPX | PKT_VCTL)
}

/// Device platform reported during handshake so JS scripts can optimize per-device if needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Ios,
    Android,
    Pc,
    Mac,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelloPayload {
    pub name: String,
    pub platform: Platform,
}

/// Full 30Hz 3D synchronization snapshot (104 bytes — exact wire match with Muguet `proto::State`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerSyncState {
    pub ped_model: u32,
    pub pos: [f32; 3],
    pub heading: f32,
    pub vel: [f32; 3],
    pub flags: u8,
    pub veh_model: u32,
    pub veh_pos: [f32; 3],
    pub veh_rot: [f32; 3],
    pub veh_vel: [f32; 3],
    pub weapon: u32,
    pub health: u16,
    pub armour: u8,
    pub veh_owner: u8,
    pub seat: i8,
    pub veh_colours: [u8; 2],
    pub veh_engine: i16,
    pub veh_body: i16,
    pub veh_tank: i16,
    pub veh_tyres: u8,
    pub veh_windows: u8,
    pub veh_dead: bool,
    pub aim: [f32; 3],
}

pub const SYNC_STATE_SIZE: usize =
    4 + 12 + 4 + 12 + 1 + 4 + 12 + 12 + 12 + 4 + 2 + 1 + 1 + 1 + 2 + 6 + 3 + 12; // 104 bytes

impl PlayerSyncState {
    pub fn in_vehicle(&self) -> bool {
        self.flags & FLAG_IN_VEHICLE != 0
    }

    pub fn effective_pos(&self) -> [f32; 3] {
        if self.in_vehicle() {
            self.veh_pos
        } else {
            self.pos
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend(self.ped_model.to_le_bytes());
        for v in self.pos {
            out.extend(v.to_le_bytes());
        }
        out.extend(self.heading.to_le_bytes());
        for v in self.vel {
            out.extend(v.to_le_bytes());
        }
        out.push(self.flags);
        out.extend(self.veh_model.to_le_bytes());
        for v in self.veh_pos.iter().chain(&self.veh_rot).chain(&self.veh_vel) {
            out.extend(v.to_le_bytes());
        }
        out.extend(self.weapon.to_le_bytes());
        out.extend(self.health.to_le_bytes());
        out.extend([
            self.armour,
            self.veh_owner,
            self.seat as u8,
            self.veh_colours[0],
            self.veh_colours[1],
        ]);
        for v in [self.veh_engine, self.veh_body, self.veh_tank] {
            out.extend(v.to_le_bytes());
        }
        out.extend([self.veh_tyres, self.veh_windows, self.veh_dead as u8]);
        for v in self.aim {
            out.extend(v.to_le_bytes());
        }
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < SYNC_STATE_SIZE {
            return None;
        }
        fn read_u32(b: &[u8], at: &mut usize) -> u32 {
            let v = u32::from_le_bytes(b[*at..*at + 4].try_into().unwrap());
            *at += 4;
            v
        }
        fn read_f32(b: &[u8], at: &mut usize) -> f32 {
            f32::from_bits(read_u32(b, at))
        }
        fn read_v3(b: &[u8], at: &mut usize) -> [f32; 3] {
            [read_f32(b, at), read_f32(b, at), read_f32(b, at)]
        }

        let mut at = 0;
        let ped_model = read_u32(b, &mut at);
        let pos = read_v3(b, &mut at);
        let heading = read_f32(b, &mut at);
        let vel = read_v3(b, &mut at);
        let flags = b[at];
        at += 1;
        let veh_model = read_u32(b, &mut at);
        let veh_pos = read_v3(b, &mut at);
        let veh_rot = read_v3(b, &mut at);
        let veh_vel = read_v3(b, &mut at);
        let weapon = read_u32(b, &mut at);
        let health = u16::from_le_bytes([b[at], b[at + 1]]);
        let t = &b[at + 2..at + 16];
        let h = |k: usize| i16::from_le_bytes([t[k], t[k + 1]]);
        let mut aim_at = at + 16;
        let aim = read_v3(b, &mut aim_at);

        let s = Self {
            ped_model,
            pos,
            heading,
            vel,
            flags,
            veh_model,
            veh_pos,
            veh_rot,
            veh_vel,
            weapon,
            health,
            armour: t[0],
            veh_owner: t[1],
            seat: t[2] as i8,
            veh_colours: [t[3], t[4]],
            veh_engine: h(5),
            veh_body: h(7),
            veh_tank: h(9),
            veh_tyres: t[11],
            veh_windows: t[12],
            veh_dead: t[13] != 0,
            aim,
        };
        let finite = s
            .pos
            .iter()
            .chain(&s.vel)
            .chain(&s.veh_pos)
            .chain(&s.veh_rot)
            .chain(&s.veh_vel)
            .chain(&s.aim)
            .all(|v| v.is_finite());
        (finite && s.heading.is_finite()).then_some(s)
    }

    pub fn distance_to(&self, other: [f32; 3]) -> f32 {
        let me = self.effective_pos();
        let dx = me[0] - other[0];
        let dy = me[1] - other[1];
        let dz = me[2] - other[2];
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

/// Argument value passed from Server JavaScript to Client `mp:<Command>` Wasm exports.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
pub enum NativeArg {
    Int(i32),
    Float(f32),
    Bool(bool),
    Vec3([f32; 3]),
    Str(String),
}

/// High-level & raw RPC commands sent from Dedicated RP Server to Client Wasm host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerRpc {
    /// Disable local ambient NPC & traffic generation (crucial for mobile RAM & FPS + sync).
    SetNpcTrafficDisabled(bool),
    /// Teleport local player character (or current vehicle) immediately.
    Teleport { pos: [f32; 3], heading: Option<f32> },
    /// Request model, spawn vehicle, and warp local player into driver seat.
    SpawnVehicle {
        model_hash: u32,
        model_name: String,
        pos: [f32; 3],
        heading: f32,
        put_in_driver_seat: bool,
    },
    /// Load anim dictionary and play Roleplay animation on local player.
    PlayAnim {
        dict: String,
        clip: String,
        flags: i32,
    },
    /// Clear active animation/tasks immediately.
    ClearAnim,
    /// Give weapon by hash & ammo count to local player.
    GiveWeapon { weapon_hash: u32, ammo: i32 },
    /// Set weather type across session (e.g. "EXTRASUNNY", "RAIN").
    SetWeather(String),
    /// Set clock time (hour, minute, second).
    SetClock { hour: u8, minute: u8, second: u8 },
    /// Generic dynamic invocation of any exported `mp:<command>` in `game.wasm`!
    InvokeNative {
        command: String,
        args: Vec<NativeArg>,
    },
}

/// Lightweight HUD state rendered natively inside GTA V's render loop (zero Chromium/CEF RAM overhead).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HudState {
    pub cash: i64,
    pub bank: i64,
    pub job: String,
    pub server_name: String,
    pub online_count: u16,
}

/// Chat message or command transmitted between client and server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub prefix: String,
    pub text: String,
    pub color_rgb: [u8; 3],
}

/// Custom JavaScript network event (`emitNet` / `onNet` FiveM style).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetEvent {
    pub event_name: String,
    pub json_payload: String,
}

/// Jenkins One-At-A-Time (JOAAT) case-insensitive hash used universally by GTA V / RAGE Engine.
pub fn joaat(name: &str) -> u32 {
    let mut h: u32 = 0;
    for c in name.bytes() {
        h = h.wrapping_add(c.to_ascii_lowercase() as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

pub fn make_packet(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + payload.len());
    out.push(MAGIC);
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

pub fn make_name_packet(id: u8, name: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + name.len());
    payload.push(id);
    payload.extend_from_slice(name.as_bytes());
    make_packet(PKT_NAME, &payload)
}

pub fn parse_packet(buf: &[u8]) -> Option<(u8, &[u8])> {
    (buf.len() >= 2 && buf[0] == MAGIC).then(|| (buf[1], &buf[2..]))
}

pub fn sanitize_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(24)
        .collect::<String>()
        .trim()
        .to_string();
    if cleaned.is_empty() {
        "Warga".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_joaat_known_gta_hashes() {
        assert_eq!(joaat("adder"), 0xB779A091);
        assert_eq!(joaat("ADDER"), 0xB779A091);
    }

    #[test]
    fn test_sync_state_105b_roundtrip() {
        assert_eq!(SYNC_STATE_SIZE, 105);
        let state = PlayerSyncState {
            ped_model: joaat("mp_m_freemode_01"),
            pos: [215.8, -810.1, 30.73],
            heading: 160.0,
            vel: [0.0, 1.5, 0.0],
            flags: FLAG_IN_VEHICLE | FLAG_AIMING,
            veh_model: joaat("sultan"),
            veh_pos: [215.8, -810.1, 30.5],
            veh_rot: [0.0, 0.0, 160.0],
            veh_vel: [0.0, 12.0, 0.0],
            weapon: joaat("weapon_combatpistol"),
            health: 200,
            armour: 100,
            veh_owner: 1,
            seat: -1,
            veh_colours: [12, 0],
            veh_engine: 1000,
            veh_body: 980,
            veh_tank: 1000,
            veh_tyres: 0,
            veh_windows: 0,
            veh_dead: false,
            aim: [220.0, -800.0, 31.0],
        };
        let mut buf = Vec::new();
        state.encode(&mut buf);
        assert_eq!(buf.len(), SYNC_STATE_SIZE);
        let decoded = PlayerSyncState::decode(&buf).expect("should decode");
        assert_eq!(state, decoded);
    }
}
