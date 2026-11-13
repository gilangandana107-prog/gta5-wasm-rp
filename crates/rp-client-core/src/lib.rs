//! Cross-platform Client Core for iOS (Metal), Android (Vulkan), and PC (DX12/Vulkan).
//!
//! Designed to plug directly into any `wasmtime` + `wgpu` runner (`rp_script_tick` in `GtaThread::Update`).
//! Keeps zero OS-specific dependencies so the exact same crate compiles on Apple, Android, Windows, & Linux.

use anyhow::Result;
use rp_proto::*;
use std::collections::{HashMap, VecDeque};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

const MAX_CHAT_HISTORY: usize = 50;

/// Trait implemented by the Wasmtime host runner (like Muguet's `Cmd`) to invoke `mp:<Name>` exports.
pub trait WasmGameBridge {
    fn player_ped_id(&mut self) -> i32;
    fn invoke_int(&mut self, command: &str, args: &[NativeArg]) -> i32;
    fn invoke_void(&mut self, command: &str, args: &[NativeArg]);
    fn has_model_loaded(&mut self, model_hash: u32) -> bool;
    fn request_model(&mut self, model_hash: u32);
    fn has_anim_dict_loaded(&mut self, dict: &str) -> bool;
    fn request_anim_dict(&mut self, dict: &str);
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

pub struct RemotePeer {
    pub id: u8,
    pub snapshots: VecDeque<(Instant, PlayerSyncState)>,
    pub last_seen: Instant,
}

pub struct RpClientCore {
    sock: UdpSocket,
    pub player_id: Option<u8>,
    pub player_name: String,
    pub platform: Platform,
    pub hud: HudState,
    pub chat_log: VecDeque<ChatMessage>,
    pub peers: HashMap<u8, RemotePeer>,
    pub npc_traffic_disabled: bool,
    rpc_queue: VecDeque<ServerRpc>,
    streaming_jobs: VecDeque<StreamingJob>,
    seen_server_seqs: VecDeque<u16>,
    next_client_seq: u16,
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
            hud: HudState::default(),
            chat_log: VecDeque::new(),
            peers: HashMap::new(),
            npc_traffic_disabled: true, // Default ON for mobile RAM safety
            rpc_queue: VecDeque::new(),
            streaming_jobs: VecDeque::new(),
            seen_server_seqs: VecDeque::new(),
            next_client_seq: 0,
            last_hello: Instant::now() - Duration::from_secs(10),
            seq_counter: 0,
        })
    }

    /// Send chat message or `/command` (e.g. `/car police`, `/me tersenyum`) to the RP server.
    pub fn send_chat_or_command(&mut self, text: &str) {
        let msg = ChatMessage {
            prefix: String::new(),
            text: text.to_string(),
            color_rgb: [255, 255, 255],
        };
        if let Ok(bytes) = serde_json::to_vec(&msg) {
            self.send_reliable(REL_CHAT_MSG, &bytes);
        }
    }

    /// Trigger a JavaScript `onNet(...)` event on the server from client UI/gameplay.
    pub fn emit_server_event(&mut self, event_name: &str, json_payload: &str) {
        let ev = NetEvent {
            event_name: event_name.to_string(),
            json_payload: json_payload.to_string(),
        };
        if let Ok(bytes) = serde_json::to_vec(&ev) {
            self.send_reliable(REL_CLIENT_EVENT, &bytes);
        }
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

    fn send_reliable(&mut self, inner_kind: u8, payload: &[u8]) {
        self.next_client_seq = self.next_client_seq.wrapping_add(1);
        let mut body = Vec::with_capacity(3 + payload.len());
        body.extend(self.next_client_seq.to_le_bytes());
        body.push(inner_kind);
        body.extend_from_slice(payload);
        let _ = self.sock.send(&make_packet(PKT_REL, &body));
    }

    /// Non-blocking network poll called once per frame.
    pub fn poll_network(&mut self) {
        let hello_interval = if self.player_id.is_none() { 1 } else { 3 };
        if self.last_hello.elapsed() >= Duration::from_secs(hello_interval) {
            self.last_hello = Instant::now();
            let hello = HelloPayload {
                name: self.player_name.clone(),
                platform: self.platform,
            };
            if let Ok(bytes) = serde_json::to_vec(&hello) {
                let _ = self.sock.send(&make_packet(PKT_HELLO, &bytes));
            }
        }

        let mut buf = [0u8; 2048];
        while let Ok(n) = self.sock.recv(&mut buf) {
            let Some((kind, payload)) = parse_packet(&buf[..n]) else {
                continue;
            };
            match kind {
                PKT_WELCOME if !payload.is_empty() => {
                    self.player_id = Some(payload[0]);
                }
                PKT_STATE if payload.len() >= 5 + SYNC_STATE_SIZE => {
                    let peer_id = payload[0];
                    if let Some(state) = PlayerSyncState::decode(&payload[5..]) {
                        let now = Instant::now();
                        let peer = self.peers.entry(peer_id).or_insert_with(|| RemotePeer {
                            id: peer_id,
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
                PKT_REL if payload.len() >= 3 => {
                    let seq = u16::from_le_bytes([payload[0], payload[1]]);
                    let _ = self
                        .sock
                        .send(&make_packet(PKT_ACK, &[payload[0], payload[1]]));
                    if !self.seen_server_seqs.contains(&seq) {
                        self.seen_server_seqs.push_back(seq);
                        while self.seen_server_seqs.len() > 128 {
                            self.seen_server_seqs.pop_front();
                        }
                        self.handle_reliable(payload[2], &payload[3..]);
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_reliable(&mut self, inner_kind: u8, body: &[u8]) {
        match inner_kind {
            REL_CHAT_MSG => {
                if let Ok(msg) = serde_json::from_slice::<ChatMessage>(body) {
                    self.chat_log.push_back(msg);
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

    /// Called every frame inside `rp_script_tick` (`GtaThread::Update`) to apply Roleplay RPCs
    /// and enforce mobile RAM/CPU optimizations inside `game.wasm`.
    pub fn execute_script_tick(&mut self, game: &mut dyn WasmGameBridge) {
        // 1. Mobile RAM & FPS Saver: suppress local ambient NPCs & random cars every frame
        if self.npc_traffic_disabled {
            game.invoke_void("SetPedDensityMultiplierThisFrame", &[NativeArg::Float(0.0)]);
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

        // 2. Process incoming Server RPCs
        while let Some(rpc) = self.rpc_queue.pop_front() {
            match rpc {
                ServerRpc::SetNpcTrafficDisabled(disabled) => {
                    self.npc_traffic_disabled = disabled;
                }
                ServerRpc::Teleport { pos, heading } => {
                    game.invoke_void(
                        "SetEntityCoordsNoOffset",
                        &[
                            NativeArg::Int(ped),
                            NativeArg::Vec3(pos),
                            NativeArg::Bool(false),
                            NativeArg::Bool(false),
                            NativeArg::Bool(false),
                        ],
                    );
                    if let Some(h) = heading {
                        game.invoke_void(
                            "SetEntityHeading",
                            &[NativeArg::Int(ped), NativeArg::Float(h)],
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

        // 3. Advance asynchronous model/animation streaming jobs
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
                        if veh != 0 && put_in_driver_seat {
                            game.invoke_void(
                                "SetPedIntoVehicle",
                                &[
                                    NativeArg::Int(ped),
                                    NativeArg::Int(veh),
                                    NativeArg::Int(-1),
                                ],
                            );
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
    }
}

