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

// ---------------------------------------------------------------------------
// C FFI (original API)
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
/// Returns 0 on success, -1 on error.
///
/// # Safety
/// `text` must be a valid C string.
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
/// Returns 0 on success, -1 on error.
///
/// # Safety
/// `dest_hex` and `text` must be valid C strings. `dest_hex` must be a 64-char hex node ID.
#[no_mangle]
pub unsafe extern "C" fn mesh_send_direct(
    dest_hex: *const c_char,
    text: *const c_char,
) -> i32 {
    let dest_str = match CStr::from_ptr(dest_hex).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let dest_bytes = match hex::decode(dest_str) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        _ => return -1,
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

/// Event types returned by mesh_poll_event.
#[repr(C)]
pub struct MeshEvent {
    /// Event type: 0=none, 1=peer_connected, 2=peer_disconnected, 3=message_received, 4=started
    pub event_type: i32,
    /// Node ID as hex string (for peer events and messages)
    pub node_id: *mut c_char,
    /// Display name or message content
    pub data: *mut c_char,
    /// Sender name (for message events)
    pub sender_name: *mut c_char,
}

/// Poll for the next event. Non-blocking.
/// Caller must free the returned strings with mesh_free_string.
#[no_mangle]
pub extern "C" fn mesh_poll_event() -> MeshEvent {
    let empty = MeshEvent {
        event_type: 0,
        node_id: std::ptr::null_mut(),
        data: std::ptr::null_mut(),
        sender_name: std::ptr::null_mut(),
    };

    match poll_event_internal() {
        None => empty,
        Some(event) => match event {
            NodeEvent::Started { node_id } => MeshEvent {
                event_type: 4,
                node_id: to_c_string(&node_id),
                data: std::ptr::null_mut(),
                sender_name: std::ptr::null_mut(),
            },
            NodeEvent::PeerConnected { node_id, display_name } => MeshEvent {
                event_type: 1,
                node_id: to_c_string(&hex::encode(node_id)),
                data: to_c_string(&display_name),
                sender_name: std::ptr::null_mut(),
            },
            NodeEvent::PeerDisconnected { node_id } => MeshEvent {
                event_type: 2,
                node_id: to_c_string(&hex::encode(node_id)),
                data: std::ptr::null_mut(),
                sender_name: std::ptr::null_mut(),
            },
            NodeEvent::MessageReceived { sender_id, sender_name, content } => MeshEvent {
                event_type: 3,
                node_id: to_c_string(&hex::encode(sender_id)),
                data: to_c_string(&content),
                sender_name: to_c_string(&sender_name),
            },
        },
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

// ---------------------------------------------------------------------------
// JNI bindings for Android (matching MeshBridge.kt in com.mesh.app)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
mod jni_bindings {
    use jni::JNIEnv;
    use jni::objects::{JClass, JObject, JString, JValue};
    use jni::sys::{jint, jstring, jobject};

    use super::*;

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshInit(
        mut env: JNIEnv,
        _class: JClass,
        name: JString,
        listen_port: jint,
        data_dir: JString,
    ) -> jint {
        let name: String = match env.get_string(&name) {
            Ok(s) => s.into(),
            Err(_) => return -1,
        };
        let data_dir: String = match env.get_string(&data_dir) {
            Ok(s) => s.into(),
            Err(_) => return -1,
        };

        match init_node(name, listen_port as u16, data_dir) {
            Ok(()) => 0,
            Err(()) => -1,
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshSendBroadcast(
        mut env: JNIEnv,
        _class: JClass,
        text: JString,
    ) -> jint {
        let text: String = match env.get_string(&text) {
            Ok(s) => s.into(),
            Err(_) => return -1,
        };

        match send_broadcast(&text) {
            Ok(()) => 0,
            Err(()) => -1,
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshSendDirect(
        mut env: JNIEnv,
        _class: JClass,
        dest_hex: JString,
        text: JString,
    ) -> jint {
        let dest_str: String = match env.get_string(&dest_hex) {
            Ok(s) => s.into(),
            Err(_) => return -1,
        };
        let text: String = match env.get_string(&text) {
            Ok(s) => s.into(),
            Err(_) => return -1,
        };

        let dest_bytes = match hex::decode(&dest_str) {
            Ok(b) if b.len() == 32 => {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&b);
                arr
            }
            _ => return -1,
        };

        match send_direct(dest_bytes, &text) {
            Ok(()) => 0,
            Err(()) => -1,
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshGetNodeId(
        env: JNIEnv,
        _class: JClass,
    ) -> jstring {
        match get_node_id() {
            Some(id) => env.new_string(&id).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut()),
            None => std::ptr::null_mut(),
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_mesh_app_MeshBridge_meshGetNodeIdShort(
        env: JNIEnv,
        _class: JClass,
    ) -> jstring {
        match get_node_id_short() {
            Some(id) => env.new_string(&id).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut()),
            None => std::ptr::null_mut(),
        }
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

        let (event_type, node_id, data, sender_name) = match event {
            NodeEvent::Started { node_id } => (4i32, Some(node_id), None, None),
            NodeEvent::PeerConnected { node_id, display_name } => {
                (1, Some(hex::encode(node_id)), Some(display_name), None)
            }
            NodeEvent::PeerDisconnected { node_id } => {
                (2, Some(hex::encode(node_id)), None, None)
            }
            NodeEvent::MessageReceived { sender_id, sender_name, content } => {
                (3, Some(hex::encode(sender_id)), Some(content), Some(sender_name))
            }
        };

        // Build Java strings (null-safe)
        let j_node_id = match &node_id {
            Some(s) => match env.new_string(s) {
                Ok(js) => JObject::from(js),
                Err(_) => JObject::null(),
            },
            None => JObject::null(),
        };
        let j_data = match &data {
            Some(s) => match env.new_string(s) {
                Ok(js) => JObject::from(js),
                Err(_) => JObject::null(),
            },
            None => JObject::null(),
        };
        let j_sender_name = match &sender_name {
            Some(s) => match env.new_string(s) {
                Ok(js) => JObject::from(js),
                Err(_) => JObject::null(),
            },
            None => JObject::null(),
        };

        // Construct MeshBridge$MeshEvent(int, String?, String?, String?)
        let result = env.new_object(
            "com/mesh/app/MeshBridge$MeshEvent",
            "(ILjava/lang/String;Ljava/lang/String;Ljava/lang/String;)V",
            &[
                JValue::Int(event_type),
                JValue::Object(&j_node_id),
                JValue::Object(&j_data),
                JValue::Object(&j_sender_name),
            ],
        );

        match result {
            Ok(obj) => obj.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    }
}
