//! Cross-platform Client Core for iOS (Metal), Android (Vulkan), macOS, and PC (DX12/Vulkan).
//!
//! Provides both:
//! 1. `RpRoleplaySession`: Lock-friendly shared-socket Roleplay engine that attaches directly onto
//!    the Wasm host's existing multiplayer `UdpSocket` + `GtaThread::Update` tick (handling
//!    `PKT_RP_REL = 28` / `PKT_RP_ACK = 29`, async vehicle/anim streaming, zero-NPC mobile RAM
//!    saver, and native GTA V HUD/Chat rendering with zero Chromium/CEF overhead).
//! 2. `RpClientCore`: Standalone UDP wrapper for headless testing or external NDK runners.

pub mod sync;

use anyhow::Result;
use rp_proto::*;
use std::collections::{HashMap, VecDeque};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

const MAX_CHAT_HISTORY: usize = 50;
const VISIBLE_CHAT_LINES: usize = 6;
const CHAT_FADE_DURATION: Duration = Duration::from_secs(14);
const CLIENT_RESEND_INTERVAL: Duration = Duration::from_millis(200);
const CLIENT_RESEND_LIFETIME: Duration = Duration::from_secs(4);

/// Trait implemented by the Wasmtime host runner (e.g. `Cmd` in `mp.rs`) to invoke `mp:<Name>` exports.
pub trait WasmGameBridge {
    fn player_ped_id(&mut self) -> i32;
    fn invoke_int(&mut self, command: &str, args: &[NativeArg]) -> i32;
    fn invoke_void(&mut self, command: &str, args: &[NativeArg]);
    fn has_model_loaded(&mut self, model_hash: u32) -> bool;
    fn request_model(&mut self, model_hash: u32);
    fn has_anim_dict_loaded(&mut self, dict: &str) -> bool;
    fn request_anim_dict(&mut self, dict: &str);

    /// Optional helper for native GTA V HUD text rendering (`STRING` + `AddTextComponentSubStringPlayerName`).
    fn draw_screen_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        scale: f32,
        rgba: [u8; 4],
        centre: bool,
    ) {
        self.invoke_void("SetTextFont", &[NativeArg::Int(4)]);
        self.invoke_void(
            "SetTextScale",
            &[NativeArg::Float(scale), NativeArg::Float(scale)],
        );
        self.invoke_void(
            "SetTextColour",
            &[
                NativeArg::Int(rgba[0] as i32),
                NativeArg::Int(rgba[1] as i32),
                NativeArg::Int(rgba[2] as i32),
                NativeArg::Int(rgba[3] as i32),
            ],
        );
        self.invoke_void("SetTextCentre", &[NativeArg::Bool(centre)]);
        self.invoke_void("SetTextOutline", &[]);
        self.invoke_void(
            "BeginTextCommandDisplayText",
            &[NativeArg::Str("STRING".to_string())],
        );
        self.invoke_void(
            "AddTextComponentSubStringPlayerName",
            &[NativeArg::Str(text.to_string())],
        );
        self.invoke_void(
            "EndTextCommandDisplayText",
            &[
                NativeArg::Float(x),
                NativeArg::Float(y),
                NativeArg::Int(0),
            ],
        );
    }

    /// Optional helper for native GTA V feed ticker notification above radar.
    fn push_feed_ticker(&mut self, text: &str) {
        self.invoke_void(
            "BeginTheFeedPost",
            &[NativeArg::Str("STRING".to_string())],
        );
        self.invoke_void(
            "AddTextComponentSubStringPlayerName",
            &[NativeArg::Str(text.to_string())],
        );
        self.invoke_void(
            "EndTheFeedPostTicker",
            &[NativeArg::Bool(false), NativeArg::Bool(true)],
        );
    }
}

/// Asynchronous multi-frame streaming tasks (e.g. waiting for `RequestModel` before `CreateVehicle`).
#[derive(Clone, Debug)]
enum StreamingJob {
    SpawnVehicle {
        model_hash: u32,
        pos: [f32; 3],
        heading: f32,
        put_in_driver_seat: bool,
        started_at: Instant,
    },
    PlayAnim {
        dict: String,
        clip: String,
        flags: i32,
        started_at: Instant,
    },
}

struct PendingClientRel {
    seq: u16,
    packet: Vec<u8>,
    first_sent: Instant,
    last_sent: Instant,
}

#[derive(Clone, Debug)]
pub struct TimedChatEntry {
    pub received_at: Instant,
    pub msg: ChatMessage,
}

/// Shared-socket Roleplay state machine that plugs directly into the Wasm host's `mp.rs`.
pub struct RpRoleplaySession {
    pub hud: HudState,
    pub chat_log: VecDeque<TimedChatEntry>,
    pub npc_traffic_disabled: bool,
    pub show_chat_pinned: bool,
    pub typing_buffer: Option<String>,
    last_spawned_veh: i32,
    rpc_queue: VecDeque<ServerRpc>,
    pending_tickers: VecDeque<String>,
    streaming_jobs: VecDeque<StreamingJob>,
    seen_server_seqs: VecDeque<u16>,
    next_client_seq: u16,
    pending_rels: Vec<PendingClientRel>,
}

impl Default for RpRoleplaySession {
    fn default() -> Self {
        Self::new()
    }
}

impl RpRoleplaySession {
    pub fn new() -> Self {
        Self {
            hud: HudState::default(),
            chat_log: VecDeque::new(),
            npc_traffic_disabled: true, // Default ON for iPhone 15 6 GB RAM safety
            show_chat_pinned: false,
            typing_buffer: None,
            last_spawned_veh: 0,
            rpc_queue: VecDeque::new(),
            pending_tickers: VecDeque::new(),
            streaming_jobs: VecDeque::new(),
            seen_server_seqs: VecDeque::new(),
            next_client_seq: 0,
            pending_rels: Vec::new(),
        }
    }

    /// Build a `PKT_HELLO` payload announcing player name + device platform (`Ios`, `Mac`, `Pc`, `Android`).
    pub fn build_hello_packet(name: &str, platform: Platform) -> Vec<u8> {
        let hello = HelloPayload {
            name: sanitize_name(name),
            platform,
        };
        let bytes = serde_json::to_vec(&hello).unwrap_or_else(|_| name.as_bytes().to_vec());
        make_packet(PKT_HELLO, &bytes)
    }

    /// Send a chat message or `/command` (e.g. `/car sultan`, `/tp legion`, `/job polisi`) over `sock`.
    pub fn send_chat_or_command(&mut self, sock: &UdpSocket, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let msg = ChatMessage {
            prefix: String::new(),
            text: trimmed.to_string(),
            color_rgb: [255, 255, 255],
        };
        if let Ok(bytes) = serde_json::to_vec(&msg) {
            self.send_reliable(sock, REL_CHAT_MSG, &bytes);
        }
    }

    /// Trigger a JavaScript `onNet(...)` event on the server from client UI/gameplay.
    pub fn emit_server_event(&mut self, sock: &UdpSocket, event_name: &str, json_payload: &str) {
        let ev = NetEvent {
            event_name: event_name.to_string(),
            json_payload: json_payload.to_string(),
        };
        if let Ok(bytes) = serde_json::to_vec(&ev) {
            self.send_reliable(sock, REL_CLIENT_EVENT, &bytes);
        }
    }

    fn send_reliable(&mut self, sock: &UdpSocket, inner_kind: u8, payload: &[u8]) {
        self.next_client_seq = self.next_client_seq.wrapping_add(1);
        let seq = self.next_client_seq;
        let mut body = Vec::with_capacity(3 + payload.len());
        body.extend(seq.to_le_bytes());
        body.push(inner_kind);
        body.extend_from_slice(payload);
        let packet = make_packet(PKT_RP_REL, &body);
        let _ = sock.send(&packet);
        let now = Instant::now();
        self.pending_rels.push(PendingClientRel {
            seq,
            packet,
            first_sent: now,
            last_sent: now,
        });
    }

    /// Retransmit unacknowledged client->server Roleplay commands/events.
    pub fn tick_resends(&mut self, sock: &UdpSocket) {
        self.pending_rels.retain_mut(|item| {
            if item.first_sent.elapsed() > CLIENT_RESEND_LIFETIME {
                return false;
            }
            if item.last_sent.elapsed() >= CLIENT_RESEND_INTERVAL {
                item.last_sent = Instant::now();
                let _ = sock.send(&item.packet);
            }
            true
        });
    }

    /// Handle an incoming `PKT_RP_REL` (`28`) or `PKT_RP_ACK` (`29`) packet from `RpServer`.
    pub fn handle_rp_packet(&mut self, sock: &UdpSocket, kind: u8, payload: &[u8]) -> bool {
        match kind {
            PKT_RP_ACK if payload.len() >= 2 => {
                let seq = u16::from_le_bytes([payload[0], payload[1]]);
                self.pending_rels.retain(|p| p.seq != seq);
                true
            }
            PKT_RP_REL if payload.len() >= 3 => {
                let seq = u16::from_le_bytes([payload[0], payload[1]]);
                let _ = sock.send(&make_packet(PKT_RP_ACK, &[payload[0], payload[1]]));
                if !self.seen_server_seqs.contains(&seq) {
                    self.seen_server_seqs.push_back(seq);
                    while self.seen_server_seqs.len() > 128 {
                        self.seen_server_seqs.pop_front();
                    }
                    self.handle_reliable_inner(payload[2], &payload[3..]);
                }
                true
            }
            _ => false,
        }
    }

    fn handle_reliable_inner(&mut self, inner_kind: u8, body: &[u8]) {
        match inner_kind {
            REL_CHAT_MSG => {
                if let Ok(msg) = serde_json::from_slice::<ChatMessage>(body) {
                    let ticker = if msg.prefix.is_empty() {
                        msg.text.clone()
                    } else {
                        format!("~b~{}~s~ {}", msg.prefix, msg.text)
                    };
                    if self.pending_tickers.len() < 8 {
                        self.pending_tickers.push_back(ticker);
                    }
                    self.chat_log.push_back(TimedChatEntry {
                        received_at: Instant::now(),
                        msg,
                    });
                    while self.chat_log.len() > MAX_CHAT_HISTORY {
                        self.chat_log.pop_front();
                    }
                }
            }
            REL_HUD_UPDATE => {
                if let Ok(hud) = serde_json::from_slice::<HudState>(body) {
                    self.hud = hud;
                }
            }
            REL_SERVER_RPC => {
                if let Ok(rpc) = serde_json::from_slice::<ServerRpc>(body) {
                    self.rpc_queue.push_back(rpc);
                }
            }
            _ => {}
        }
    }

    /// Execute Roleplay RPCs, Zero-NPC Mobile RAM Saver, and lightweight native GTA V HUD/Chat inside `GtaThread::Update`.
    pub fn execute_script_tick(&mut self, game: &mut dyn WasmGameBridge) {
        // 1. Mobile RAM & FPS Saver: suppress local ambient NPCs & random traffic every frame
        if self.npc_traffic_disabled {
            game.invoke_void(
                "SetPedDensityMultiplierThisFrame",
                &[NativeArg::Float(0.0)],
            );
            game.invoke_void(
                "SetScenarioPedDensityMultiplierThisFrame",
                &[NativeArg::Float(0.0), NativeArg::Float(0.0)],
            );
            game.invoke_void(
                "SetVehicleDensityMultiplierThisFrame",
                &[NativeArg::Float(0.0)],
            );
            game.invoke_void(
                "SetRandomVehicleDensityMultiplierThisFrame",
                &[NativeArg::Float(0.0)],
            );
            game.invoke_void(
                "SetParkedVehicleDensityMultiplierThisFrame",
                &[NativeArg::Float(0.0)],
            );
        }

        let ped = game.player_ped_id();
        if ped == 0 {
            return;
        }

        // 2. Display native feed tickers for newly arrived chat/system notifications
        while let Some(ticker) = self.pending_tickers.pop_front() {
            game.push_feed_ticker(&ticker);
        }

        // 3. Process incoming Server RPCs
        while let Some(rpc) = self.rpc_queue.pop_front() {
            match rpc {
                ServerRpc::SetNpcTrafficDisabled(disabled) => {
                    self.npc_traffic_disabled = disabled;
                }
                ServerRpc::Teleport { pos, heading } => {
                    // Teleport the vehicle if local player is driving, otherwise teleport ped directly
                    let in_veh =
                        game.invoke_int("IsPedInAnyVehicle", &[NativeArg::Int(ped), NativeArg::Bool(false)])
                            != 0;
                    let target_ent = if in_veh {
                        let v = game.invoke_int(
                            "GetVehiclePedIsIn",
                            &[NativeArg::Int(ped), NativeArg::Bool(false)],
                        );
                        let driver = game.invoke_int(
                            "GetPedInVehicleSeat",
                            &[NativeArg::Int(v), NativeArg::Int(-1), NativeArg::Bool(false)],
                        );
                        if v != 0 && driver == ped { v } else { ped }
                    } else {
                        ped
                    };
                    game.invoke_void(
                        "SetEntityCoordsNoOffset",
                        &[
                            NativeArg::Int(target_ent),
                            NativeArg::Vec3(pos),
                            NativeArg::Bool(false),
                            NativeArg::Bool(false),
                            NativeArg::Bool(false),
                        ],
                    );
                    if let Some(h) = heading {
                        game.invoke_void(
                            "SetEntityHeading",
                            &[NativeArg::Int(target_ent), NativeArg::Float(h)],
                        );
                    }
                }
                ServerRpc::SpawnVehicle {
                    model_hash,
                    pos,
                    heading,
                    put_in_driver_seat,
                    ..
                } => {
                    game.request_model(model_hash);
                    self.streaming_jobs.push_back(StreamingJob::SpawnVehicle {
                        model_hash,
                        pos,
                        heading,
                        put_in_driver_seat,
                        started_at: Instant::now(),
                    });
                }
                ServerRpc::PlayAnim { dict, clip, flags } => {
                    game.request_anim_dict(&dict);
                    self.streaming_jobs.push_back(StreamingJob::PlayAnim {
                        dict,
                        clip,
                        flags,
                        started_at: Instant::now(),
                    });
                }
                ServerRpc::ClearAnim => {
                    game.invoke_void("ClearPedTasks", &[NativeArg::Int(ped)]);
                }
                ServerRpc::GiveWeapon { weapon_hash, ammo } => {
                    game.invoke_void(
                        "GiveWeaponToPed",
                        &[
                            NativeArg::Int(ped),
                            NativeArg::Int(weapon_hash as i32),
                            NativeArg::Int(ammo),
                            NativeArg::Bool(false),
                            NativeArg::Bool(true),
                        ],
                    );
                    game.invoke_void(
                        "SetCurrentPedWeapon",
                        &[
                            NativeArg::Int(ped),
                            NativeArg::Int(weapon_hash as i32),
                            NativeArg::Bool(true),
                        ],
                    );
                }
                ServerRpc::SetWeather(w) => {
                    game.invoke_void("SetWeatherTypeNowPersist", &[NativeArg::Str(w)]);
                }
                ServerRpc::SetClock {
                    hour,
                    minute,
                    second,
                } => {
                    game.invoke_void(
                        "SetClockTime",
                        &[
                            NativeArg::Int(hour as i32),
                            NativeArg::Int(minute as i32),
                            NativeArg::Int(second as i32),
                        ],
                    );
                }
                ServerRpc::InvokeNative { command, args } => {
                    game.invoke_void(&command, &args);
                }
            }
        }

        // 4. Advance asynchronous model/animation streaming jobs
        let mut remaining_jobs = VecDeque::new();
        while let Some(job) = self.streaming_jobs.pop_front() {
            match job {
                StreamingJob::SpawnVehicle {
                    model_hash,
                    pos,
                    heading,
                    put_in_driver_seat,
                    started_at,
                } => {
                    if game.has_model_loaded(model_hash) {
                        let veh = game.invoke_int(
                            "CreateVehicle",
                            &[
                                NativeArg::Int(model_hash as i32),
                                NativeArg::Vec3(pos),
                                NativeArg::Float(heading),
                                NativeArg::Bool(false),
                                NativeArg::Bool(false),
                                NativeArg::Bool(false),
                            ],
                        );
                        if veh != 0 {
                            game.invoke_void(
                                "SetVehicleEngineOn",
                                &[
                                    NativeArg::Int(veh),
                                    NativeArg::Bool(true),
                                    NativeArg::Bool(true),
                                    NativeArg::Bool(false),
                                ],
                            );
                            game.invoke_void(
                                "SetVehicleNumberPlateText",
                                &[NativeArg::Int(veh), NativeArg::Str("INDORP".to_string())],
                            );
                            if put_in_driver_seat {
                                game.invoke_void(
                                    "SetPedIntoVehicle",
                                    &[
                                        NativeArg::Int(ped),
                                        NativeArg::Int(veh),
                                        NativeArg::Int(-1),
                                    ],
                                );
                            }
                            self.last_spawned_veh = veh;
                        }
                    } else if started_at.elapsed() < Duration::from_secs(5) {
                        game.request_model(model_hash);
                        remaining_jobs.push_back(StreamingJob::SpawnVehicle {
                            model_hash,
                            pos,
                            heading,
                            put_in_driver_seat,
                            started_at,
                        });
                    }
                }
                StreamingJob::PlayAnim {
                    dict,
                    clip,
                    flags,
                    started_at,
                } => {
                    if game.has_anim_dict_loaded(&dict) {
                        game.invoke_void(
                            "TaskPlayAnim",
                            &[
                                NativeArg::Int(ped),
                                NativeArg::Str(dict),
                                NativeArg::Str(clip),
                                NativeArg::Float(8.0),
                                NativeArg::Float(-8.0),
                                NativeArg::Int(-1),
                                NativeArg::Int(flags),
                                NativeArg::Float(0.0),
                                NativeArg::Bool(false),
                                NativeArg::Int(0),
                                NativeArg::Bool(false),
                            ],
                        );
                    } else if started_at.elapsed() < Duration::from_secs(5) {
                        game.request_anim_dict(&dict);
                        remaining_jobs.push_back(StreamingJob::PlayAnim {
                            dict,
                            clip,
                            flags,
                            started_at,
                        });
                    }
                }
            }
        }
        self.streaming_jobs = remaining_jobs;

        // 5. Render ultra-lightweight native GTA V Roleplay HUD (zero HTML/CEF RAM overhead)
        self.render_native_hud(game);
    }

    fn render_native_hud(&self, game: &mut dyn WasmGameBridge) {
        if !self.hud.server_name.is_empty() {
            // Top-center subtle Roleplay status bar (safe from touch buttons on top-left & top-right)
            game.invoke_void(
                "DrawRect",
                &[
                    NativeArg::Float(0.50),
                    NativeArg::Float(0.032),
                    NativeArg::Float(0.36),
                    NativeArg::Float(0.044),
                    NativeArg::Int(10),
                    NativeArg::Int(14),
                    NativeArg::Int(22),
                    NativeArg::Int(155),
                    NativeArg::Bool(false),
                ],
            );
            let title_line = format!(
                "{}  |  Warga: {}",
                self.hud.server_name, self.hud.online_count
            );
            game.draw_screen_text(&title_line, 0.50, 0.011, 0.28, [120, 220, 255, 240], true);

            let econ_line = format!(
                "{}   Tunai: ${}   Bank: ${}",
                self.hud.job, self.hud.cash, self.hud.bank
            );
            game.draw_screen_text(&econ_line, 0.50, 0.031, 0.29, [135, 255, 155, 245], true);
        }

        // Recent Roleplay chat lines (left side below touch buttons)
        let typing_active = self.typing_buffer.is_some();
        let recent: Vec<&TimedChatEntry> = self
            .chat_log
            .iter()
            .rev()
            .filter(|e| {
                typing_active
                    || self.show_chat_pinned
                    || e.received_at.elapsed() < CHAT_FADE_DURATION
            })
            .take(VISIBLE_CHAT_LINES)
            .collect();

        for (idx, entry) in recent.into_iter().rev().enumerate() {
            let y = 0.14 + (idx as f32) * 0.026;
            let line = if entry.msg.prefix.is_empty() {
                entry.msg.text.clone()
            } else {
                format!("{} {}", entry.msg.prefix, entry.msg.text)
            };
            let truncated: String = line.chars().take(58).collect();
            let c = entry.msg.color_rgb;
            game.draw_screen_text(&truncated, 0.025, y, 0.29, [c[0], c[1], c[2], 235], false);
        }

        // Desktop live chat typing bar (KeyT)
        if let Some(buf) = &self.typing_buffer {
            game.invoke_void(
                "DrawRect",
                &[
                    NativeArg::Float(0.22),
                    NativeArg::Float(0.325),
                    NativeArg::Float(0.40),
                    NativeArg::Float(0.034),
                    NativeArg::Int(0),
                    NativeArg::Int(0),
                    NativeArg::Int(0),
                    NativeArg::Int(190),
                    NativeArg::Bool(false),
                ],
            );
            let prompt: String = format!("CHAT/RP > {}_", buf).chars().take(58).collect();
            game.draw_screen_text(&prompt, 0.025, 0.312, 0.31, [255, 230, 120, 255], false);
        }
    }
}

pub struct RemotePeer {
    pub id: u8,
    pub name: String,
    pub snapshots: VecDeque<(Instant, PlayerSyncState)>,
    pub last_seen: Instant,
}

/// Standalone UDP Roleplay client (for headless integration tests, bots, or standalone NDK).
pub struct RpClientCore {
    sock: UdpSocket,
    pub player_id: Option<u8>,
    pub player_name: String,
    pub platform: Platform,
    pub session: RpRoleplaySession,
    pub peers: HashMap<u8, RemotePeer>,
    last_hello: Instant,
    seq_counter: u32,
}

impl RpClientCore {
    pub fn connect(server_addr: &str, player_name: &str, platform: Platform) -> Result<Self> {
        let target = if server_addr.contains(':') {
            server_addr.to_string()
        } else {
            format!("{server_addr}:{DEFAULT_PORT}")
        };
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        sock.connect(&target)?;
        sock.set_nonblocking(true)?;

        Ok(Self {
            sock,
            player_id: None,
            player_name: sanitize_name(player_name),
            platform,
            session: RpRoleplaySession::new(),
            peers: HashMap::new(),
            last_hello: Instant::now() - Duration::from_secs(10),
            seq_counter: 0,
        })
    }

    pub fn send_chat_or_command(&mut self, text: &str) {
        self.session.send_chat_or_command(&self.sock, text);
    }

    pub fn emit_server_event(&mut self, event_name: &str, json_payload: &str) {
        self.session
            .emit_server_event(&self.sock, event_name, json_payload);
    }

    pub fn send_local_state(&mut self, state: &PlayerSyncState) {
        if self.player_id.is_none() {
            return;
        }
        self.seq_counter = self.seq_counter.wrapping_add(1);
        let mut payload = Vec::with_capacity(4 + SYNC_STATE_SIZE);
        payload.extend(self.seq_counter.to_le_bytes());
        state.encode(&mut payload);
        let _ = self.sock.send(&make_packet(PKT_STATE, &payload));
    }

    pub fn poll_network(&mut self) {
        let hello_interval = if self.player_id.is_none() { 1 } else { 3 };
        if self.last_hello.elapsed() >= Duration::from_secs(hello_interval) {
            self.last_hello = Instant::now();
            let pkt = RpRoleplaySession::build_hello_packet(&self.player_name, self.platform);
            let _ = self.sock.send(&pkt);
        }
        self.session.tick_resends(&self.sock);

        let mut buf = [0u8; 2048];
        while let Ok(n) = self.sock.recv(&mut buf) {
            let Some((kind, payload)) = parse_packet(&buf[..n]) else {
                continue;
            };
            if self.session.handle_rp_packet(&self.sock, kind, payload) {
                continue;
            }
            match kind {
                PKT_WELCOME if !payload.is_empty() => {
                    self.player_id = Some(payload[0]);
                }
                PKT_NAME if !payload.is_empty() => {
                    let peer_id = payload[0];
                    let name = sanitize_name(&String::from_utf8_lossy(&payload[1..]));
                    let now = Instant::now();
                    let peer = self.peers.entry(peer_id).or_insert_with(|| RemotePeer {
                        id: peer_id,
                        name: String::new(),
                        snapshots: VecDeque::new(),
                        last_seen: now,
                    });
                    peer.name = name;
                    peer.last_seen = now;
                }
                PKT_STATE if payload.len() >= 5 + SYNC_STATE_SIZE => {
                    let peer_id = payload[0];
                    if let Some(state) = PlayerSyncState::decode(&payload[5..]) {
                        let now = Instant::now();
                        let peer = self.peers.entry(peer_id).or_insert_with(|| RemotePeer {
                            id: peer_id,
                            name: format!("Warga {peer_id}"),
                            snapshots: VecDeque::new(),
                            last_seen: now,
                        });
                        peer.last_seen = now;
                        peer.snapshots.push_back((now, state));
                        while peer.snapshots.len() > 20 {
                            peer.snapshots.pop_front();
                        }
                    }
                }
                PKT_LEAVE if !payload.is_empty() => {
                    if payload[0] == 0 {
                        self.player_id = None;
                    } else {
                        self.peers.remove(&payload[0]);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn execute_script_tick(&mut self, game: &mut dyn WasmGameBridge) {
        self.session.execute_script_tick(game);
    }
}
