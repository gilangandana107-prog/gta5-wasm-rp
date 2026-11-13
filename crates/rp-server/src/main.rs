//! Dedicated Authoritative Roleplay Server for iOS, Android, and PC clients.

use anyhow::Result;
use rp_js_runtime::{JsRoleplayEngine, PlayerScriptView, ScriptEffect};
use rp_proto::*;
use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::time::{Duration, Instant};

const MAX_PLAYERS_DEFAULT: usize = 64;
/// OneSync-style spatial culling radius: only relay 30Hz state updates to nearby players
/// to dramatically reduce RAM, CPU, and mobile data usage on iOS & Android devices.
const SPATIAL_CULL_RADIUS: f32 = 300.0;
const REL_RESEND_INTERVAL: Duration = Duration::from_millis(180);
const REL_MAX_LIFETIME: Duration = Duration::from_secs(5);

struct ConnectedPlayer {
    id: u8,
    addr: SocketAddr,
    name: String,
    platform: Platform,
    state: PlayerSyncState,
    cash: i64,
    bank: i64,
    job: String,
    last_seen: Instant,
    seen_seqs: VecDeque<u16>,
}

struct PendingRel {
    seq: u16,
    target_addr: SocketAddr,
    packet: Vec<u8>,
    first_sent: Instant,
    last_sent: Instant,
}

struct RpServer {
    sock: UdpSocket,
    server_name: String,
    max_players: usize,
    players: HashMap<SocketAddr, ConnectedPlayer>,
    js: JsRoleplayEngine,
    next_rel_seq: u16,
    pending_rels: Vec<PendingRel>,
}

impl RpServer {
    fn new(port: u16, max_players: usize, resources_dir: &Path) -> Result<Self> {
        let sock = UdpSocket::bind(("0.0.0.0", port))?;
        sock.set_read_timeout(Some(Duration::from_millis(50)))?;

        let mut js = JsRoleplayEngine::new()?;
        if resources_dir.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(resources_dir)?
                .filter_map(|e| e.ok())
                .collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let path = entry.path();
                if path.join("fxmanifest.json").is_file() {
                    let manifest = js.load_resource_dir(&path)?;
                    println!(
                        "[rp-server] Loaded JS resource '{}' v{} by {}",
                        manifest.name, manifest.version, manifest.author
                    );
                }
            }
        }

        Ok(Self {
            sock,
            server_name: "IndoCross RP [iOS | Android | PC]".to_string(),
            max_players: max_players.clamp(2, 128),
            players: HashMap::new(),
            js,
            next_rel_seq: 0,
            pending_rels: Vec::new(),
        })
    }

    fn run(&mut self) {
        println!(
            "[rp-server] '{}' listening on UDP {} (max {} players, OneSync radius {}m)",
            self.server_name,
            self.sock.local_addr().unwrap(),
            self.max_players,
            SPATIAL_CULL_RADIUS
        );

        let mut buf = [0u8; 2048];
        let mut last_sweep = Instant::now();

        loop {
            if let Ok((n, from)) = self.sock.recv_from(&mut buf) {
                if let Some((kind, payload)) = parse_packet(&buf[..n]) {
                    self.handle_packet(from, kind, payload);
                }
            }

            self.tick_reliable_resends();

            if last_sweep.elapsed() >= Duration::from_secs(2) {
                last_sweep = Instant::now();
                self.sweep_timeouts();
            }
        }
    }

    fn sync_js_snapshot(&self) {
        let views = self.players.values().map(|p| PlayerScriptView {
            id: p.id,
            name: p.name.clone(),
            platform: p.platform,
            pos: p.state.pos,
            cash: p.cash,
            bank: p.bank,
            job: p.job.clone(),
        });
        self.js.sync_players(views);
    }

    fn handle_packet(&mut self, from: SocketAddr, kind: u8, payload: &[u8]) {
        match kind {
            PKT_HELLO => self.handle_hello(from, payload),
            PKT_BYE => self.disconnect_player(from, "keluar"),
            PKT_STATE => {
                if payload.len() >= 4 + SYNC_STATE_SIZE {
                    let Some(decoded) = PlayerSyncState::decode(&payload[4..]) else {
                        return;
                    };
                    let sender_id = if let Some(p) = self.players.get_mut(&from) {
                        p.last_seen = Instant::now();
                        p.state = decoded;
                        p.id
                    } else {
                        let _ = self.sock.send_to(&make_packet(PKT_LEAVE, &[0]), from);
                        return;
                    };

                    // Spatial Culling (OneSync): only forward 30Hz state to players within 300m!
                    let mut relay_body = Vec::with_capacity(1 + payload.len());
                    relay_body.push(sender_id);
                    relay_body.extend_from_slice(payload);
                    let pkt = make_packet(PKT_STATE, &relay_body);

                    for (addr, other) in &self.players {
                        if *addr != from && other.state.distance_to(decoded.pos) <= SPATIAL_CULL_RADIUS
                        {
                            let _ = self.sock.send_to(&pkt, addr);
                        }
                    }
                }
            }
            PKT_VOICE | PKT_SHOT => {
                let Some(sender) = self.players.get_mut(&from) else {
                    return;
                };
                sender.last_seen = Instant::now();
                let sender_id = sender.id;
                let sender_pos = sender.state.pos;
                let max_dist = if kind == PKT_VOICE { 35.0 } else { SPATIAL_CULL_RADIUS };

                let mut out = Vec::with_capacity(1 + payload.len());
                out.push(sender_id);
                out.extend_from_slice(payload);
                let pkt = make_packet(kind, &out);

                for (addr, other) in &self.players {
                    if *addr != from && other.state.distance_to(sender_pos) <= max_dist {
                        let _ = self.sock.send_to(&pkt, addr);
                    }
                }
            }
            PKT_ACK if payload.len() >= 2 => {
                let seq = u16::from_le_bytes([payload[0], payload[1]]);
                self.pending_rels
                    .retain(|p| !(p.target_addr == from && p.seq == seq));
            }
            PKT_REL if payload.len() >= 3 => {
                let seq = u16::from_le_bytes([payload[0], payload[1]]);
                let inner_kind = payload[2];
                let inner_body = &payload[3..];
                let _ = self
                    .sock
                    .send_to(&make_packet(PKT_ACK, &[payload[0], payload[1]]), from);

                let Some(player_id) = self.accept_reliable_seq(from, seq) else {
                    return;
                };
                self.handle_reliable_inner(player_id, inner_kind, inner_body);
            }
            _ => {}
        }
    }

    fn accept_reliable_seq(&mut self, from: SocketAddr, seq: u16) -> Option<u8> {
        let p = self.players.get_mut(&from)?;
        p.last_seen = Instant::now();
        if p.seen_seqs.contains(&seq) {
            return None;
        }
        p.seen_seqs.push_back(seq);
        while p.seen_seqs.len() > 128 {
            p.seen_seqs.pop_front();
        }
        Some(p.id)
    }

    fn handle_hello(&mut self, from: SocketAddr, payload: &[u8]) {
        if let Some(existing) = self.players.get_mut(&from) {
            existing.last_seen = Instant::now();
            let _ = self
                .sock
                .send_to(&make_packet(PKT_WELCOME, &[existing.id]), from);
            return;
        }
        if self.players.len() >= self.max_players {
            let _ = self.sock.send_to(&make_packet(PKT_FULL, &[]), from);
            return;
        }

        let hello: HelloPayload = serde_json::from_slice(payload).unwrap_or(HelloPayload {
            name: sanitize_name(&String::from_utf8_lossy(payload)),
            platform: Platform::Pc,
        });
        let clean = sanitize_name(&hello.name);
        let id = (1..=254u8)
            .find(|candidate| !self.players.values().any(|p| p.id == *candidate))
            .unwrap_or(1);

        let new_player = ConnectedPlayer {
            id,
            addr: from,
            name: clean.clone(),
            platform: hello.platform,
            state: PlayerSyncState {
                pos: [-1037.0, -2737.0, 20.17], // LSIA Airport default spawn
                ..Default::default()
            },
            cash: 0,
            bank: 0,
            job: "Warga Sipil".to_string(),
            last_seen: Instant::now(),
            seen_seqs: VecDeque::new(),
        };

        println!(
            "[rp-server] {} ({:?}) joined as ID {} from {}",
            clean, hello.platform, id, from
        );
        let _ = self.sock.send_to(&make_packet(PKT_WELCOME, &[id]), from);
        self.players.insert(from, new_player);

        self.sync_js_snapshot();
        if let Ok(effects) = self.js.dispatch_server_event(
            "playerJoining",
            &[
                serde_json::json!(id),
                serde_json::json!(clean),
                serde_json::json!(format!("{:?}", hello.platform)),
            ],
        ) {
            self.apply_script_effects(effects);
        }
        self.push_hud_to_all();
    }

    fn handle_reliable_inner(&mut self, source_id: u8, inner_kind: u8, body: &[u8]) {
        match inner_kind {
            REL_CHAT_MSG => {
                let Ok(chat) = serde_json::from_slice::<ChatMessage>(body) else {
                    return;
                };
                let trimmed = chat.text.trim();
                if let Some(cmd_line) = trimmed.strip_prefix('/') {
                    let mut parts = cmd_line.split_whitespace();
                    let Some(cmd) = parts.next() else { return };
                    let args: Vec<String> = parts.map(String::from).collect();
                    self.sync_js_snapshot();
                    match self.js.dispatch_command(source_id, cmd, &args) {
                        Ok((true, effects)) => self.apply_script_effects(effects),
                        Ok((false, _)) => {
                            self.send_reliable_to_id(
                                source_id,
                                REL_CHAT_MSG,
                                &serde_json::to_vec(&ChatMessage {
                                    prefix: "[SISTEM]".to_string(),
                                    text: format!("Perintah /{cmd} tidak ditemukan."),
                                    color_rgb: [255, 90, 90],
                                })
                                .unwrap_or_default(),
                            );
                        }
                        Err(e) => eprintln!("[rp-server] JS command error: {e}"),
                    }
                } else if !trimmed.is_empty() {
                    // Default local IC proximity chat (20m)
                    let sender_name = self
                        .find_player_by_id(source_id)
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| format!("ID {source_id}"));
                    self.apply_script_effects(vec![ScriptEffect::ProximityChat {
                        source_id,
                        radius: 20.0,
                        msg: ChatMessage {
                            prefix: format!("{sender_name} berkata:"),
                            text: trimmed.to_string(),
                            color_rgb: [240, 240, 240],
                        },
                    }]);
                }
            }
            REL_CLIENT_EVENT => {
                if let Ok(ev) = serde_json::from_slice::<NetEvent>(body) {
                    self.sync_js_snapshot();
                    if let Ok(effects) = self.js.dispatch_net_event(source_id, &ev) {
                        self.apply_script_effects(effects);
                    }
                }
            }
            _ => {}
        }
    }

    fn apply_script_effects(&mut self, effects: Vec<ScriptEffect>) {
        let mut hud_dirty_ids = Vec::new();

        for effect in effects {
            match effect {
                ScriptEffect::UpdatePlayerEconomy {
                    player_id,
                    cash,
                    bank,
                    job,
                } => {
                    if let Some(p) = self.players.values_mut().find(|p| p.id == player_id) {
                        if let Some(c) = cash {
                            p.cash = c;
                        }
                        if let Some(b) = bank {
                            p.bank = b;
                        }
                        if let Some(j) = job {
                            p.job = j;
                        }
                        hud_dirty_ids.push(player_id);
                    }
                }
                ScriptEffect::SendRpc { target_id, rpc } => {
                    if let Ok(bytes) = serde_json::to_vec(&rpc) {
                        self.route_reliable(target_id, REL_SERVER_RPC, &bytes);
                    }
                }
                ScriptEffect::SendChat { target_id, msg } => {
                    if let Ok(bytes) = serde_json::to_vec(&msg) {
                        self.route_reliable(target_id, REL_CHAT_MSG, &bytes);
                    }
                }
                ScriptEffect::ProximityChat {
                    source_id,
                    radius,
                    msg,
                } => {
                    if let Some(origin) = self.find_player_by_id(source_id).map(|p| p.state.pos) {
                        let bytes = serde_json::to_vec(&msg).unwrap_or_default();
                        let targets: Vec<u8> = self
                            .players
                            .values()
                            .filter(|p| p.state.distance_to(origin) <= radius)
                            .map(|p| p.id)
                            .collect();
                        for id in targets {
                            self.send_reliable_to_id(id, REL_CHAT_MSG, &bytes);
                        }
                    }
                }
                ScriptEffect::EmitNet { target_id, event } => {
                    if let Ok(bytes) = serde_json::to_vec(&event) {
                        self.route_reliable(target_id, REL_SERVER_EVENT, &bytes);
                    }
                }
            }
        }

        for id in hud_dirty_ids {
            self.push_hud_to_player(id);
        }
    }

    fn route_reliable(&mut self, target_id: i32, inner_kind: u8, payload: &[u8]) {
        if target_id < 0 {
            let ids: Vec<u8> = self.players.values().map(|p| p.id).collect();
            for id in ids {
                self.send_reliable_to_id(id, inner_kind, payload);
            }
        } else {
            self.send_reliable_to_id(target_id as u8, inner_kind, payload);
        }
    }

    fn send_reliable_to_id(&mut self, player_id: u8, inner_kind: u8, payload: &[u8]) {
        let Some(addr) = self.find_player_by_id(player_id).map(|p| p.addr) else {
            return;
        };
        self.next_rel_seq = self.next_rel_seq.wrapping_add(1);
        let seq = self.next_rel_seq;

        let mut body = Vec::with_capacity(3 + payload.len());
        body.extend(seq.to_le_bytes());
        body.push(inner_kind);
        body.extend_from_slice(payload);
        let packet = make_packet(PKT_REL, &body);

        let _ = self.sock.send_to(&packet, addr);
        let now = Instant::now();
        self.pending_rels.push(PendingRel {
            seq,
            target_addr: addr,
            packet,
            first_sent: now,
            last_sent: now,
        });
    }

    fn push_hud_to_player(&mut self, player_id: u8) {
        let online_count = self.players.len() as u16;
        let Some(hud) = self.find_player_by_id(player_id).map(|p| HudState {
            cash: p.cash,
            bank: p.bank,
            job: p.job.clone(),
            server_name: self.server_name.clone(),
            online_count,
        }) else {
            return;
        };
        if let Ok(bytes) = serde_json::to_vec(&hud) {
            self.send_reliable_to_id(player_id, REL_HUD_UPDATE, &bytes);
        }
    }

    fn push_hud_to_all(&mut self) {
        let ids: Vec<u8> = self.players.values().map(|p| p.id).collect();
        for id in ids {
            self.push_hud_to_player(id);
        }
    }

    fn find_player_by_id(&self, id: u8) -> Option<&ConnectedPlayer> {
        self.players.values().find(|p| p.id == id)
    }

    fn tick_reliable_resends(&mut self) {
        let sock = &self.sock;
        let active_addrs: std::collections::HashSet<SocketAddr> =
            self.players.keys().copied().collect();
        self.pending_rels.retain_mut(|item| {
            if !active_addrs.contains(&item.target_addr)
                || item.first_sent.elapsed() > REL_MAX_LIFETIME
            {
                return false;
            }
            if item.last_sent.elapsed() >= REL_RESEND_INTERVAL {
                item.last_sent = Instant::now();
                let _ = sock.send_to(&item.packet, item.target_addr);
            }
            true
        });
    }

    fn disconnect_player(&mut self, addr: SocketAddr, reason: &str) {
        if let Some(removed) = self.players.remove(&addr) {
            println!(
                "[rp-server] {} (ID {}) disconnected: {}",
                removed.name, removed.id, reason
            );
            for other_addr in self.players.keys() {
                let _ = self
                    .sock
                    .send_to(&make_packet(PKT_LEAVE, &[removed.id]), other_addr);
            }
            self.sync_js_snapshot();
            let _ = self.js.dispatch_server_event(
                "playerDropped",
                &[
                    serde_json::json!(removed.id),
                    serde_json::json!(removed.name),
                    serde_json::json!(reason),
                ],
            );
            self.push_hud_to_all();
        }
    }

    fn sweep_timeouts(&mut self) {
        let timed_out: Vec<SocketAddr> = self
            .players
            .iter()
            .filter(|(_, p)| p.last_seen.elapsed() > Duration::from_secs(TIMEOUT_SECS))
            .map(|(addr, _)| *addr)
            .collect();
        for addr in timed_out {
            self.disconnect_player(addr, "timeout");
        }
    }
}

fn main() -> Result<()> {
    let port = std::env::var("RP_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    let resources_path = std::env::var("RP_RESOURCES").unwrap_or_else(|_| "resources".to_string());
    let mut server = RpServer::new(port, MAX_PLAYERS_DEFAULT, Path::new(&resources_path))?;
    server.run();
    Ok(())
}
