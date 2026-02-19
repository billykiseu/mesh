# Build & Run Cheat Sheet

Quick reference for building and running on this machine.

---

## Desktop

```powershell
cd D:\Mainframe\Lab\Mesh

# Build (release)
cargo build --release -p mesh-node

# Run
.\target\release\mesh-node.exe

# Run with custom name
.\target\release\mesh-node.exe alice

# Run with custom name + port
.\target\release\mesh-node.exe alice 7333

# Two nodes side by side (open two terminals)
.\target\release\mesh-node.exe alice 7332
.\target\release\mesh-node.exe bob 7333
```

Output: `target\release\mesh-node.exe` (~5.8MB)

---

## Android

### Debug APK (fastest, auto-signed)

```powershell
cd D:\Mainframe\Lab\Mesh\mesh-android
.\gradlew.bat assembleDebug
```

Output: `app\build\outputs\apk\debug\app-debug.apk`

Install directly -- no signing needed.

### Release APK (optimized, needs signing)

```powershell
cd D:\Mainframe\Lab\Mesh\mesh-android
.\gradlew.bat assembleRelease
```

Output: `app\build\outputs\apk\release\app-release-unsigned.apk`

### Sign + Install

```powershell
cd D:\Mainframe\Lab\Mesh\mesh-android

$BT = "$env:LOCALAPPDATA\Android\Sdk\build-tools\35.0.0"

# Align
& "$BT\zipalign.exe" -v -p 4 `
  app\build\outputs\apk\release\app-release-unsigned.apk `
  app\build\outputs\apk\release\app-release-aligned.apk

# Sign
& "$BT\apksigner.bat" sign `
  --ks "$env:USERPROFILE\.android\debug.keystore" `
  --ks-key-alias androiddebugkey `
  --ks-pass pass:android `
  --key-pass pass:android `
  app\build\outputs\apk\release\app-release-aligned.apk

# Install via ADB
adb install -r app\build\outputs\apk\release\app-release-aligned.apk
```

Or just copy the signed APK to your phone and open it.

---

## Tests

```powershell
cd D:\Mainframe\Lab\Mesh

# Run all 40 tests
cargo test

# Just core library tests
cargo test -p mesh-core
```

---

## One-Liner Combos

```powershell
# Build + run desktop
cargo build --release -p mesh-node; .\target\release\mesh-node.exe

# Build + install Android debug APK
cd D:\Mainframe\Lab\Mesh\mesh-android; .\gradlew.bat assembleDebug; adb install -r app\build\outputs\apk\debug\app-debug.apk

# Full release cycle: build, align, sign, install
cd D:\Mainframe\Lab\Mesh\mesh-android; .\gradlew.bat assembleRelease; $BT="$env:LOCALAPPDATA\Android\Sdk\build-tools\35.0.0"; & "$BT\zipalign.exe" -v -p 4 app\build\outputs\apk\release\app-release-unsigned.apk app\build\outputs\apk\release\app-release-aligned.apk; & "$BT\apksigner.bat" sign --ks "$env:USERPROFILE\.android\debug.keystore" --ks-key-alias androiddebugkey --ks-pass pass:android --key-pass pass:android app\build\outputs\apk\release\app-release-aligned.apk; adb install -r app\build\outputs\apk\release\app-release-aligned.apk
```

---

## Rebuild Just the Rust Native Library

If you only changed Rust code and want to recompile the `.so` without a full Gradle build:

```powershell
cd D:\Mainframe\Lab\Mesh
cargo ndk -t arm64-v8a -t x86_64 -o mesh-android\app\src\main\jniLibs build --release -p mesh-ffi
```

Then run `.\gradlew.bat assembleDebug` from `mesh-android\` to package the new `.so` into an APK.
