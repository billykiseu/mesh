use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;
use std::path::PathBuf;
use once_cell::sync::OnceCell;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use mesh_core::{NodeConfig, NodeEvent, NodeHandle, NodeIdentity, start_mesh_node};

/// Global state for the FFI layer.
struct FfiState {
    runtime: Runtime,
    handle: NodeHandle,
    identity: NodeIdentity,
    event_rx: Mutex<mpsc::Receiver<NodeEvent>>,
}

static STATE: OnceCell<FfiState> = OnceCell::new();

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn init_node(name: String, listen_port: u16, data_dir: String) -> Result<(), ()> {
    let config = NodeConfig {
        display_name: name,
        listen_port,
        key_path: PathBuf::from(&data_dir).join("mesh_identity.key"),
    };

    let runtime = Runtime::new().map_err(|_| ())?;

    let result = runtime.block_on(async {
        start_mesh_node(config).await
    });

    match result {
        Ok((identity, handle, event_rx)) => {
            let state = FfiState {
                runtime,
                handle,
                identity,
                event_rx: Mutex::new(event_rx),
            };
            STATE.set(state).map_err(|_| ())?;
            Ok(())
        }
        Err(_) => Err(()),
    }
}

fn send_broadcast(text: &str) -> Result<(), ()> {
    let state = STATE.get().ok_or(())?;
    let handle = state.handle.clone();
    state.runtime.block_on(handle.send_broadcast(text)).map_err(|_| ())
}

fn send_direct(dest: [u8; 32], text: &str) -> Result<(), ()> {
    let state = STATE.get().ok_or(())?;
    let handle = state.handle.clone();
    state.runtime.block_on(handle.send_direct(dest, text)).map_err(|_| ())
}

fn poll_event_internal() -> Option<NodeEvent> {
    let state = STATE.get()?;
    let mut rx = state.event_rx.lock().ok()?;
    rx.try_recv().ok()
}

fn get_node_id() -> Option<String> {
    STATE.get().map(|s| s.identity.node_id_hex())
}

fn get_node_id_short() -> Option<String> {
    STATE.get().map(|s| s.identity.node_id_short())
}

fn to_c_string(s: &str) -> *mut c_char {
    CString::new(s).unwrap_or_default().into_raw()
}

fn parse_hex_node_id(hex_str: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(hex_str).ok()?;
    if bytes.len() != 32 { return None; }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Some(arr)
}

// ---------------------------------------------------------------------------
// C FFI
// ---------------------------------------------------------------------------

/// Initialize and start the mesh node.
/// Returns 0 on success, -1 on error.
///
/// # Safety
/// `name` and `data_dir` must be valid C strings.
#[no_mangle]
pub unsafe extern "C" fn mesh_init(
    name: *const c_char,
    listen_port: u16,
    data_dir: *const c_char,
) -> i32 {
    let name = if name.is_null() {
        "MeshNode".to_string()
    } else {
        match CStr::from_ptr(name).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    let data_dir = if data_dir.is_null() {
        ".".to_string()
    } else {
        match CStr::from_ptr(data_dir).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    match init_node(name, listen_port, data_dir) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

/// Send a broadcast text message.
#[no_mangle]
pub unsafe extern "C" fn mesh_send_broadcast(text: *const c_char) -> i32 {
    let text = match CStr::from_ptr(text).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    match send_broadcast(text) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

/// Send a direct text message to a specific node.
#[no_mangle]
pub unsafe extern "C" fn mesh_send_direct(
    dest_hex: *const c_char,
    text: *const c_char,
) -> i32 {
    let dest_str = match CStr::from_ptr(dest_hex).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let dest_bytes = match parse_hex_node_id(dest_str) {
        Some(b) => b,
        None => return -1,
    };
    let text = match CStr::from_ptr(text).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    match send_direct(dest_bytes, text) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

/// Send a file to a specific node.
#[no_mangle]
pub unsafe extern "C" fn mesh_send_file(dest_hex: *const c_char, file_path: *const c_char) -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let dest_str = match CStr::from_ptr(dest_hex).to_str() { Ok(s) => s, Err(_) => return -1 };
    let dest = match parse_hex_node_id(dest_str) { Some(b) => b, None => return -1 };
    let path = match CStr::from_ptr(file_path).to_str() { Ok(s) => s, Err(_) => return -1 };
    let h = state.handle.clone();
    state.runtime.block_on(h.send_file(dest, path)).map(|_| 0i32).unwrap_or(-1)
}

/// Send a voice note.
#[no_mangle]
pub unsafe extern "C" fn mesh_send_voice(
    dest_hex: *const c_char,
    audio_data: *const u8,
    audio_len: u32,
    duration_ms: u32,
) -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let dest = if dest_hex.is_null() {
        None
    } else {
        let s = match CStr::from_ptr(dest_hex).to_str() { Ok(s) => s, Err(_) => return -1 };
        Some(match parse_hex_node_id(s) { Some(b) => b, None => return -1 })
    };
    if audio_data.is_null() || audio_len == 0 { return -1; }
    let data = std::slice::from_raw_parts(audio_data, audio_len as usize).to_vec();
    let h = state.handle.clone();
    state.runtime.block_on(h.send_voice(dest, data, duration_ms)).map(|_| 0i32).unwrap_or(-1)
}

/// Send a public broadcast message.
#[no_mangle]
pub unsafe extern "C" fn mesh_send_public_broadcast(text: *const c_char) -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let text = match CStr::from_ptr(text).to_str() { Ok(s) => s, Err(_) => return -1 };
    let h = state.handle.clone();
    state.runtime.block_on(h.send_public_broadcast(text)).map(|_| 0i32).unwrap_or(-1)
}

/// Send an SOS emergency broadcast.
#[no_mangle]
pub unsafe extern "C" fn mesh_send_sos(text: *const c_char, lat: f64, lon: f64) -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let text = match CStr::from_ptr(text).to_str() { Ok(s) => s, Err(_) => return -1 };
    let location = if lat == 0.0 && lon == 0.0 { None } else { Some((lat, lon)) };
    let h = state.handle.clone();
    state.runtime.block_on(h.send_sos(text, location)).map(|_| 0i32).unwrap_or(-1)
}

/// Update node profile.
#[no_mangle]
pub unsafe extern "C" fn mesh_update_profile(name: *const c_char, bio: *const c_char) -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let name = match CStr::from_ptr(name).to_str() { Ok(s) => s, Err(_) => return -1 };
    let bio = match CStr::from_ptr(bio).to_str() { Ok(s) => s, Err(_) => return -1 };
    let h = state.handle.clone();
    state.runtime.block_on(h.update_profile(name, bio)).map(|_| 0i32).unwrap_or(-1)
}

/// Accept a file transfer by file_id (16 bytes hex = 32 chars).
#[no_mangle]
pub unsafe extern "C" fn mesh_accept_file(file_id_hex: *const c_char) -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let hex_str = match CStr::from_ptr(file_id_hex).to_str() { Ok(s) => s, Err(_) => return -1 };
    let bytes = match hex::decode(hex_str) { Ok(b) if b.len() == 16 => b, _ => return -1 };
    let mut file_id = [0u8; 16];
    file_id.copy_from_slice(&bytes);
    let h = state.handle.clone();
    state.runtime.block_on(h.accept_file(file_id)).map(|_| 0i32).unwrap_or(-1)
}

/// Nuke: destroy identity and stop node.
#[no_mangle]
pub extern "C" fn mesh_nuke() -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let h = state.handle.clone();
    state.runtime.block_on(h.nuke()).map(|_| 0i32).unwrap_or(-1)
}

/// Stop the mesh node gracefully.
#[no_mangle]
pub extern "C" fn mesh_stop() -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let h = state.handle.clone();
    state.runtime.block_on(h.shutdown()).map(|_| 0i32).unwrap_or(-1)
}

/// Request stats (will be returned via mesh_poll_event as event_type 11).
#[no_mangle]
pub extern "C" fn mesh_get_stats() -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let h = state.handle.clone();
    state.runtime.block_on(h.get_stats()).map(|_| 0i32).unwrap_or(-1)
}

/// Request peer list (will be returned via mesh_poll_event as event_type 16).
#[no_mangle]
pub extern "C" fn mesh_get_peers_list() -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let h = state.handle.clone();
    state.runtime.block_on(h.get_peers()).map(|_| 0i32).unwrap_or(-1)
}

/// Start a voice call with a peer.
#[no_mangle]
pub unsafe extern "C" fn mesh_start_call(peer_hex: *const c_char) -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let peer_str = match CStr::from_ptr(peer_hex).to_str() { Ok(s) => s, Err(_) => return -1 };
    let peer = match parse_hex_node_id(peer_str) { Some(b) => b, None => return -1 };
    let h = state.handle.clone();
    state.runtime.block_on(h.start_voice_call(peer)).map(|_| 0i32).unwrap_or(-1)
}

/// End the current voice call.
#[no_mangle]
pub extern "C" fn mesh_end_call() -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let h = state.handle.clone();
    state.runtime.block_on(h.end_voice_call()).map(|_| 0i32).unwrap_or(-1)
}

/// Send an audio frame during a call.
#[no_mangle]
pub unsafe extern "C" fn mesh_send_audio_frame(
    peer_hex: *const c_char,
    data: *const u8,
    data_len: u32,
) -> i32 {
    let state = match STATE.get() { Some(s) => s, None => return -1 };
    let peer_str = match CStr::from_ptr(peer_hex).to_str() { Ok(s) => s, Err(_) => return -1 };
    let peer = match parse_hex_node_id(peer_str) { Some(b) => b, None => return -1 };
    if data.is_null() || data_len == 0 { return -1; }
    let frame = std::slice::from_raw_parts(data, data_len as usize).to_vec();
    let h = state.handle.clone();
    state.runtime.block_on(h.send_audio_frame(peer, frame)).map(|_| 0i32).unwrap_or(-1)
}

// ---------------------------------------------------------------------------
// Event polling
// ---------------------------------------------------------------------------

/// Event types returned by mesh_poll_event.
/// Event type codes:
///   0=none, 1=peer_connected, 2=peer_disconnected, 3=message_received, 4=started,
///   5=file_offered, 6=file_progress, 7=file_complete, 8=voice_received,
///   9=profile_updated, 10=gateway_found, 11=stats, 12=sos_received,
///   13=call_incoming, 14=audio_frame, 15=call_ended, 16=peer_list,
///   17=public_broadcast, 18=gateway_lost, 19=nuked, 20=stopped
#[repr(C)]
pub struct MeshEvent {
    pub event_type: i32,
    /// Node ID as hex string (for peer events and messages)
    pub node_id: *mut c_char,
    /// Display name or message content
    pub data: *mut c_char,
    /// Sender name (for message events)
    pub sender_name: *mut c_char,
    /// Extra data (file_id hex, stats JSON, etc.)
    pub extra: *mut c_char,
    /// Numeric value (progress %, duration, etc.)
    pub value: i64,
    /// Float values (lat, lon for SOS)
    pub float1: f64,
    pub float2: f64,
    /// Binary data pointer and length (for audio)
    pub binary_data: *mut u8,
    pub binary_len: u32,
}

impl MeshEvent {
    fn empty() -> Self {
        Self {
            event_type: 0,
            node_id: std::ptr::null_mut(),
            data: std::ptr::null_mut(),
            sender_name: std::ptr::null_mut(),
            extra: std::ptr::null_mut(),
            value: 0,
            float1: 0.0,
            float2: 0.0,
            binary_data: std::ptr::null_mut(),
            binary_len: 0,
        }
    }
}

/// Get the node ID as a hex string. Caller must free with mesh_free_string.
#[no_mangle]
pub extern "C" fn mesh_get_node_id() -> *mut c_char {
    match get_node_id() {
        Some(id) => to_c_string(&id),
        None => std::ptr::null_mut(),
    }
}

/// Get the short node ID. Caller must free with mesh_free_string.
#[no_mangle]
pub extern "C" fn mesh_get_node_id_short() -> *mut c_char {
    match get_node_id_short() {
        Some(id) => to_c_string(&id),
        None => std::ptr::null_mut(),
    }
}

/// Poll for the next event. Non-blocking.
/// Caller must free returned strings with mesh_free_string and binary data with mesh_free_binary.
#[no_mangle]
pub extern "C" fn mesh_poll_event() -> MeshEvent {
    match poll_event_internal() {
        None => MeshEvent::empty(),
        Some(event) => convert_event(event),
    }
}

fn convert_event(event: NodeEvent) -> MeshEvent {
    match event {
        NodeEvent::Started { node_id } => MeshEvent {
            event_type: 4,
            node_id: to_c_string(&node_id),
            ..MeshEvent::empty()
        },
        NodeEvent::PeerConnected { node_id, display_name } => MeshEvent {
            event_type: 1,
            node_id: to_c_string(&hex::encode(node_id)),
            data: to_c_string(&display_name),
            ..MeshEvent::empty()
        },
        NodeEvent::PeerDisconnected { node_id } => MeshEvent {
            event_type: 2,
            node_id: to_c_string(&hex::encode(node_id)),
            ..MeshEvent::empty()
        },
        NodeEvent::MessageReceived { sender_id, sender_name, content } => MeshEvent {
            event_type: 3,
            node_id: to_c_string(&hex::encode(sender_id)),
            data: to_c_string(&content),
            sender_name: to_c_string(&sender_name),
            ..MeshEvent::empty()
        },
        NodeEvent::FileOffered { sender_id, sender_name, file_id, filename, size } => MeshEvent {
            event_type: 5,
            node_id: to_c_string(&hex::encode(sender_id)),
            data: to_c_string(&filename),
            sender_name: to_c_string(&sender_name),
            extra: to_c_string(&hex::encode(file_id)),
            value: size as i64,
            ..MeshEvent::empty()
        },
        NodeEvent::FileProgress { file_id, pct } => MeshEvent {
            event_type: 6,
            extra: to_c_string(&hex::encode(file_id)),
            value: pct as i64,
            ..MeshEvent::empty()
        },
        NodeEvent::FileComplete { file_id, path } => MeshEvent {
            event_type: 7,
            data: to_c_string(&path),
            extra: to_c_string(&hex::encode(file_id)),
            ..MeshEvent::empty()
        },
        NodeEvent::VoiceReceived { sender_id, sender_name, audio_data, duration_ms } => {
            let len = audio_data.len() as u32;
            let ptr = if audio_data.is_empty() {
                std::ptr::null_mut()
            } else {
                let mut boxed = audio_data.into_boxed_slice();
                let ptr = boxed.as_mut_ptr();
                std::mem::forget(boxed);
                ptr
            };
            MeshEvent {
                event_type: 8,
                node_id: to_c_string(&hex::encode(sender_id)),
                sender_name: to_c_string(&sender_name),
                value: duration_ms as i64,
                binary_data: ptr,
                binary_len: len,
                ..MeshEvent::empty()
            }
        },
        NodeEvent::ProfileUpdated { node_id, name, bio } => MeshEvent {
            event_type: 9,
            node_id: to_c_string(&hex::encode(node_id)),
            data: to_c_string(&name),
            extra: to_c_string(&bio),
            ..MeshEvent::empty()
        },
        NodeEvent::GatewayFound { node_id, display_name } => MeshEvent {
            event_type: 10,
            node_id: to_c_string(&hex::encode(node_id)),
            data: to_c_string(&display_name),
            ..MeshEvent::empty()
        },
        NodeEvent::Stats { stats } => {
            let json = format!(
                r#"{{"total_peers":{},"messages_relayed":{},"messages_received":{},"unique_nodes_seen":{},"avg_hops":{:.2}}}"#,
                stats.total_peers, stats.messages_relayed, stats.messages_received,
                stats.unique_nodes_seen, stats.avg_hops
            );
            MeshEvent {
                event_type: 11,
                data: to_c_string(&json),
                ..MeshEvent::empty()
            }
        },
        NodeEvent::SOSReceived { sender_id, sender_name, text, location } => {
            let (lat, lon) = location.unwrap_or((0.0, 0.0));
            MeshEvent {
                event_type: 12,
                node_id: to_c_string(&hex::encode(sender_id)),
                data: to_c_string(&text),
                sender_name: to_c_string(&sender_name),
                float1: lat,
                float2: lon,
                ..MeshEvent::empty()
            }
        },
        NodeEvent::IncomingCall { peer, peer_name } => MeshEvent {
            event_type: 13,
            node_id: to_c_string(&hex::encode(peer)),
            data: to_c_string(&peer_name),
            ..MeshEvent::empty()
        },
        NodeEvent::AudioFrame { peer, data } => {
            let len = data.len() as u32;
            let ptr = if data.is_empty() {
                std::ptr::null_mut()
            } else {
                let mut boxed = data.into_boxed_slice();
                let ptr = boxed.as_mut_ptr();
                std::mem::forget(boxed);
                ptr
            };
            MeshEvent {
                event_type: 14,
                node_id: to_c_string(&hex::encode(peer)),
                binary_data: ptr,
                binary_len: len,
                ..MeshEvent::empty()
            }
        },
        NodeEvent::CallEnded { peer } => MeshEvent {
            event_type: 15,
            node_id: to_c_string(&hex::encode(peer)),
            ..MeshEvent::empty()
        },
        NodeEvent::PeerList { peers } => {
            // Serialize peer list as JSON
            let entries: Vec<String> = peers.iter().map(|p| {
                format!(
                    r#"{{"node_id":"{}","name":"{}","addr":"{}","is_gateway":{},"bio":"{}"}}"#,
                    hex::encode(p.node_id),
                    p.display_name.replace('"', r#"\""#),
                    p.addr.replace('"', r#"\""#),
                    p.is_gateway,
                    p.bio.replace('"', r#"\""#),
                )
            }).collect();
            let json = format!("[{}]", entries.join(","));
            MeshEvent {
                event_type: 16,
                data: to_c_string(&json),
                value: peers.len() as i64,
                ..MeshEvent::empty()
            }
        },
        NodeEvent::PublicBroadcast { sender_id, sender_name, text } => MeshEvent {
            event_type: 17,
            node_id: to_c_string(&hex::encode(sender_id)),
            data: to_c_string(&text),
            sender_name: to_c_string(&sender_name),
            ..MeshEvent::empty()
        },
        NodeEvent::GatewayLost { node_id } => MeshEvent {
            event_type: 18,
            node_id: to_c_string(&hex::encode(node_id)),
            ..MeshEvent::empty()
        },
        NodeEvent::Nuked => MeshEvent {
            event_type: 19,
            ..MeshEvent::empty()
        },
        NodeEvent::Stopped => MeshEvent {
            event_type: 20,
            ..MeshEvent::empty()
        },
    }
}

/// Free a string returned by the FFI layer.
///
/// # Safety
/// `s` must be a pointer returned by one of the mesh_* functions, or null.
#[no_mangle]
pub unsafe extern "C" fn mesh_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// Free binary data returned by the FFI layer (audio frames, etc.).
///
/// # Safety
/// `ptr` must be a pointer returned in MeshEvent.binary_data, or null.
#[no_mangle]
pub unsafe extern "C" fn mesh_free_binary(ptr: *mut u8, len: u32) {
    if !ptr.is_null() && len > 0 {
        let _ = Vec::from_raw_parts(ptr, len as usize, len as usize);
    }
}

// ---------------------------------------------------------------------------
// JNI bindings for Android
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod jni_bindings {
    use jni::JNIEnv;
    use jni::objects::{JClass, JObject, JString, JValue, JByteArray};
    use jni::sys::{jint, jstring, jobject, jdouble};

    use super::*;

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshInit(
        mut env: JNIEnv,
        _class: JClass,
        name: JString,
        listen_port: jint,
        data_dir: JString,
    ) -> jint {
        let name: String = match env.get_string(&name) { Ok(s) => s.into(), Err(_) => return -1 };
        let data_dir: String = match env.get_string(&data_dir) { Ok(s) => s.into(), Err(_) => return -1 };
        match init_node(name, listen_port as u16, data_dir) { Ok(()) => 0, Err(()) => -1 }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshSendBroadcast(
        mut env: JNIEnv, _class: JClass, text: JString,
    ) -> jint {
        let text: String = match env.get_string(&text) { Ok(s) => s.into(), Err(_) => return -1 };
        match send_broadcast(&text) { Ok(()) => 0, Err(()) => -1 }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshSendDirect(
        mut env: JNIEnv, _class: JClass, dest_hex: JString, text: JString,
    ) -> jint {
        let dest_str: String = match env.get_string(&dest_hex) { Ok(s) => s.into(), Err(_) => return -1 };
        let text: String = match env.get_string(&text) { Ok(s) => s.into(), Err(_) => return -1 };
        let dest = match parse_hex_node_id(&dest_str) { Some(b) => b, None => return -1 };
        match send_direct(dest, &text) { Ok(()) => 0, Err(()) => -1 }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshGetNodeId(
        env: JNIEnv, _class: JClass,
    ) -> jstring {
        match get_node_id() {
            Some(id) => env.new_string(&id).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut()),
            None => std::ptr::null_mut(),
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshGetNodeIdShort(
        env: JNIEnv, _class: JClass,
    ) -> jstring {
        match get_node_id_short() {
            Some(id) => env.new_string(&id).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut()),
            None => std::ptr::null_mut(),
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshSendFile(
        mut env: JNIEnv, _class: JClass, dest_hex: JString, file_path: JString,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let dest_str: String = match env.get_string(&dest_hex) { Ok(s) => s.into(), Err(_) => return -1 };
        let path: String = match env.get_string(&file_path) { Ok(s) => s.into(), Err(_) => return -1 };
        let dest = match parse_hex_node_id(&dest_str) { Some(b) => b, None => return -1 };
        let h = state.handle.clone();
        state.runtime.block_on(h.send_file(dest, &path)).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshAcceptFile(
        mut env: JNIEnv, _class: JClass, file_id_hex: JString,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let hex_str: String = match env.get_string(&file_id_hex) { Ok(s) => s.into(), Err(_) => return -1 };
        let bytes = match hex::decode(&hex_str) { Ok(b) if b.len() == 16 => b, _ => return -1 };
        let mut file_id = [0u8; 16];
        file_id.copy_from_slice(&bytes);
        let h = state.handle.clone();
        state.runtime.block_on(h.accept_file(file_id)).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshSendVoice(
        mut env: JNIEnv, _class: JClass,
        dest_hex: JString, audio_data: JByteArray, duration_ms: jint,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let dest = if env.is_same_object(&dest_hex, JObject::null()).unwrap_or(true) {
            None
        } else {
            let s: String = match env.get_string(&dest_hex) { Ok(s) => s.into(), Err(_) => return -1 };
            Some(match parse_hex_node_id(&s) { Some(b) => b, None => return -1 })
        };
        let data = match env.convert_byte_array(audio_data) { Ok(d) => d, Err(_) => return -1 };
        let h = state.handle.clone();
        state.runtime.block_on(h.send_voice(dest, data, duration_ms as u32)).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshSendPublicBroadcast(
        mut env: JNIEnv, _class: JClass, text: JString,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let text: String = match env.get_string(&text) { Ok(s) => s.into(), Err(_) => return -1 };
        let h = state.handle.clone();
        state.runtime.block_on(h.send_public_broadcast(&text)).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshSendSOS(
        mut env: JNIEnv, _class: JClass, text: JString, lat: jdouble, lon: jdouble,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let text: String = match env.get_string(&text) { Ok(s) => s.into(), Err(_) => return -1 };
        let loc = if lat == 0.0 && lon == 0.0 { None } else { Some((lat, lon)) };
        let h = state.handle.clone();
        state.runtime.block_on(h.send_sos(&text, loc)).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshUpdateProfile(
        mut env: JNIEnv, _class: JClass, name: JString, bio: JString,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let name: String = match env.get_string(&name) { Ok(s) => s.into(), Err(_) => return -1 };
        let bio: String = match env.get_string(&bio) { Ok(s) => s.into(), Err(_) => return -1 };
        let h = state.handle.clone();
        state.runtime.block_on(h.update_profile(&name, &bio)).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshNuke(
        _env: JNIEnv, _class: JClass,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let h = state.handle.clone();
        state.runtime.block_on(h.nuke()).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshStop(
        _env: JNIEnv, _class: JClass,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let h = state.handle.clone();
        state.runtime.block_on(h.shutdown()).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshGetStats(
        _env: JNIEnv, _class: JClass,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let h = state.handle.clone();
        state.runtime.block_on(h.get_stats()).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshGetPeersList(
        _env: JNIEnv, _class: JClass,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let h = state.handle.clone();
        state.runtime.block_on(h.get_peers()).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshStartCall(
        mut env: JNIEnv, _class: JClass, peer_hex: JString,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let s: String = match env.get_string(&peer_hex) { Ok(s) => s.into(), Err(_) => return -1 };
        let peer = match parse_hex_node_id(&s) { Some(b) => b, None => return -1 };
        let h = state.handle.clone();
        state.runtime.block_on(h.start_voice_call(peer)).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshEndCall(
        _env: JNIEnv, _class: JClass,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let h = state.handle.clone();
        state.runtime.block_on(h.end_voice_call()).map(|_| 0i32).unwrap_or(-1)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshSendAudioFrame(
        mut env: JNIEnv, _class: JClass, peer_hex: JString, data: JByteArray,
    ) -> jint {
        let state = match STATE.get() { Some(s) => s, None => return -1 };
        let s: String = match env.get_string(&peer_hex) { Ok(s) => s.into(), Err(_) => return -1 };
        let peer = match parse_hex_node_id(&s) { Some(b) => b, None => return -1 };
        let frame = match env.convert_byte_array(data) { Ok(d) => d, Err(_) => return -1 };
        let h = state.handle.clone();
        state.runtime.block_on(h.send_audio_frame(peer, frame)).map(|_| 0i32).unwrap_or(-1)
    }

    /// Poll for the next mesh event. Returns a MeshBridge.MeshEvent object, or null.
    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshPollEvent(
        mut env: JNIEnv,
        _class: JClass,
    ) -> jobject {
        let event = match poll_event_internal() {
            Some(ev) => ev,
            None => return std::ptr::null_mut(),
        };

        let (event_type, node_id, data, sender_name, extra, value, float1, float2, binary) = match event {
            NodeEvent::Started { node_id } =>
                (4i32, Some(node_id), None, None, None, 0i64, 0.0, 0.0, None),
            NodeEvent::PeerConnected { node_id, display_name } =>
                (1, Some(hex::encode(node_id)), Some(display_name), None, None, 0, 0.0, 0.0, None),
            NodeEvent::PeerDisconnected { node_id } =>
                (2, Some(hex::encode(node_id)), None, None, None, 0, 0.0, 0.0, None),
            NodeEvent::MessageReceived { sender_id, sender_name, content } =>
                (3, Some(hex::encode(sender_id)), Some(content), Some(sender_name), None, 0, 0.0, 0.0, None),
            NodeEvent::FileOffered { sender_id, sender_name, file_id, filename, size } =>
                (5, Some(hex::encode(sender_id)), Some(filename), Some(sender_name), Some(hex::encode(file_id)), size as i64, 0.0, 0.0, None),
            NodeEvent::FileProgress { file_id, pct } =>
                (6, None, None, None, Some(hex::encode(file_id)), pct as i64, 0.0, 0.0, None),
            NodeEvent::FileComplete { file_id, path } =>
                (7, None, Some(path), None, Some(hex::encode(file_id)), 0, 0.0, 0.0, None),
            NodeEvent::VoiceReceived { sender_id, sender_name, audio_data, duration_ms } =>
                (8, Some(hex::encode(sender_id)), None, Some(sender_name), None, duration_ms as i64, 0.0, 0.0, Some(audio_data)),
            NodeEvent::ProfileUpdated { node_id, name, bio } =>
                (9, Some(hex::encode(node_id)), Some(name), None, Some(bio), 0, 0.0, 0.0, None),
            NodeEvent::GatewayFound { node_id, display_name } =>
                (10, Some(hex::encode(node_id)), Some(display_name), None, None, 0, 0.0, 0.0, None),
            NodeEvent::Stats { stats } => {
                let json = format!(
                    r#"{{"total_peers":{},"messages_relayed":{},"messages_received":{},"unique_nodes_seen":{},"avg_hops":{:.2}}}"#,
                    stats.total_peers, stats.messages_relayed, stats.messages_received,
                    stats.unique_nodes_seen, stats.avg_hops
                );
                (11, None, Some(json), None, None, 0, 0.0, 0.0, None)
            },
            NodeEvent::SOSReceived { sender_id, sender_name, text, location } => {
                let (lat, lon) = location.unwrap_or((0.0, 0.0));
                (12, Some(hex::encode(sender_id)), Some(text), Some(sender_name), None, 0, lat, lon, None)
            },
            NodeEvent::IncomingCall { peer, peer_name } =>
                (13, Some(hex::encode(peer)), Some(peer_name), None, None, 0, 0.0, 0.0, None),
            NodeEvent::AudioFrame { peer, data } =>
                (14, Some(hex::encode(peer)), None, None, None, 0, 0.0, 0.0, Some(data)),
            NodeEvent::CallEnded { peer } =>
                (15, Some(hex::encode(peer)), None, None, None, 0, 0.0, 0.0, None),
            NodeEvent::PeerList { peers } => {
                let entries: Vec<String> = peers.iter().map(|p| {
                    format!(
                        r#"{{"node_id":"{}","name":"{}","addr":"{}","is_gateway":{},"bio":"{}"}}"#,
                        hex::encode(p.node_id), p.display_name, p.addr, p.is_gateway, p.bio,
                    )
                }).collect();
                let json = format!("[{}]", entries.join(","));
                (16, None, Some(json), None, None, peers.len() as i64, 0.0, 0.0, None)
            },
            NodeEvent::PublicBroadcast { sender_id, sender_name, text } =>
                (17, Some(hex::encode(sender_id)), Some(text), Some(sender_name), None, 0, 0.0, 0.0, None),
            NodeEvent::GatewayLost { node_id } =>
                (18, Some(hex::encode(node_id)), None, None, None, 0, 0.0, 0.0, None),
            NodeEvent::Nuked =>
                (19, None, None, None, None, 0, 0.0, 0.0, None),
            NodeEvent::Stopped =>
                (20, None, None, None, None, 0, 0.0, 0.0, None),
        };

        // Build Java strings (null-safe)
        let j_node_id = make_jstring(&mut env, &node_id);
        let j_data = make_jstring(&mut env, &data);
        let j_sender_name = make_jstring(&mut env, &sender_name);
        let j_extra = make_jstring(&mut env, &extra);

        // Create byte array for binary data
        let j_binary = match binary {
            Some(ref b) if !b.is_empty() => {
                match env.byte_array_from_slice(b) {
                    Ok(arr) => JObject::from(arr),
                    Err(_) => JObject::null(),
                }
            },
            _ => JObject::null(),
        };

        // Construct MeshBridge$MeshEvent(int, String?, String?, String?, String?, long, double, double, byte[]?)
        let result = env.new_object(
            "com/mesh/app/MeshBridge$MeshEvent",
            "(ILjava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;JDD[B)V",
            &[
                JValue::Int(event_type),
                JValue::Object(&j_node_id),
                JValue::Object(&j_data),
                JValue::Object(&j_sender_name),
                JValue::Object(&j_extra),
                JValue::Long(value),
                JValue::Double(float1),
                JValue::Double(float2),
                JValue::Object(&j_binary),
            ],
        );

        match result {
            Ok(obj) => obj.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    }

    fn make_jstring<'a>(env: &mut JNIEnv<'a>, s: &Option<String>) -> JObject<'a> {
        match s {
            Some(s) => match env.new_string(s) {
                Ok(js) => JObject::from(js),
                Err(_) => JObject::null(),
            },
            None => JObject::null(),
        }
    }
}
