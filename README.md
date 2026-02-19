# Mesh Network

A peer-to-peer mesh networking system that works without internet infrastructure. Nodes discover each other over the local network, establish encrypted connections, and relay messages through the mesh. Runs on Windows (desktop GUI) and Android (mobile app).

No servers. No accounts. No tracking. Just direct, encrypted communication between devices.

---

## Features

### Communication
- **Direct messages** -- encrypted 1-to-1 text between peers
- **Broadcast messages** -- text to all nodes in the mesh
- **Public broadcasts** -- high-TTL messages that reach further (TTL 50 vs default 10)
- **SOS emergency broadcasts** -- priority messages with optional GPS coordinates
- **Voice notes** -- record and send PCM audio clips (16kHz, 16-bit, mono)
- **Voice calls** -- real-time bidirectional audio streaming (20ms frames, 50fps)

### File Transfer
- **Chunked file transfer** -- any file type, up to 100MB
- **File picker** -- native OS file dialog on desktop, Android document picker on mobile
- **Accept/decline flow** -- receiver sees file name and size before accepting

### Networking
- **Automatic discovery** -- UDP broadcast on port 7331 finds nearby nodes
- **Encrypted transport** -- X25519 key exchange + ChaCha20-Poly1305 AEAD
- **Flooding router** -- messages relay through intermediate nodes with TTL and dedup
- **Gateway detection** -- nodes that have internet access are tagged as gateways
- **Connectivity display** -- shows which network interface the mesh is using (WiFi, Ethernet, Cellular)

### Identity & Security
- **Ed25519 identity** -- each node has a persistent keypair stored on disk
- **Session encryption** -- per-peer X25519 Diffie-Hellman key exchange
- **PIN lock** -- optional app-level PIN protection (Android)
- **NUKE** -- instantly destroy your identity keypair and all data

### Platform-Specific
- **Desktop** -- egui-based GUI with dark theme, tabbed interface, peer sidebar
- **Android** -- native Kotlin app with onboarding flow, foreground service, notification controls

---

## Project Structure

```
Mesh/
+-- Cargo.toml                  # Workspace root (members: mesh-core, mesh-node, mesh-ffi)
+-- Cargo.lock
+-- build-android.sh            # Convenience script for Android builds
+-- README.md
|
+-- mesh-core/                  # Shared Rust library -- all networking logic
|   +-- Cargo.toml
|   +-- src/
|       +-- lib.rs              # Module declarations and public re-exports
|       +-- identity.rs         # Ed25519 keypair generation, save/load, signing
|       +-- crypto.rs           # X25519 key exchange, ChaCha20-Poly1305 encrypt/decrypt
|       +-- message.rs          # Wire protocol: message types, serialization, payloads
|       +-- transport.rs        # TCP listener/connector, length-prefixed framing
|       +-- discovery.rs        # UDP broadcast peer discovery (port 7331)
|       +-- router.rs           # Flooding router with TTL, dedup cache, stats
|       +-- node.rs             # Main event loop, NodeHandle API, MeshStats, commands
|       +-- peer.rs             # Peer state management, timeouts, heartbeats
|       +-- file_transfer.rs    # Chunked file send/receive, progress tracking
|       +-- gateway.rs          # Internet connectivity check, network interface detection
|
+-- mesh-node/                  # Windows desktop application
|   +-- Cargo.toml              # Dependencies: eframe, egui_extras, rfd, cpal, hex
|   +-- src/
|       +-- main.rs             # egui app: tabs, chat, peers, files, settings, voice, calls
|
+-- mesh-ffi/                   # C-compatible FFI layer for Android
|   +-- Cargo.toml              # Builds as cdylib + staticlib
|   +-- src/
|       +-- lib.rs              # C FFI functions + JNI bindings for Kotlin
|
+-- mesh-android/               # Android application
    +-- build.gradle.kts         # Root Gradle config
    +-- settings.gradle.kts
    +-- gradle.properties
    +-- gradlew / gradlew.bat
    +-- app/
        +-- build.gradle.kts     # App config, cargo-ndk integration, dependencies
        +-- src/main/
            +-- AndroidManifest.xml
            +-- java/com/mesh/app/
            |   +-- MainActivity.kt       # Main UI: tabs, chat, peers, radar, settings, voice
            |   +-- MeshBridge.kt         # JNI bridge to Rust (System.loadLibrary)
            |   +-- MeshService.kt        # Foreground service, event polling, notifications
            |   +-- OnboardingActivity.kt  # First-launch tutorial (4-page swipe)
            |   +-- PinActivity.kt         # Optional PIN lock screen
            +-- jniLibs/
                +-- arm64-v8a/libmesh_ffi.so   # Compiled Rust library (ARM64)
                +-- x86_64/libmesh_ffi.so      # Compiled Rust library (x86_64 emulator)
```

---

## Architecture

### How the Mesh Works

```
Node A                         Node B                         Node C
  |                              |                              |
  |-- UDP discovery (7331) ----->|                              |
  |<---- UDP discovery (7331) ---|                              |
  |                              |                              |
  |== TCP connect (7332) =======>|== TCP connect (7332) =======>|
  |-- X25519 key exchange ------>|-- X25519 key exchange ------>|
  |<--- X25519 key exchange -----|<--- X25519 key exchange -----|
  |                              |                              |
  |== Encrypted session ========>|== Encrypted session ========>|
  |-- Text/Voice/File ---------->|-- Relay (TTL-1) ----------->|
```

1. **Discovery**: Every 5 seconds, nodes broadcast a UDP packet on port 7331 containing their node ID, display name, listen port, and gateway status. All nodes on the same LAN subnet receive these broadcasts.

2. **Connection**: When a new peer is discovered, a TCP connection is established on port 7332. Messages are length-prefixed (4-byte big-endian length + bincode-serialized payload).

3. **Key Exchange**: Immediately after TCP connect, both peers exchange X25519 public keys. The shared secret is derived and used for ChaCha20-Poly1305 AEAD encryption.

4. **Routing**: Messages use flooding -- each node forwards received messages to all connected peers (except the sender). Deduplication uses a 32-byte random message ID with a 5-minute expiry cache (max 10,000 entries). TTL starts at 10 (50 for public broadcasts) and decrements each hop.

5. **Heartbeat**: Every 10 seconds, each node sends a Ping to all peers. Peers that don't respond within 30 seconds are pruned.

### Message Types

| Code | Type | Description |
|------|------|-------------|
| 0x01 | Discovery | UDP peer announcement |
| 0x02 | Ping | Heartbeat request |
| 0x03 | Pong | Heartbeat response |
| 0x10 | Text | Direct or broadcast text message |
| 0x11 | PublicBroadcast | Wide-reach text (TTL 50) |
| 0x12 | SOS | Emergency broadcast with optional GPS |
| 0x20 | FileChunk | File data chunk |
| 0x21 | FileOffer | File transfer offer (name, size, chunk count) |
| 0x22 | FileAccept | File transfer acceptance |
| 0x30 | Voice | Voice note (PCM audio blob) |
| 0x31 | VoiceStream | Real-time audio frame (20ms) |
| 0x32 | CallStart | Voice call initiation |
| 0x33 | CallEnd | Voice call termination |
| 0x40 | PeerExchange | Peer list sharing |
| 0x50 | KeyExchange | X25519 public key exchange |
| 0x60 | ProfileUpdate | Display name + bio update |

### Audio Format

All audio (voice notes and voice calls) uses raw PCM:
- **Sample rate**: 16,000 Hz
- **Bit depth**: 16-bit signed, little-endian
- **Channels**: 1 (mono)
- **Call frame size**: 640 bytes (320 samples = 20ms)
- **Voice note max**: ~10 seconds = 320KB

### FFI Bridge (Android)

The Android app communicates with the Rust mesh engine through JNI:

```
Kotlin (MainActivity) --> MeshBridge.kt (JNI) --> libmesh_ffi.so (Rust) --> mesh-core
```

Events flow back through polling: `MeshService` calls `meshPollEvent()` every 100ms on a background thread and broadcasts results to the activity via `LocalBroadcastManager`.

---

## Desktop Application (mesh-node)

### UI Layout

```
+------------------------------------------------------------------------+
| Mesh Network | node-abc | Peers: 2 | Port: 7332 | GW: phone | WiFi    |
+------------------------------------------------------------------------+
| [Chat]  [Peers]  [Files]  [Settings]          F1-F4: tabs  Esc: exit DM|
+------------------------------------------------------------------------+
| Peers     |  Messages                                                   |
|           |    * Node started (a1b2c3d4)                                |
| > alice   |    [You] hello mesh                                         |
|   (a1b2)  |    [DM alice] hey!                                          |
|           |    [Voice] bob (3.2s)  [Play]                                |
| > bob     |                                                             |
|   (c3d4)  |                                                             |
|   [GW]    |                                                             |
|           +------------------------------------------------------------|
|           | > [input...........................] [Mic] [+] [Send]       |
+-----------+-------------------------------------------------------------+
```

### Tabs

- **Chat** (F1): Message stream, peer sidebar, input bar with Send/Mic/File buttons
- **Peers** (F2): Table of connected peers with DM and Call actions
- **Files** (F3): File transfer table showing direction, progress, status
- **Settings** (F4): Profile editor, node info, connectivity, mesh statistics, NUKE

### Slash Commands

| Command | Description |
|---------|-------------|
| `/dm <name> <msg>` | Send a direct message |
| `/send <peer> <path>` | Send a file |
| `/accept` | Accept the latest file offer |
| `/voice <peer> <path>` | Send audio file as voice note |
| `/call <peer>` | Start a voice call |
| `/endcall` | End the current voice call |
| `/broadcast <msg>` | Send a public broadcast |
| `/sos <msg>` | Send an SOS emergency broadcast |
| `/name <name>` | Change your display name |
| `/stats` | Open settings tab and refresh stats |
| `/peers` | Switch to peers tab |
| `/nuke` | Destroy identity and exit |
| `/help` | Show command list |

---

## Android Application (mesh-android)

### App Flow

```
Launch --> OnboardingActivity (first run only, 4-page tutorial)
       --> PinActivity (if PIN is set, otherwise skip)
       --> MainActivity (main interface)
           --> MeshService (foreground service, runs in background)
```

### Tabs

- **Radar**: Peer count display, start/stop mesh node
- **Chat**: Message list, input bar with Mic/Attach/Send buttons
- **Peers**: Connected peer list, tap for actions (DM, Send File, Start Call, Copy ID)
- **Settings**: Profile editor, node info, connectivity, NUKE button

### Permissions

| Permission | Purpose |
|------------|---------|
| `INTERNET` | TCP peer connections |
| `ACCESS_NETWORK_STATE` | Network status detection |
| `ACCESS_WIFI_STATE` | WiFi discovery |
| `CHANGE_WIFI_MULTICAST_STATE` | UDP broadcast reception |
| `FOREGROUND_SERVICE` | Background mesh node |
| `FOREGROUND_SERVICE_CONNECTED_DEVICE` | Android 14+ service type |
| `POST_NOTIFICATIONS` | Service notification |
| `RECORD_AUDIO` | Voice notes and calls |

---

## Build Instructions

### Prerequisites

- **Rust** (1.70+): https://rustup.rs
- **Android SDK** with NDK (for Android builds)
- **cargo-ndk** (for Android cross-compilation)

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add Android targets (only needed for Android builds)
rustup target add aarch64-linux-android x86_64-linux-android

# Install cargo-ndk (only needed for Android builds)
cargo install cargo-ndk
```

### Desktop (Windows)

```bash
# Debug build
cargo build -p mesh-node

# Release build (optimized, stripped, ~5.8MB)
cargo build --release -p mesh-node

# Run
./target/release/mesh-node.exe [display_name] [port]

# Examples
./target/release/mesh-node.exe                    # Auto-name, port 7332
./target/release/mesh-node.exe alice              # Name: alice, port 7332
./target/release/mesh-node.exe alice 7333         # Name: alice, port 7333
```

To test locally with two nodes, open two terminals:
```bash
./target/release/mesh-node.exe alice 7332
./target/release/mesh-node.exe bob 7333
```

### Android

#### Quick Build (debug APK)

```bash
# From the workspace root
./build-android.sh

# APK output: mesh-android/app/build/outputs/apk/debug/app-debug.apk
```

#### Release Build

```bash
cd mesh-android

# Build release APK (includes cargo-ndk cross-compilation)
./gradlew assembleRelease

# APK output: app/build/outputs/apk/release/app-release-unsigned.apk
```

#### Sign the APK (for sideloading)

```bash
# Align the APK
zipalign -v -p 4 \
  app/build/outputs/apk/release/app-release-unsigned.apk \
  app/build/outputs/apk/release/app-release-aligned.apk

# Sign with debug keystore
apksigner sign \
  --ks ~/.android/debug.keystore \
  --ks-key-alias androiddebugkey \
  --ks-pass pass:android \
  --key-pass pass:android \
  app/build/outputs/apk/release/app-release-aligned.apk

# Verify
apksigner verify app/build/outputs/apk/release/app-release-aligned.apk
```

The `zipalign` and `apksigner` tools are in your Android SDK build-tools directory:
```
$ANDROID_HOME/build-tools/<version>/zipalign
$ANDROID_HOME/build-tools/<version>/apksigner
```

#### Install on Device

```bash
# Via USB (ADB)
adb install app/build/outputs/apk/release/app-release-aligned.apk

# Or transfer the APK file to your phone and open it
```

### Run Tests

```bash
# All tests (40 tests across crypto, identity, message, routing, gateway)
cargo test

# Specific crate
cargo test -p mesh-core

# Specific test
cargo test -p mesh-core test_encrypt_decrypt
```

### Build Just the Rust FFI Library (without Gradle)

```bash
# Build for Android ARM64
cargo ndk -t arm64-v8a build --release -p mesh-ffi

# Build for Android x86_64 (emulator)
cargo ndk -t x86_64 build --release -p mesh-ffi

# Build for both and output to jniLibs
cargo ndk \
  -t arm64-v8a \
  -t x86_64 \
  -o mesh-android/app/src/main/jniLibs \
  build --release -p mesh-ffi
```

---

## Network Ports

| Port | Protocol | Purpose |
|------|----------|---------|
| 7331 | UDP | Peer discovery broadcasts |
| 7332 | TCP | Peer connections (configurable via CLI arg) |

Both ports must be reachable on the local network. Devices must be on the same subnet for UDP discovery to work (same WiFi network, or hotspot).

---

## Dependencies

### mesh-core
| Crate | Version | Purpose |
|-------|---------|---------|
| tokio | 1.x | Async runtime |
| serde + bincode | 1.x | Message serialization |
| ed25519-dalek | 2.x | Identity keypair (signing) |
| x25519-dalek | 2.x | Key exchange (ECDH) |
| chacha20poly1305 | 0.10 | Symmetric encryption (AEAD) |
| rand | 0.8 | Random number generation |
| sha2 | 0.10 | Hashing |
| hex | 0.4 | Hex encoding |
| socket2 | 0.5 | SO_REUSEADDR for UDP |
| if-addrs | 0.13 | Network interface detection |
| tracing | 0.1 | Structured logging |
| anyhow | 1.x | Error handling |

### mesh-node (desktop)
| Crate | Version | Purpose |
|-------|---------|---------|
| eframe | 0.31 | egui desktop framework (glow backend) |
| egui_extras | 0.31 | Table widget |
| rfd | 0.15 | Native file dialogs |
| cpal | 0.15 | Cross-platform audio I/O |

### mesh-ffi (Android)
| Crate | Version | Purpose |
|-------|---------|---------|
| once_cell | 1.x | Global singleton state |
| jni | 0.21 | JNI bindings for Kotlin |

---

## Test Coverage

40 unit tests across mesh-core:

| Module | Tests | Coverage |
|--------|-------|----------|
| crypto | 6 | Key exchange, encrypt/decrypt, tampering, wrong key |
| identity | 5 | Generation, save/load, sign/verify |
| message | 11 | Serialization, framing, all payload types |
| router | 10 | Dedup, TTL, forwarding, broadcast, SOS priority |
| gateway | 3 | Internet check, interface detection, interface classification |
| file_transfer | 2 | Roundtrip transfer, size limit |

---

## Identity Files

Node identity is stored as an Ed25519 keypair on disk:
- **Desktop**: `mesh_identity_<port>.key` in the working directory
- **Android**: `mesh_identity.key` in the app's internal files directory

These files contain your private key. If lost, your identity cannot be recovered. The NUKE function securely deletes this file.
