use rp_client_core::{RpRoleplaySession, WasmGameBridge};
use rp_proto::*;
use std::net::UdpSocket;

#[derive(Default)]
struct MockWasmGame {
    calls: Vec<(String, Vec<NativeArg>)>,
}

impl WasmGameBridge for MockWasmGame {
    fn player_ped_id(&mut self) -> i32 {
        77
    }

    fn invoke_int(&mut self, command: &str, args: &[NativeArg]) -> i32 {
        self.calls.push((command.to_string(), args.to_vec()));
        if command == "CreateVehicle" {
            999
        } else {
            0
        }
    }

    fn invoke_void(&mut self, command: &str, args: &[NativeArg]) {
        self.calls.push((command.to_string(), args.to_vec()));
    }

    fn has_model_loaded(&mut self, _model_hash: u32) -> bool {
        true
    }

    fn request_model(&mut self, model_hash: u32) {
        self.calls
            .push(("RequestModel".to_string(), vec![NativeArg::Int(model_hash as i32)]));
    }

    fn has_anim_dict_loaded(&mut self, _dict: &str) -> bool {
        true
    }

    fn request_anim_dict(&mut self, dict: &str) {
        self.calls
            .push(("RequestAnimDict".to_string(), vec![NativeArg::Str(dict.to_string())]));
    }
}

#[test]
fn test_roleplay_session_udp_rpc_and_hud_pipeline() {
    let server_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let server_addr = server_sock.local_addr().unwrap();
    let client_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    client_sock.connect(server_addr).unwrap();

    let mut session = RpRoleplaySession::new();

    // 1. Client sends `/car sultan` over PKT_RP_REL (28)
    session.send_chat_or_command(&client_sock, "/car sultan");
    let mut buf = [0u8; 1024];
    let (n, client_addr) = server_sock.recv_from(&mut buf).unwrap();
    let (kind, payload) = parse_packet(&buf[..n]).unwrap();
    assert_eq!(kind, PKT_RP_REL);
    assert_eq!(payload[2], REL_CHAT_MSG);
    let chat: ChatMessage = serde_json::from_slice(&payload[3..]).unwrap();
    assert_eq!(chat.text, "/car sultan");

    // 2. Server replies with PKT_RP_ACK + HUD update + SpawnVehicle RPC
    let _ = server_sock.send_to(&make_packet(PKT_RP_ACK, &[payload[0], payload[1]]), client_addr);
    let ack_len = client_sock.recv(&mut buf).unwrap();
    let (ack_kind, ack_body) = parse_packet(&buf[..ack_len]).unwrap();
    assert!(session.handle_rp_packet(&client_sock, ack_kind, ack_body));

    let hud = HudState {
        cash: 5000,
        bank: 25000,
        job: "Kepolisian LSPD".to_string(),
        server_name: "IndoCross RP".to_string(),
        online_count: 12,
    };
    let mut hud_rel = vec![1, 0, REL_HUD_UPDATE];
    hud_rel.extend(serde_json::to_vec(&hud).unwrap());
    assert!(session.handle_rp_packet(&client_sock, PKT_RP_REL, &hud_rel));
    assert_eq!(session.hud.cash, 5000);
    assert_eq!(session.hud.job, "Kepolisian LSPD");

    let spawn_rpc = ServerRpc::SpawnVehicle {
        model_hash: joaat("sultan"),
        model_name: "sultan".to_string(),
        pos: [215.8, -810.1, 30.73],
        heading: 160.0,
        put_in_driver_seat: true,
    };
    let mut rpc_rel = vec![2, 0, REL_SERVER_RPC];
    rpc_rel.extend(serde_json::to_vec(&spawn_rpc).unwrap());
    assert!(session.handle_rp_packet(&client_sock, PKT_RP_REL, &rpc_rel));

    // 3. Execute Wasm script tick and verify vehicle creation + plate + HUD draw + zero-NPC density
    let mut mock_game = MockWasmGame::default();
    session.execute_script_tick(&mut mock_game);

    assert!(mock_game
        .calls
        .iter()
        .any(|(c, _)| c == "SetPedDensityMultiplierThisFrame"));
    assert!(mock_game.calls.iter().any(|(c, _)| c == "CreateVehicle"));
    assert!(mock_game
        .calls
        .iter()
        .any(|(c, _)| c == "SetVehicleNumberPlateText"));
    assert!(mock_game
        .calls
        .iter()
        .any(|(c, _)| c == "SetPedIntoVehicle"));
    assert!(mock_game.calls.iter().any(|(c, _)| c == "DrawRect"));
}

