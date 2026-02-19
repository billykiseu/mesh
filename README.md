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

## Build Instructions (Windows)

All commands below are for **Windows** using PowerShell or the VS Code integrated terminal. If you use Git Bash (comes with Git for Windows), the commands are the same but paths use forward slashes.

### Prerequisites

You need the following installed before you can build anything. Here's exactly what to install and how.

#### 1. Git

Download and install from https://git-scm.com/download/win

Run the installer with defaults. This also gives you **Git Bash** which is useful for running shell scripts.

Verify:
```powershell
git --version
```

#### 2. Visual Studio C++ Build Tools

Rust on Windows compiles against the MSVC toolchain. You need the C++ build tools (not the full Visual Studio IDE).

1. Download **Visual Studio Build Tools** from https://visualstudio.microsoft.com/visual-cpp-build-tools/
2. Run the installer
3. Select **"Desktop development with C++"** workload
4. Click Install (downloads ~2-3GB)

This provides `cl.exe`, `link.exe`, and the Windows SDK headers that Rust and cpal need.

#### 3. Rust

1. Download `rustup-init.exe` from https://rustup.rs
2. Run it -- accept all defaults (installs to `%USERPROFILE%\.cargo`)
3. Close and reopen your terminal so `cargo` is on PATH

Verify:
```powershell
rustc --version
cargo --version
```

#### 4. Android SDK + NDK (only needed for Android builds)

The easiest way is to install **Android Studio** which bundles everything:

1. Download from https://developer.android.com/studio
2. Run the installer with defaults
3. On first launch, let it download the SDK (installs to `%LOCALAPPDATA%\Android\Sdk`)
4. Open **SDK Manager** (Settings > Android SDK) and install:
   - Under **SDK Platforms**: Android 14 (API 34)
   - Under **SDK Tools**: check **NDK (Side by side)** and **Android SDK Build-Tools**
5. Set the environment variable so Gradle and cargo-ndk can find it:

```powershell
# Add to your system environment variables (System Properties > Environment Variables)
# Variable: ANDROID_HOME
# Value:    C:\Users\<YourUsername>\AppData\Local\Android\Sdk

# Or set it temporarily in PowerShell:
$env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk"
```

If you don't want the full Android Studio IDE, you can download just the **command-line tools** from the same page (scroll to bottom), but Android Studio is easier.

#### 5. Rust Android Targets + cargo-ndk (only needed for Android builds)

Open a terminal and run:
```powershell
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
```

#### 6. Java JDK 17 (only needed for Android builds)

Android Gradle requires JDK 17. Android Studio bundles one, but if Gradle can't find it:

1. Download from https://adoptium.net/temurin/releases/ (pick JDK 17, Windows, x64, .msi)
2. Install it
3. Set `JAVA_HOME`:

```powershell
$env:JAVA_HOME = "C:\Program Files\Eclipse Adoptium\jdk-17.x.x-hotspot"
```

Android Studio's bundled JDK is at `C:\Program Files\Android\Android Studio\jbr` -- you can use that too.

#### Summary: What You Need

| Tool | Required For | Install From |
|------|-------------|-------------|
| Git | Cloning the repo | https://git-scm.com/download/win |
| VS C++ Build Tools | Compiling Rust on Windows | https://visualstudio.microsoft.com/visual-cpp-build-tools/ |
| Rust (rustup) | All Rust compilation | https://rustup.rs |
| Android Studio | Android SDK, NDK, emulator | https://developer.android.com/studio |
| cargo-ndk | Cross-compiling Rust for Android | `cargo install cargo-ndk` |
| JDK 17 | Android Gradle builds | https://adoptium.net or bundled with Android Studio |

For **desktop-only** builds, you only need Git, VS C++ Build Tools, and Rust.

---

### Clone and Build

```powershell
git clone <your-repo-url>
cd Mesh
```

### Desktop Build (Windows)

```powershell
# Debug build (fast compile, slow runtime, ~15MB)
cargo build -p mesh-node

# Release build (slow compile, optimized + stripped, ~5.8MB)
cargo build --release -p mesh-node
```

The output binary is at:
```
target\release\mesh-node.exe
```

Run it:
```powershell
# Default name and port
.\target\release\mesh-node.exe

# Custom display name
.\target\release\mesh-node.exe alice

# Custom name and port
.\target\release\mesh-node.exe alice 7333
```

To test locally with two nodes, open two separate terminals:
```powershell
# Terminal 1
.\target\release\mesh-node.exe alice 7332

# Terminal 2
.\target\release\mesh-node.exe bob 7333
```

Windows Firewall will prompt you to allow network access the first time -- click **Allow**.

### Android Build

#### Debug APK (quick, no signing needed)

```powershell
cd mesh-android
.\gradlew.bat assembleDebug
```

Output: `app\build\outputs\apk\debug\app-debug.apk`

This APK is automatically signed with a debug key and can be installed directly.

#### Release APK

```powershell
cd mesh-android
.\gradlew.bat assembleRelease
```

Output: `app\build\outputs\apk\release\app-release-unsigned.apk`

This APK is **unsigned** and must be signed before it can be installed on a device.

#### Sign the Release APK

The signing tools are in your Android SDK build-tools directory. Find your version:

```powershell
dir "$env:LOCALAPPDATA\Android\Sdk\build-tools"
```

Then sign (replace `35.0.0` with your version):

```powershell
# Set the build tools path for convenience
$BT = "$env:LOCALAPPDATA\Android\Sdk\build-tools\35.0.0"

# Step 1: Align the APK
& "$BT\zipalign.exe" -v -p 4 `
  app\build\outputs\apk\release\app-release-unsigned.apk `
  app\build\outputs\apk\release\app-release-aligned.apk

# Step 2: Sign with the debug keystore
& "$BT\apksigner.bat" sign `
  --ks "$env:USERPROFILE\.android\debug.keystore" `
  --ks-key-alias androiddebugkey `
  --ks-pass pass:android `
  --key-pass pass:android `
  app\build\outputs\apk\release\app-release-aligned.apk

# Step 3: Verify the signature
& "$BT\apksigner.bat" verify app\build\outputs\apk\release\app-release-aligned.apk
```

If you don't have a debug keystore yet (first time building Android on this machine), create one:
```powershell
keytool -genkey -v -keystore "$env:USERPROFILE\.android\debug.keystore" `
  -storepass android -alias androiddebugkey -keypass android `
  -keyalg RSA -keysize 2048 -validity 10000 `
  -dname "CN=Android Debug,O=Android,C=US"
```

#### Install on Phone

**Via ADB (USB debugging enabled on phone):**
```powershell
adb install app\build\outputs\apk\release\app-release-aligned.apk

# If upgrading over an existing install:
adb install -r app\build\outputs\apk\release\app-release-aligned.apk
```

**Without ADB:** Copy the signed APK to your phone (USB cable, cloud drive, email to yourself) and open it. You may need to enable "Install from unknown sources" in Android settings.

### Run Tests

```powershell
# All 40 tests
cargo test

# Just mesh-core tests
cargo test -p mesh-core

# A specific test by name
cargo test -p mesh-core test_encrypt_decrypt

# With output shown
cargo test -- --nocapture
```

### Build Just the Rust FFI Library (without Gradle)

If you only want to compile the native `.so` files without building the full APK:

```powershell
# ARM64 (physical phones)
cargo ndk -t arm64-v8a build --release -p mesh-ffi

# x86_64 (Android emulator)
cargo ndk -t x86_64 build --release -p mesh-ffi

# Both targets, output directly into jniLibs
cargo ndk -t arm64-v8a -t x86_64 -o mesh-android\app\src\main\jniLibs build --release -p mesh-ffi
```

### Troubleshooting

**"linker `link.exe` not found"** -- You need the VS C++ Build Tools installed (step 2 above).

**"failed to run custom build command for cpal"** -- cpal needs Windows SDK headers. Make sure you selected "Desktop development with C++" during VS Build Tools install.

**Gradle: "ANDROID_HOME is not set"** -- Set the environment variable:
```powershell
$env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk"
```
Or add it permanently in System Properties > Environment Variables.

**Gradle: "NDK not installed"** -- Open Android Studio > SDK Manager > SDK Tools > check "NDK (Side by side)" > Apply.

**cargo-ndk: "no such command"** -- Run `cargo install cargo-ndk` and make sure `%USERPROFILE%\.cargo\bin` is on your PATH.

**"Access denied" or firewall popup** -- Windows Firewall blocks new apps from listening on network ports. Click "Allow access" when prompted, or pre-allow ports 7331 (UDP) and 7332 (TCP) in Windows Firewall settings.

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
