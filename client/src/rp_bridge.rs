//! Roleplay Bridge connecting `rp-client-core` (`RpRoleplaySession`) into the Wasmtime/Metal/Desktop client.
//!
//! Works across iOS (`.ipa`), macOS (`.app`), Windows, Linux, and Android with zero HTML/CEF RAM overhead.

use crate::mp::Cmd;
use rp_client_core::{RpRoleplaySession, WasmGameBridge};
use rp_proto::{NativeArg, Platform};
use std::ffi::CStr;
use std::net::UdpSocket;
use std::os::raw::c_char;
use std::sync::{LazyLock, Mutex};
use wasmtime::Val;

const RP_STR_BASE: u64 = 1600;
const RP_STR_SLOT_SIZE: u64 = 192;
const RP_VEC_BASE: u64 = 2240;

static SESSION: LazyLock<Mutex<RpRoleplaySession>> =
    LazyLock::new(|| Mutex::new(RpRoleplaySession::new()));
static ACTIVE_SOCK: LazyLock<Mutex<Option<UdpSocket>>> = LazyLock::new(|| Mutex::new(None));

pub fn current_platform() -> Platform {
    if cfg!(target_os = "ios") {
        Platform::Ios
    } else if cfg!(target_os = "android") {
        Platform::Android
    } else if cfg!(target_os = "macos") {
        Platform::Mac
    } else {
        Platform::Pc
    }
}

pub fn register_socket(sock: &UdpSocket) {
    if let Ok(cloned) = sock.try_clone() {
        *ACTIVE_SOCK.lock().unwrap() = Some(cloned);
    }
}

pub fn build_hello(name: &str) -> Vec<u8> {
    RpRoleplaySession::build_hello_packet(name, current_platform())
}

pub fn on_resend_tick(sock: &UdpSocket) {
    SESSION.lock().unwrap().tick_resends(sock);
}

pub fn on_udp_packet(sock: &UdpSocket, kind: u8, payload: &[u8]) -> bool {
    SESSION.lock().unwrap().handle_rp_packet(sock, kind, payload)
}

pub fn is_npc_traffic_disabled() -> bool {
    SESSION.lock().unwrap().npc_traffic_disabled
}

pub fn send_chat(line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let sock_guard = ACTIVE_SOCK.lock().unwrap();
    if let Some(sock) = sock_guard.as_ref() {
        SESSION.lock().unwrap().send_chat_or_command(sock, trimmed);
    }
}

/// FFI entry point called by iOS `Launcher.swift` (`UIAlertController` Roleplay Chat/Command dialog).
#[no_mangle]
pub extern "C" fn muguet_send_rp_chat(line: *const c_char) {
    if line.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(line) }.to_string_lossy();
    send_chat(&text);
}

/// Open the native iOS Roleplay Chat / `/command` input sheet when tapping `"RP / CHAT"` on iPhone.
#[cfg(target_os = "ios")]
pub fn open_ios_chat_prompt() {
    extern "C" {
        fn muguet_prompt_rp_chat();
    }
    unsafe { muguet_prompt_rp_chat() };
}

pub fn is_typing() -> bool {
    SESSION.lock().unwrap().typing_buffer.is_some()
}

/// Intercepts keyboard events on Desktop when `KeyT` chat input is open (or opens it on `KeyT` / `Slash`).
/// Returns `true` if the key event was consumed by Roleplay chat and should NOT be sent to GTA V controls.
pub fn handle_desktop_key(code: &str, pressed: bool, text: Option<&str>, ctrl_or_cmd: bool) -> bool {
    let mut to_send: Option<String> = None;
    let consumed = {
        let mut sess = SESSION.lock().unwrap();
        if let Some(buf) = sess.typing_buffer.as_mut() {
            if !pressed {
                return true;
            }
            match code {
                "Escape" => {
                    sess.typing_buffer = None;
                }
                "Enter" | "NumpadEnter" => {
                    let finished = sess.typing_buffer.take().unwrap_or_default();
                    if !finished.trim().is_empty() {
                        to_send = Some(finished);
                    }
                }
                "Backspace" => {
                    buf.pop();
                }
                _ => {
                    if !ctrl_or_cmd {
                        if let Some(t) = text {
                            for ch in t.chars().filter(|c| !c.is_control()) {
                                if buf.chars().count() < 120 {
                                    buf.push(ch);
                                }
                            }
                        }
                    }
                }
            }
            true
        } else if pressed && !ctrl_or_cmd && ACTIVE_SOCK.lock().unwrap().is_some() {
            if code == "KeyT" {
                sess.typing_buffer = Some(String::new());
                true
            } else if code == "Slash" {
                sess.typing_buffer = Some("/".to_string());
                true
            } else if code == "F7" {
                sess.show_chat_pinned = !sess.show_chat_pinned;
                true
            } else {
                false
            }
        } else {
            false
        }
    };

    if let Some(line) = to_send {
        send_chat(&line);
    }
    consumed
}

struct CmdBridge<'a, 'b, 'c> {
    cmd: &'c mut Cmd<'a, 'b>,
}

impl CmdBridge<'_, '_, '_> {
    fn encode_args(&mut self, args: &[NativeArg]) -> Vec<Val> {
        let mut out = Vec::with_capacity(args.len());
        let mut str_slot = 0u64;
        let mut vec_slot = 0u64;

        for arg in args {
            match arg {
                NativeArg::Int(v) => out.push(Val::I32(*v)),
                NativeArg::Float(v) => out.push(Val::F32(v.to_bits())),
                NativeArg::Bool(v) => out.push(Val::I32(*v as i32)),
                NativeArg::Vec3(v) => {
                    let off = RP_VEC_BASE + (vec_slot % 4) * 32;
                    vec_slot += 1;
                    out.push(self.cmd.rp_vec_at(off, *v));
                }
                NativeArg::Str(s) => {
                    let off = RP_STR_BASE + (str_slot % 3) * RP_STR_SLOT_SIZE;
                    str_slot += 1;
                    out.push(self.cmd.rp_cstr_at(off, s, (RP_STR_SLOT_SIZE - 2) as usize));
                }
            }
        }
        out
    }
}

impl WasmGameBridge for CmdBridge<'_, '_, '_> {
    fn player_ped_id(&mut self) -> i32 {
        let ped = self.cmd.i("PlayerPedId", &[]);
        if ped != 0 && self.cmd.exists(ped) {
            ped
        } else {
            0
        }
    }

    fn invoke_int(&mut self, command: &str, args: &[NativeArg]) -> i32 {
        let wasm_args = self.encode_args(args);
        self.cmd.i(command, &wasm_args)
    }

    fn invoke_void(&mut self, command: &str, args: &[NativeArg]) {
        let wasm_args = self.encode_args(args);
        let _ = self.cmd.i(command, &wasm_args);
    }

    fn has_model_loaded(&mut self, model_hash: u32) -> bool {
        self.cmd.i("HasModelLoaded", &[Val::I32(model_hash as i32)]) != 0
    }

    fn request_model(&mut self, model_hash: u32) {
        let _ = self.cmd.i("RequestModel", &[Val::I32(model_hash as i32)]);
    }

    fn has_anim_dict_loaded(&mut self, dict: &str) -> bool {
        let ptr = self.cmd.rp_cstr_at(RP_STR_BASE, dict, 120);
        self.cmd.i("DoesAnimDictExist", &[ptr]) != 0
            && self.cmd.i("HasAnimDictLoaded", &[ptr]) != 0
    }

    fn request_anim_dict(&mut self, dict: &str) {
        let ptr = self.cmd.rp_cstr_at(RP_STR_BASE, dict, 120);
        if self.cmd.i("DoesAnimDictExist", &[ptr]) != 0 {
            let _ = self.cmd.i("RequestAnimDict", &[ptr]);
        }
    }
}

pub fn on_script_tick(cmd: &mut Cmd<'_, '_>) {
    if ACTIVE_SOCK.lock().unwrap().is_none() {
        return;
    }
    let mut bridge = CmdBridge { cmd };
    SESSION.lock().unwrap().execute_script_tick(&mut bridge);
}

