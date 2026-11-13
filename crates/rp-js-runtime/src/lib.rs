//! Embedded JavaScript Resource Engine (`boa_engine`) with FiveM-like server scripting APIs.

use anyhow::{anyhow, Result};
use boa_engine::{js_string, Context, JsResult, JsString, JsValue, NativeFunction, Source};
use rp_proto::{joaat, ChatMessage, NativeArg, NetEvent, Platform, ServerRpc};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const PRELUDE_JS: &str = include_str!("prelude.js");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsResourceManifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    pub server_scripts: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct PlayerScriptView {
    pub id: u8,
    pub name: String,
    pub platform: Platform,
    pub pos: [f32; 3],
    pub cash: i64,
    pub bank: i64,
    pub job: String,
}

/// Outbound side-effects produced by JavaScript execution for the UDP server to apply/transmit.
#[derive(Clone, Debug)]
pub enum ScriptEffect {
    SendRpc {
        target_id: i32, // -1 = broadcast all
        rpc: ServerRpc,
    },
    SendChat {
        target_id: i32, // -1 = broadcast all
        msg: ChatMessage,
    },
    ProximityChat {
        source_id: u8,
        radius: f32,
        msg: ChatMessage,
    },
    EmitNet {
        target_id: i32, // -1 = broadcast all
        event: NetEvent,
    },
    UpdatePlayerEconomy {
        player_id: u8,
        cash: Option<i64>,
        bank: Option<i64>,
        job: Option<String>,
    },
}

#[derive(Default)]
struct HostState {
    players: HashMap<u8, PlayerScriptView>,
    registered_commands: HashSet<String>,
    effects: Vec<ScriptEffect>,
}

thread_local! {
    static ACTIVE_HOST: RefCell<HostState> = RefCell::new(HostState::default());
}

#[derive(Deserialize)]
#[serde(tag = "op")]
enum HostRequest {
    #[serde(rename = "log")]
    Log { level: String, message: String },
    #[serde(rename = "registerCommand")]
    RegisterCommand { name: String },
    #[serde(rename = "getPlayers")]
    GetPlayers,
    #[serde(rename = "getPlayerInfo")]
    GetPlayerInfo {
        #[serde(rename = "playerId")]
        player_id: u8,
    },
    #[serde(rename = "setEconomy")]
    SetEconomy {
        #[serde(rename = "playerId")]
        player_id: u8,
        cash: Option<i64>,
        bank: Option<i64>,
    },
    #[serde(rename = "setJob")]
    SetJob {
        #[serde(rename = "playerId")]
        player_id: u8,
        job: String,
    },
    #[serde(rename = "sendChat")]
    SendChat {
        #[serde(rename = "targetId")]
        target_id: i32,
        prefix: String,
        text: String,
        color: [u8; 3],
    },
    #[serde(rename = "proximityChat")]
    ProximityChat {
        #[serde(rename = "sourceId")]
        source_id: u8,
        radius: f32,
        prefix: String,
        text: String,
        color: [u8; 3],
    },
    #[serde(rename = "emitNet")]
    EmitNet {
        #[serde(rename = "eventName")]
        event_name: String,
        #[serde(rename = "targetId")]
        target_id: i32,
        #[serde(rename = "jsonPayload")]
        json_payload: String,
    },
    #[serde(rename = "disableNpcTraffic")]
    DisableNpcTraffic {
        #[serde(rename = "targetId")]
        target_id: i32,
        disabled: bool,
    },
    #[serde(rename = "teleport")]
    Teleport {
        #[serde(rename = "targetId")]
        target_id: i32,
        pos: [f32; 3],
        heading: Option<f32>,
    },
    #[serde(rename = "spawnVehicle")]
    SpawnVehicle {
        #[serde(rename = "targetId")]
        target_id: i32,
        #[serde(rename = "modelName")]
        model_name: String,
        pos: [f32; 3],
        heading: f32,
        #[serde(rename = "warpIn")]
        warp_in: bool,
    },
    #[serde(rename = "playAnim")]
    PlayAnim {
        #[serde(rename = "targetId")]
        target_id: i32,
        dict: String,
        clip: String,
        flags: i32,
    },
    #[serde(rename = "clearAnim")]
    ClearAnim {
        #[serde(rename = "targetId")]
        target_id: i32,
    },
    #[serde(rename = "giveWeapon")]
    GiveWeapon {
        #[serde(rename = "targetId")]
        target_id: i32,
        #[serde(rename = "weaponName")]
        weapon_name: String,
        ammo: i32,
    },
    #[serde(rename = "setWeather")]
    SetWeather {
        #[serde(rename = "targetId")]
        target_id: i32,
        weather: String,
    },
    #[serde(rename = "invokeNative")]
    InvokeNative {
        #[serde(rename = "targetId")]
        target_id: i32,
        command: String,
        args: Vec<serde_json::Value>,
    },
}

fn to_js_string(s: &str) -> JsString {
    let utf16: Vec<u16> = s.encode_utf16().collect();
    JsString::from(utf16.as_slice())
}

fn json_to_native_arg(v: &serde_json::Value) -> NativeArg {
    match v {
        serde_json::Value::Bool(b) => NativeArg::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                NativeArg::Int(i as i32)
            } else {
                NativeArg::Float(n.as_f64().unwrap_or(0.0) as f32)
            }
        }
        serde_json::Value::String(s) => NativeArg::Str(s.clone()),
        serde_json::Value::Array(arr) if arr.len() == 3 => NativeArg::Vec3([
            arr[0].as_f64().unwrap_or(0.0) as f32,
            arr[1].as_f64().unwrap_or(0.0) as f32,
            arr[2].as_f64().unwrap_or(0.0) as f32,
        ]),
        _ => NativeArg::Int(0),
    }
}

fn js_host_bridge(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let raw_req = match args.first() {
        Some(v) => v.to_string(ctx)?.to_std_string_escaped(),
        None => return Ok(JsValue::from(js_string!("{}"))),
    };

    let parsed: Result<HostRequest, _> = serde_json::from_str(&raw_req);
    let response_json = ACTIVE_HOST.with(|cell| {
        let mut state = cell.borrow_mut();
        match parsed {
            Ok(HostRequest::Log { level, message }) => {
                println!("[JS:{level}] {message}");
                "{}".to_string()
            }
            Ok(HostRequest::RegisterCommand { name }) => {
                state.registered_commands.insert(name);
                "{}".to_string()
            }
            Ok(HostRequest::GetPlayers) => {
                let ids: Vec<u8> = state.players.keys().copied().collect();
                serde_json::json!({ "players": ids }).to_string()
            }
            Ok(HostRequest::GetPlayerInfo { player_id }) => {
                if let Some(p) = state.players.get(&player_id) {
                    serde_json::json!({
                        "name": p.name,
                        "platform": format!("{:?}", p.platform),
                        "pos": p.pos,
                        "cash": p.cash,
                        "bank": p.bank,
                        "job": p.job,
                    })
                    .to_string()
                } else {
                    "{}".to_string()
                }
            }
            Ok(HostRequest::SetEconomy {
                player_id,
                cash,
                bank,
            }) => {
                if let Some(p) = state.players.get_mut(&player_id) {
                    if let Some(c) = cash {
                        p.cash = c;
                    }
                    if let Some(b) = bank {
                        p.bank = b;
                    }
                }
                state.effects.push(ScriptEffect::UpdatePlayerEconomy {
                    player_id,
                    cash,
                    bank,
                    job: None,
                });
                "{}".to_string()
            }
            Ok(HostRequest::SetJob { player_id, job }) => {
                if let Some(p) = state.players.get_mut(&player_id) {
                    p.job = job.clone();
                }
                state.effects.push(ScriptEffect::UpdatePlayerEconomy {
                    player_id,
                    cash: None,
                    bank: None,
                    job: Some(job),
                });
                "{}".to_string()
            }
            Ok(HostRequest::SendChat {
                target_id,
                prefix,
                text,
                color,
            }) => {
                state.effects.push(ScriptEffect::SendChat {
                    target_id,
                    msg: ChatMessage {
                        prefix,
                        text,
                        color_rgb: color,
                    },
                });
                "{}".to_string()
            }
            Ok(HostRequest::ProximityChat {
                source_id,
                radius,
                prefix,
                text,
                color,
            }) => {
                state.effects.push(ScriptEffect::ProximityChat {
                    source_id,
                    radius,
                    msg: ChatMessage {
                        prefix,
                        text,
                        color_rgb: color,
                    },
                });
                "{}".to_string()
            }
            Ok(HostRequest::EmitNet {
                event_name,
                target_id,
                json_payload,
            }) => {
                state.effects.push(ScriptEffect::EmitNet {
                    target_id,
                    event: NetEvent {
                        event_name,
                        json_payload,
                    },
                });
                "{}".to_string()
            }
            Ok(HostRequest::DisableNpcTraffic {
                target_id,
                disabled,
            }) => {
                state.effects.push(ScriptEffect::SendRpc {
                    target_id,
                    rpc: ServerRpc::SetNpcTrafficDisabled(disabled),
                });
                "{}".to_string()
            }
            Ok(HostRequest::Teleport {
                target_id,
                pos,
                heading,
            }) => {
                state.effects.push(ScriptEffect::SendRpc {
                    target_id,
                    rpc: ServerRpc::Teleport { pos, heading },
                });
                "{}".to_string()
            }
            Ok(HostRequest::SpawnVehicle {
                target_id,
                model_name,
                pos,
                heading,
                warp_in,
            }) => {
                let model_hash = joaat(&model_name);
                state.effects.push(ScriptEffect::SendRpc {
                    target_id,
                    rpc: ServerRpc::SpawnVehicle {
                        model_hash,
                        model_name,
                        pos,
                        heading,
                        put_in_driver_seat: warp_in,
                    },
                });
                "{}".to_string()
            }
            Ok(HostRequest::PlayAnim {
                target_id,
                dict,
                clip,
                flags,
            }) => {
                state.effects.push(ScriptEffect::SendRpc {
                    target_id,
                    rpc: ServerRpc::PlayAnim { dict, clip, flags },
                });
                "{}".to_string()
            }
            Ok(HostRequest::ClearAnim { target_id }) => {
                state.effects.push(ScriptEffect::SendRpc {
                    target_id,
                    rpc: ServerRpc::ClearAnim,
                });
                "{}".to_string()
            }
            Ok(HostRequest::GiveWeapon {
                target_id,
                weapon_name,
                ammo,
            }) => {
                let normalized = if weapon_name.to_ascii_lowercase().starts_with("weapon_") {
                    weapon_name
                } else {
                    format!("weapon_{weapon_name}")
                };
                state.effects.push(ScriptEffect::SendRpc {
                    target_id,
                    rpc: ServerRpc::GiveWeapon {
                        weapon_hash: joaat(&normalized),
                        ammo,
                    },
                });
                "{}".to_string()
            }
            Ok(HostRequest::SetWeather { target_id, weather }) => {
                state.effects.push(ScriptEffect::SendRpc {
                    target_id,
                    rpc: ServerRpc::SetWeather(weather.to_ascii_uppercase()),
                });
                "{}".to_string()
            }
            Ok(HostRequest::InvokeNative {
                target_id,
                command,
                args,
            }) => {
                let native_args = args.iter().map(json_to_native_arg).collect();
                state.effects.push(ScriptEffect::SendRpc {
                    target_id,
                    rpc: ServerRpc::InvokeNative {
                        command,
                        args: native_args,
                    },
                });
                "{}".to_string()
            }
            Err(e) => {
                eprintln!("[rp-js-runtime] Invalid host call payload: {e}");
                "{}".to_string()
            }
        }
    });

    Ok(JsValue::from(to_js_string(&response_json)))
}

pub struct JsRoleplayEngine {
    ctx: Context,
}

impl JsRoleplayEngine {
    pub fn new() -> Result<Self> {
        let mut ctx = Context::default();
        ctx.register_global_builtin_callable(
            js_string!("__rp_host_call"),
            1,
            NativeFunction::from_fn_ptr(js_host_bridge),
        )
        .map_err(|e| anyhow!("Failed to register __rp_host_call: {e}"))?;

        ctx.eval(Source::from_bytes(PRELUDE_JS.as_bytes()))
            .map_err(|e| anyhow!("Failed to initialize JS prelude: {e}"))?;

        Ok(Self { ctx })
    }

    pub fn eval_script(&mut self, script_name: &str, code: &str) -> Result<Vec<ScriptEffect>> {
        ACTIVE_HOST.with(|cell| cell.borrow_mut().effects.clear());
        self.ctx
            .eval(Source::from_bytes(code.as_bytes()))
            .map_err(|e| anyhow!("JS error in {script_name}: {e}"))?;
        Ok(Self::drain_effects())
    }

    pub fn load_resource_dir(&mut self, dir: &Path) -> Result<JsResourceManifest> {
        let manifest_path = dir.join("fxmanifest.json");
        let raw = std::fs::read_to_string(&manifest_path)
            .map_err(|e| anyhow!("Missing {}: {e}", manifest_path.display()))?;
        let manifest: JsResourceManifest = serde_json::from_str(&raw)?;
        for rel_script in &manifest.server_scripts {
            let script_path = dir.join(rel_script);
            let code = std::fs::read_to_string(&script_path)
                .map_err(|e| anyhow!("Cannot read {}: {e}", script_path.display()))?;
            self.eval_script(&format!("{}/{}", manifest.name, rel_script), &code)?;
        }
        Ok(manifest)
    }

    pub fn sync_players(&self, players: impl IntoIterator<Item = PlayerScriptView>) {
        ACTIVE_HOST.with(|cell| {
            let mut state = cell.borrow_mut();
            state.players.clear();
            for p in players {
                state.players.insert(p.id, p);
            }
        });
    }

    pub fn dispatch_command(
        &mut self,
        source_id: u8,
        command: &str,
        args: &[String],
    ) -> Result<(bool, Vec<ScriptEffect>)> {
        ACTIVE_HOST.with(|cell| cell.borrow_mut().effects.clear());
        let cmd_json = serde_json::to_string(command)?;
        let args_json = serde_json::to_string(args)?;
        let expr = format!("__rp_dispatch_command({source_id}, {cmd_json}, {args_json});");
        let val = self
            .ctx
            .eval(Source::from_bytes(expr.as_bytes()))
            .map_err(|e| anyhow!("Command /{command} error: {e}"))?;
        let handled = val.to_boolean();
        Ok((handled, Self::drain_effects()))
    }

    pub fn dispatch_server_event(
        &mut self,
        event_name: &str,
        args: &[serde_json::Value],
    ) -> Result<Vec<ScriptEffect>> {
        ACTIVE_HOST.with(|cell| cell.borrow_mut().effects.clear());
        let ev_json = serde_json::to_string(event_name)?;
        let args_json = serde_json::to_string(args)?;
        let expr = format!("__rp_dispatch_event({ev_json}, {args_json});");
        self.ctx
            .eval(Source::from_bytes(expr.as_bytes()))
            .map_err(|e| anyhow!("Event {event_name} error: {e}"))?;
        Ok(Self::drain_effects())
    }

    pub fn dispatch_net_event(
        &mut self,
        source_id: u8,
        event: &NetEvent,
    ) -> Result<Vec<ScriptEffect>> {
        ACTIVE_HOST.with(|cell| cell.borrow_mut().effects.clear());
        let ev_json = serde_json::to_string(&event.event_name)?;
        let payload_json = serde_json::to_string(&event.json_payload)?;
        let expr = format!("__rp_dispatch_net_event({ev_json}, {source_id}, {payload_json});");
        self.ctx
            .eval(Source::from_bytes(expr.as_bytes()))
            .map_err(|e| anyhow!("NetEvent {} error: {e}", event.event_name))?;
        Ok(Self::drain_effects())
    }

    fn drain_effects() -> Vec<ScriptEffect> {
        ACTIVE_HOST.with(|cell| std::mem::take(&mut cell.borrow_mut().effects))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fivem_style_js_commands_and_events() {
        let mut engine = JsRoleplayEngine::new().expect("engine init");
        engine
            .eval_script(
                "test.js",
                r#"
                on("playerJoining", (src, name) => {
                    SetPlayerCash(src, 7500);
                    DisableNpcTraffic(src, true);
                    SendChatMessage(src, "[RP]", `Halo ${name}!`);
                });

                RegisterCommand("car", (src, args) => {
                    const model = args[0] || "zentorno";
                    const [x, y, z] = GetPlayerCoords(src);
                    SpawnVehicleForPlayer(src, model, x + 2.0, y, z, 90.0);
                });
                "#,
            )
            .expect("script eval");

        engine.sync_players([PlayerScriptView {
            id: 1,
            name: "Budi".to_string(),
            platform: Platform::Ios,
            pos: [100.0, 200.0, 30.0],
            cash: 0,
            bank: 0,
            job: "Warga".to_string(),
        }]);

        let join_effects = engine
            .dispatch_server_event(
                "playerJoining",
                &[serde_json::json!(1), serde_json::json!("Budi")],
            )
            .expect("join event");
        assert_eq!(join_effects.len(), 3);

        let (handled, cmd_effects) = engine
            .dispatch_command(1, "car", &["police".to_string()])
            .expect("car cmd");
        assert!(handled);
        assert!(matches!(
            &cmd_effects[0],
            ScriptEffect::SendRpc {
                target_id: 1,
                rpc: ServerRpc::SpawnVehicle { model_name, .. }
            } if model_name == "police"
        ));
    }

    #[test]
    fn test_roleplay_core_starter_resource_loads_and_runs() {
        let mut engine = JsRoleplayEngine::new().expect("engine init");
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/roleplay-core");
        let manifest = engine
            .load_resource_dir(&manifest_dir)
            .expect("roleplay-core should load cleanly");
        assert_eq!(manifest.name, "roleplay-core");

        engine.sync_players([
            PlayerScriptView {
                id: 1,
                name: "Andi".to_string(),
                platform: Platform::Ios,
                pos: [215.0, -810.0, 30.7],
                cash: 5000,
                bank: 25000,
                job: "Warga Sipil".to_string(),
            },
            PlayerScriptView {
                id: 2,
                name: "Siti".to_string(),
                platform: Platform::Android,
                pos: [218.0, -812.0, 30.7],
                cash: 5000,
                bank: 10000,
                job: "Warga Sipil".to_string(),
            },
        ]);

        let (ok_job, job_effects) = engine
            .dispatch_command(1, "job", &["polisi".to_string()])
            .expect("/job polisi");
        assert!(ok_job);
        assert!(job_effects.len() >= 4); // SetJob + 3 police weapons + chat notification

        let (ok_tf, tf_effects) = engine
            .dispatch_command(1, "transfer", &["2".to_string(), "5000".to_string()])
            .expect("/transfer 2 5000");
        assert!(ok_tf);
        assert_eq!(tf_effects.len(), 4); // 2 bank updates + 2 bank notifications
    }
}
