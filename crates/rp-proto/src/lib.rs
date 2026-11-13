//! Cross-platform binary & RPC network protocol for GTA V Wasm Roleplay (iOS / Android / PC).
//!
//! Combines ultra-compact binary UDP packets for 30Hz player/vehicle sync with reliable
//! Roleplay packets (Dynamic Native RPC, JS Net Events, Proximity Chat, and Lightweight HUD).

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 47520;
pub const MAGIC: u8 = 0x52; // 'R' for Roleplay Cross-Platform
pub const TIMEOUT_SECS: u64 = 8;

// Core connection & fast unreliable sync packet kinds
pub const PKT_HELLO: u8 = 1;
pub const PKT_WELCOME: u8 = 2;
pub const PKT_STATE: u8 = 3;
pub const PKT_PLAYER_INFO: u8 = 4;
pub const PKT_LEAVE: u8 = 5;
pub const PKT_BYE: u8 = 6;
pub const PKT_FULL: u8 = 7;
pub const PKT_SHOT: u8 = 8;
pub const PKT_VOICE: u8 = 9;

// Reliable wrapper & ACK
pub const PKT_REL: u8 = 19;
pub const PKT_ACK: u8 = 20;

// Reliable Roleplay inner message kinds (sent inside PKT_REL)
pub const REL_CHAT_MSG: u8 = 30;
pub const REL_HUD_UPDATE: u8 = 31;
pub const REL_SERVER_RPC: u8 = 32;
pub const REL_CLIENT_EVENT: u8 = 33;
pub const REL_SERVER_EVENT: u8 = 34;

// Player state bitflags
pub const FLAG_IN_VEHICLE: u8 = 1 << 0;
pub const FLAG_DEAD: u8 = 1 << 1;
pub const FLAG_AIMING: u8 = 1 << 2;
pub const FLAG_RAGDOLL: u8 = 1 << 3;

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

/// Compact 30Hz synchronization snapshot (fixed 85 bytes) for minimal mobile battery/bandwidth use.
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
    pub seat: i8,
}

pub const SYNC_STATE_SIZE: usize = 4 + 12 + 4 + 12 + 1 + 4 + 12 + 12 + 12 + 4 + 2 + 1 + 1; // 81 bytes

impl PlayerSyncState {
    pub fn in_vehicle(&self) -> bool {
        self.flags & FLAG_IN_VEHICLE != 0
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
        out.push(self.armour);
        out.push(self.seat as u8);
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

        let mut at = 0;
        let ped_model = read_u32(b, &mut at);
        let pos = [
            read_f32(b, &mut at),
            read_f32(b, &mut at),
            read_f32(b, &mut at),
        ];
        let heading = read_f32(b, &mut at);
        let vel = [
            read_f32(b, &mut at),
            read_f32(b, &mut at),
            read_f32(b, &mut at),
        ];
        let flags = b[at];
        at += 1;
        let veh_model = read_u32(b, &mut at);
        let veh_pos = [
            read_f32(b, &mut at),
            read_f32(b, &mut at),
            read_f32(b, &mut at),
        ];
        let veh_rot = [
            read_f32(b, &mut at),
            read_f32(b, &mut at),
            read_f32(b, &mut at),
        ];
        let veh_vel = [
            read_f32(b, &mut at),
            read_f32(b, &mut at),
            read_f32(b, &mut at),
        ];
        let weapon = read_u32(b, &mut at);
        let health = u16::from_le_bytes([b[at], b[at + 1]]);
        let armour = b[at + 2];
        let seat = b[at + 3] as i8;

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
            armour,
            seat,
        };
        let finite = s
            .pos
            .iter()
            .chain(&s.vel)
            .chain(&s.veh_pos)
            .chain(&s.veh_rot)
            .chain(&s.veh_vel)
            .all(|v| v.is_finite());
        (finite && s.heading.is_finite()).then_some(s)
    }

    pub fn distance_to(&self, other: [f32; 3]) -> f32 {
        let dx = self.pos[0] - other[0];
        let dy = self.pos[1] - other[1];
        let dz = self.pos[2] - other[2];
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
    /// Teleport local player character immediately.
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

/// Lightweight HUD state rendered by native client overlay (replaces heavy Chromium CEF).
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
        // Known GTA V hash for "adder" is 0xB779A091 (3078201489)
        assert_eq!(joaat("adder"), 0xB779A091);
        assert_eq!(joaat("ADDER"), 0xB779A091);
    }

    #[test]
    fn test_sync_state_roundtrip() {
        let state = PlayerSyncState {
            ped_model: joaat("mp_m_freemode_01"),
            pos: [125.5, -1040.25, 29.3],
            heading: 180.0,
            vel: [0.0, 1.5, 0.0],
            flags: FLAG_IN_VEHICLE,
            veh_model: joaat("police"),
            veh_pos: [125.5, -1040.25, 29.0],
            veh_rot: [0.0, 0.0, 180.0],
            veh_vel: [0.0, 12.0, 0.0],
            weapon: joaat("weapon_combatpistol"),
            health: 200,
            armour: 100,
            seat: -1,
        };
        let mut buf = Vec::new();
        state.encode(&mut buf);
        assert_eq!(buf.len(), SYNC_STATE_SIZE);
        let decoded = PlayerSyncState::decode(&buf).expect("should decode");
        assert_eq!(state, decoded);
    }
}

