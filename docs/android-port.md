# PhotoCraft on Android

A complete guide to the Android port: what was added, why, how to build it, how to debug it, and
what is still missing. It is written so someone can reproduce the port from a fresh clone, or port
the same app to another Android target.

> **Status:** first alpha. Verified on a Xiaomi 2406APNFAG (Android 15, arm64): launch, landscape UI,
> File › Open through the Storage Access Framework, and Save As writing a 14 MB PSD back to the
> content URI. The APK is a debug build.
>
> **This repository:** <https://github.com/aminghaseminia/photocraft-android> (public)
> **Upstream:** <https://github.com/storytold/photocraft>

---

## 1. What this is

PhotoCraft is a Rust image editor: an engine (`photocraft-engine` and the library crates), an egui
UI (`photocraft-ui-egui`), and a thin shell per platform that owns the window, files, clipboard and
lifecycle. The desktop shell is `apps/photocraft`; the browser shell is `apps/photocraft-web`.

The Android port adds a third shell. It does **not** fork the editor:

| Piece | Path | Role |
| --- | --- | --- |
| Android shell | `apps/photocraft-android` | `android_main`, eframe/wgpu window, `Services` |
| JNI bridge | `crates/android-jni` | safe wrapper over the JNI calls (the only `unsafe`) |
| Gradle project | `packaging/android` | APK packaging, `MainActivity`, manifest, icon |
| Plan/status | this document | how it works, how to build, what is left |

The engine, document model, filters, tools, PSD/PNG/JPEG codecs and the UI are the same code the
desktop and web builds run. Android-specific behaviour is injected through
`photocraft_ui_egui::Services`, the same seam the desktop and web shells use.

## 2. Status

| Area | State |
| --- | --- |
| Builds an arm64 APK | yes |
| Launch, render, menu, canvas | yes (wgpu/Vulkan) |
| File › Open (SAF, multi-select) | yes |
| Save / Save As to a content URI | yes |
| Preferences | yes (`files/preferences.json`) |
| Crash-recovery autosave | yes (`files/Recovery`) |
| Brush presets / actions / patterns | yes (`files/Presets`) |
| Background saves with progress/cancel | yes (`Services::save_file`) |
| Back gesture | yes (normal close path) |
| Pen pressure | yes (winit forwards `MotionEvent` pressure) |
| Image clipboard, palm rejection, folder pickers | no |
| Flush autosaves on `onPause` | no |
| Safe-area / IME insets | no (system bars are hidden) |
| Printing, update checks, control/MCP server | no |
| Release signing, AAB, CI, device matrix | no |

The layout is desktop-shaped (760 × 480 pt minimum, left tool column, right panels). V1 targets a
tablet in landscape and locks the activity to `sensorLandscape`; it also runs on a phone, but the
panels are cramped. A 10" tablet in landscape is ~1280 × 800 pt, the same shape as the 1440 × 900
desktop window.

## 3. Architecture

### 3.1 Startup path

1. `GameActivity` (from `androidx.games:games-activity`) loads `libphotocraft_android.so` and calls
   the exported `android_main`.
2. `android-activity` (through winit) has already initialized `ndk-context` and, before
   `android_main`, calls the optional `android_on_create` export.
3. `android_on_create` caches the **Activity** as a JNI global reference. This matters:
   `ndk-context` publishes the **Application** (android-activity calls `get_application`), which has
   `getFilesDir()` but none of the app's methods.
4. `android_main` initializes logging and the JVM singleton, builds `eframe::NativeOptions` with
   `android_app`, and calls `eframe::run_native` with a closure that constructs `PhotocraftApp`.
5. eframe/winit creates the surface, egui runs, and every platform action goes through `Services`.

### 3.2 The JNI bridge (`crates/android-jni`)

JNI needs raw `JavaVM` and `jobject` handles, and its constructors are `unsafe`. The bridge is an
isolated helper crate, in the same spirit as `photocraft-tablet`:

- `[lints.rust] unsafe_code = "deny"` in the manifest;
- `#![allow(unsafe_code)]` only in `src/bridge.rs`, with a `SAFETY:` comment on every block;
- the public API is safe and returns `Result<_, String>`;
- host builds compile an empty crate (`#![cfg(target_os = "android")]`), so
  `cargo clippy --workspace`/`cargo xtask wasm` on a desktop still pass.

Public functions: `init`, `files_dir`, `open_documents`, `create_document`, `take_result`,
`document_name`, `read_all`, `write_all`, `open_url`.

The Activity capture:

```rust
static ACTIVITY: Mutex<Option<Global<JObject<'static>>>> = Mutex::new(None);

#[unsafe(no_mangle)]
fn android_on_create(state: &android_activity::OnCreateState) {
    let vm = unsafe { JavaVM::from_raw(state.vm_as_ptr().cast()) };
    let raw = state.activity_as_ptr() as jni::sys::jobject;
    let _ = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        // SAFETY: android-activity owns this global reference for the callback.
        let activity = unsafe { JObject::from_raw(env, raw) };
        let global = env.new_global_ref(activity)?;
        *ACTIVITY.lock().unwrap_or_else(|p| p.into_inner()) = Some(global);
        Ok(())
    });
}

fn activity<'local>(env: &Env<'local>) -> JObject<'local> {
    let slot = ACTIVITY.lock().unwrap_or_else(|p| p.into_inner());
    match slot.as_ref() {
        // SAFETY: the Global keeps the reference alive; the wrapper does not delete it.
        Some(global) => unsafe { JObject::from_raw(env, global.as_obj().as_raw()) },
        None => JObject::null(), // a null object makes the JNI call return a clear error
    }
}
```

All calls are instance methods on the activity (`call_method`), never `FindClass` on a native
thread. That avoids the bootstrap-classloader trap described in Android's JNI tips.

### 3.3 The shell (`apps/photocraft-android`)

`src/entry.rs` is the only place with `unsafe` (the `#[unsafe(no_mangle)]` export), so it has
`#![allow(unsafe_code)]` while the crate sets `unsafe_code = "deny"`.

```rust
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    init_logging();
    let options = eframe::NativeOptions {
        android_app: Some(app),
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]),
        renderer: eframe::Renderer::Wgpu,
        system_font_fallback: false,
        ..Default::default()
    };
    let _ = eframe::run_native("PhotoCraft", options, Box::new(|cc| {
        PhotocraftApp::setup_context(&cc.egui_ctx, ThemeKind::Pro);
        let mut services = services::services();
        services.preset_store = services::presets();
        let mut app = PhotocraftApp::new(Session::new(), services);
        app.custom_titlebar = false;
        app.integrated_titlebar = false;
        app.background_jobs = true;
        if let Some(rs) = cc.wgpu_render_state.clone() {
            app.set_wgpu(rs);
        }
        Ok(Box::new(app))
    }));
}
```

`src/services.rs` implements the subset of `Services` Android can provide:

| Service | Implementation |
| --- | --- |
| `import` / `export` | `photocraft_io` directly (pure Rust) |
| `file_dialog` | SAF pickers via JNI, result polled (below) |
| `save_file` | encode on a worker thread, write via JNI, cancellable |
| `write` | content URI via JNI, plain paths via `photocraft_format::atomic_write` |
| `load_prefs` / `save_prefs` | `files/preferences.json` |
| autosave / recover / adopt / discard | `photocraft_format::RecoveryStore` in `files/Recovery` |
| `preset_store` | `photocraft_engine::preset_store::open_dir_async(files/Presets)` |
| `os_events` | polls the shell result string; `back` → `OsEvent::Quit` |
| `open_url` | `ACTION_VIEW` |
| clipboard images, printing, updates | absent (defaults) |

### 3.4 File access: SAF and content URIs

Android gives an app content URIs, not filesystem paths. The port uses the Storage Access Framework:

- **Open**: `ACTION_OPEN_DOCUMENT`, `EXTRA_ALLOW_MULTIPLE`, persistable read permission.
- **Save As**: `ACTION_CREATE_DOCUMENT` with the suggested name and a generic MIME type.
- **Read**: `ContentResolver.openInputStream` → bytes (same model as the browser).
- **Write**: `ContentResolver.openOutputStream(uri, "wt")`.

Pickups are asynchronous. `MainActivity` stores the result in a string and Rust polls it once per
frame:

```
open:<uri>\n<uri>…     save:<uri>     cancel     back
```

Rust turns that into `FileDialogAnswer::ContentsMany`, `Contents` or `SaveTo`, and reads the bytes on
a worker thread (never on the UI thread).

**The extension trap.** The app appends the chosen format's extension to the document path, turning
`content://…/document/1299` into `…/document/1299.psd`. SAF granted the URI *without* the extension,
so writing to the suffixed URI fails with:

```
Permission Denial: … requires android.permission.MANAGE_DOCUMENTS, or grantUriPermission()
```

The shell therefore remembers the granted URI (`SAVED_URIS`) and maps any path that is
`<granted>` or `<granted>.<something>` back to it before writing. `export_name` likewise reads the
format from the path's extension first, then from the provider's display name. That second path is
needed because the provider deduplicates names as `foo.psd (1)` — the suffix sits *after* the
extension, so a naive `Path::extension()` returns `1`.

### 3.5 Threads and lifecycle

- `android_main` runs on a native thread; eframe drives the loop.
- JNI calls attach the calling thread (the JVM detaches it at thread exit).
- File reads/saves run on worker threads; `FileDialogReply` is `Send`, so an answer can be sent
  from any thread.
- eframe drops and recreates the wgpu window on `suspended()`/`resumed()`.
- The Activity can be destroyed and recreated; autosave covers process death, but autosaves are not
  flushed on `onPause` yet.

## 4. Prerequisites

### 4.1 Host tools (macOS, Apple Silicon)

```sh
# Rust + the Android target
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
~/.cargo/bin/rustup target add aarch64-linux-android

# JDK 21 (Temurin), no installer/sudo: extract to ~/Library/Java/JavaVirtualMachines
curl -L -o /tmp/jdk.tar.gz \
  "https://api.adoptium.net/v3/binary/latest/21/ga/mac/aarch64/jdk/hotspot/normal/eclipse"
mkdir -p ~/Library/Java/JavaVirtualMachines
tar xzf /tmp/jdk.tar.gz -C /tmp && mv /tmp/jdk-21* ~/Library/Java/JavaVirtualMachines/temurin-21.jdk

# Android command-line tools
mkdir -p ~/Library/Android/sdk/cmdline-tools
curl -L -o /tmp/clt.zip \
  "https://dl.google.com/android/repository/commandlinetools-mac-13114758_latest.zip"
unzip -q /tmp/clt.zip -d ~/Library/Android/sdk/cmdline-tools
mv ~/Library/Android/sdk/cmdline-tools/cmdline-tools ~/Library/Android/sdk/cmdline-tools/latest

# SDK packages: platform, build tools, NDK. No emulator, no system image.
export JAVA_HOME="$HOME/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home"
export ANDROID_HOME="$HOME/Library/Android/sdk"
yes | "$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" --sdk_root="$ANDROID_HOME" --licenses
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" --sdk_root="$ANDROID_HOME" \
  "platform-tools" "platforms;android-35" "build-tools;35.0.0" "ndk;27.2.12479018"

# Gradle (only needed once: it generates the wrapper) and cargo-ndk
brew install gradle
~/.cargo/bin/cargo install cargo-ndk
```

If GitHub/Google downloads need a proxy, add `--socks5-hostname localhost:1080` to `curl` and
`--proxy=socks --proxy_host=localhost --proxy_port=1080` to `sdkmanager`; Gradle accepts
`-DsocksProxyHost=localhost -DsocksProxyPort=1080`.

### 4.2 Phone

1. Enable Developer options → USB debugging.
2. Connect USB, accept the RSA fingerprint.
3. Some OEMs also need "Install via USB" (Xiaomi/Redmi).

### 4.3 Environment file

`~/Library/Android/sdk/env.sh`:

```sh
export JAVA_HOME="$HOME/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home"
export ANDROID_HOME="$HOME/Library/Android/sdk"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.2.12479018"
export GRADLE_HOME="$HOME/tools/gradle-8.9"
export PATH="$HOME/.cargo/bin:$JAVA_HOME/bin:$ANDROID_HOME/platform-tools:$ANDROID_HOME/cmdline-tools/latest/bin:$GRADLE_HOME/bin:$PATH"
```

## 5. Quick start

```sh
source ~/Library/Android/sdk/env.sh
cd packaging/android

# First time only: generate the wrapper (the jar is not committed).
gradle wrapper --gradle-version 8.9

# Build the Rust library into app/src/main/jniLibs/<abi>/.
cargo ndk -t arm64-v8a -o app/src/main/jniLibs \
  build --manifest-path ../../apps/photocraft-android/Cargo.toml

# AGP cannot strip this .so; strip it before packaging or the APK is ~4× larger.
"$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-strip" \
  --strip-unneeded app/src/main/jniLibs/arm64-v8a/libphotocraft_android.so

gradle assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n ai.storyteller.photocraft/.MainActivity
```

The first `cargo build` updates `Cargo.lock` for the new packages; run it without `--locked` and
commit the lock. After that `--locked` works.

## 6. Step-by-step porting guide

This is the order the port was built in. Each step ends with something testable.

### Step 1 — register the crates

- `Cargo.toml`: add `photocraft-android-jni = { path = "crates/android-jni" }` to
  `[workspace.dependencies]`. `crates/*` and `apps/*` are already workspace members.
- `xtask/src/layers.rs`: register `("android-jni", Class::Standalone)` and
  `("android", Class::Exempt)`. `cargo xtask layers` fails on an unknown workspace crate.

### Step 2 — the JNI bridge

Create `crates/android-jni`:

- `[lints.rust] unsafe_code = "deny"`, explicit clippy lints, `jni = "0.22"`,
  `ndk-context = "0.1"`, `android-activity = "0.6"` (all Android-only).
- `#![cfg(target_os = "android")]` at the crate root so host builds stay empty.
- `src/bridge.rs` with `#![allow(unsafe_code)]`, `init`, `android_on_create`, `activity`, and one
  function per activity method.
- Re-export the safe API from `src/lib.rs`.

Test it alone before the big build:

```sh
cargo check -p photocraft-android-jni --target aarch64-linux-android
```

### Step 3 — the app shell

Create `apps/photocraft-android`:

- `[lib] crate-type = ["cdylib"]`, `name = "photocraft_android"` (the `.so` and
  `System.loadLibrary` name).
- All dependencies under `[target.'cfg(target_os = "android")'.dependencies]` so host workspace
  checks see an empty crate.
- `eframe` with `wgpu`, `accesskit`, `clipboard`, and **`android-game-activity`**. `accesskit`
  cannot be combined with `android-native-activity` (eframe has a `compile_error!` for it).
- `src/entry.rs` calls `eframe::run_native`; `src/services.rs` implements `Services`.

### Step 4 — Gradle and the activity

Create `packaging/android` (see §8 for the files). The Java side:

- `MainActivity extends GameActivity`, loads `photocraft_android`, hides the system bars, keeps the
  screen on, and overrides `onActivityResult`/`onBackPressed`.
- Public `photocraft*` methods are called from Rust: open/create a document, take the pending result,
  read/write a URI, get the display name, open a URL.
- `themes.xml` **must** use `Theme.AppCompat.NoActionBar` (games-activity 4.4's `GameActivity`
  extends `AppCompatActivity`).
- The manifest needs `android.app.lib_name = photocraft_android`, no permissions, and (v1)
  `android:screenOrientation="sensorLandscape"`.

### Step 5 — build, install, iterate

Build the `.so`, strip it, `gradle assembleDebug`, `adb install`, launch, and watch logcat. Expect
to fix a handful of JNI signatures and packaging details here; the compiler is the fastest feedback.

### Step 6 — implement the services in this order

1. `import`/`export`/`encode_png` — pure Rust, no platform code.
2. `files_dir`, `load_prefs`, `save_prefs`.
3. `file_dialog` + `write` for **Open** only.
4. `file_dialog` for **Save As** + `write` for content URIs (the extension trap, §3.4).
5. `save_file` (background, cancellable) — otherwise big saves freeze the UI.
6. autosave/recovery, then the preset store.
7. `os_events` for the back gesture.
8. `open_url`, then the optional pieces.

### Step 7 — lifecycle and polish

- `onPause` → flush autosaves (not implemented; add an `ActivityLifecycleCallbacks` hook or an
  `on_suspend` hook in the egui fork).
- `onTrimMemory` → drop caches (eframe exposes `memory_warning`).
- Insets: winit has no safe-area API (rust-windowing/winit#3910); read `WindowInsets` over JNI.
- Palm rejection: filter `TOOL_TYPE_FINGER` while a stylus is near.
- Image clipboard: `ClipboardManager` + `Bitmap` over JNI.

## 7. Implementation notes and gotchas

### 7.1 Theme

`games-activity:4.4.0` extends `AppCompatActivity`, so `onCreateSurfaceView` calls
`setContentView` and crashes with:

```
java.lang.IllegalStateException: You need to use a Theme.AppCompat theme (or descendant) with this activity.
```

Use `Theme.AppCompat.NoActionBar` (the platform `Theme.Material.NoActionBar` is not enough).
`appcompat` must be a dependency.

### 7.2 Kotlin stdlib duplicates

`games-activity` and `appcompat` can pull different Kotlin stdlib versions, and Gradle fails with
`Duplicate class kotlin.collections.jdk8.CollectionsJDK8Kt`. Align them:

```groovy
implementation platform('org.jetbrains.kotlin:kotlin-bom:1.9.24')
```

### 7.3 Activity vs Application

`ndk_context::android_context().context()` is an **Application**, not the Activity (android-activity
calls `get_application` and stores that global reference). Symptoms: `Method not found:
photocraftTakeResult`, and "no settings directory" because the custom method is absent. Capture the
Activity in `android_on_create`; use `Context.getFilesDir()` (inherited by both) for the files
directory.

### 7.4 Content-URI saves

The app appends the export extension to the document path; SAF grants only the URI returned by the
picker. Remember granted URIs and map back before writing. The provider may also rename duplicates
to `name.psd (1)`: strip a trailing ` (n)` before reading the display name's extension, and prefer
the path's extension when it is a known export format.

### 7.5 Large saves and the UI thread

Without `Services::save_file`, the app encodes on the UI thread. Implement it: encode with
`photocraft_io::export_with` and a `photocraft_raster::Interrupt` built from `SaveCtl`, then write
on the same worker. JNI attaches the worker thread automatically.

### 7.6 APK size

- Debug builds carry debuginfo; the `.so` can be ~390 MB. Strip it with NDK `llvm-strip` before
  packaging (AGP may report "Unable to strip … packaging them as they are"). The APK drops to
  ~91 MB.
- A release build (`cargo ndk … --release`) is much smaller and faster; it needs signing before it
  can be installed outside `adb`.
- Do not commit the APK. Attach it to a GitHub release (§8.3).

### 7.7 Back button

winit does not turn Android back into `Escape`. `MainActivity.onBackPressed` sets a flag; Rust polls
it and returns `OsEvent::Quit`, which goes through the app's normal unsaved-changes path. Set
`android:enableOnBackInvokedCallback="false"` in the manifest so the callback is used.

## 8. Packaging

### 8.1 Gradle

`packaging/android` (AGP 8.7.3, Gradle 8.9, compileSdk 35, minSdk 28, targetSdk 35):

| File | Purpose |
| --- | --- |
| `settings.gradle` | repositories and `include ':app'` |
| `build.gradle` | AGP version |
| `gradle.properties` | AndroidX, JVM heap |
| `app/build.gradle` | SDK levels, `abiFilters 'arm64-v8a'`, no minify, platform Java 17 |
| `app/src/main/AndroidManifest.xml` | activity, theme, lib name, landscape, no permissions |
| `app/src/main/java/…/MainActivity.java` | GameActivity + SAF + JNI methods |
| `app/src/main/res/values/themes.xml` | `Theme.AppCompat.NoActionBar` |
| `app/src/main/res/mipmap-*` | launcher icons from `assets/app-icon` |
| `.gitignore` | `jniLibs/`, Gradle build dirs, keystores |

Dependencies:

```groovy
implementation platform('org.jetbrains.kotlin:kotlin-bom:1.9.24')
implementation 'androidx.appcompat:appcompat:1.7.0'
implementation 'androidx.games:games-activity:4.4.0'
```

### 8.2 Native library

- Name: `photocraft_android` → `libphotocraft_android.so`.
- Manifest meta-data `android.app.lib_name` must match.
- `abiFilters 'arm64-v8a'` for a phone/tablet build; add `x86_64` only if an emulator is used (this
  port installs no emulator).
- Android 15 requires 16 KB page-size alignment for native libraries. NDK r28+ defaults to it; with
  older linkers add `RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384"` and check with
  `zipalign -c -P 16 -v 4 app-release.apk`.

### 8.3 Release

```sh
# Debug build (debug-signed, tested)
gradle assembleDebug

# Or a signed release build
cargo ndk -t arm64-v8a -o app/src/main/jniLibs build --release --manifest-path ../../apps/photocraft-android/Cargo.toml
"$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-strip" --strip-unneeded app/src/main/jniLibs/arm64-v8a/libphotocraft_android.so
gradle bundleRelease    # .aab, after adding a signingConfig
```

Publish the APK as a release asset, not in git:

```sh
gh release create v0.18.0-android.1 \
  photocraft-android-0.18.0-arm64-debug.apk \
  --title "PhotoCraft for Android v0.18.0 (alpha 1)" \
  --notes-file release-notes.md
```

## 9. Testing on the device

```sh
adb devices -l
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n ai.storyteller.photocraft/.MainActivity
adb logcat -c
adb logcat photocraft:V AndroidRuntime:E *:S
adb exec-out screencap -p > screen.png
```

Useful input commands:

```sh
adb shell input tap 1216 515                 # tap (screen coordinates)
adb shell input swipe 400 500 400 180 300    # scroll the File menu
adb shell input keyevent 30                  # B: Brush
adb shell input keyevent 4                   # Back
adb shell input keycombination 113 59 47     # Ctrl+Shift+S (Save As)
```

Smoke checklist for a build:

- [ ] launches to the empty workspace, no banner
- [ ] File › Open → picker → image opens; tab name and status line
- [ ] File › Save As → picker → `Saved …`; file size > 0; tab loses `*`
- [ ] paint a stroke on a raster layer; undo
- [ ] pinch to zoom, two-finger rotate
- [ ] back → close prompt (no data loss)
- [ ] rotate the device (portrait is locked out in v1)
- [ ] background/foreground; no crash
- [ ] `adb logcat` free of `JNI`, `panic`, `FATAL`

Host tests are unchanged (`cargo test …`); there is no on-device test runner.

## 10. Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `You need to use a Theme.AppCompat theme` | games-activity 4.4 extends `AppCompatActivity` | parent theme `Theme.AppCompat.NoActionBar` |
| `Method not found: photocraftTakeResult` / "no settings directory" | the JNI object is the Application, not the Activity | capture the Activity in `android_on_create` |
| `Permission Denial … MANAGE_DOCUMENTS` on save | writing to a path with an extension SAF did not grant | remember the granted URI and map the path back |
| `unknown file format: foo.psd (1)` | provider dedup suffix after the extension | strip ` (n)`; read the format from the path |
| `Duplicate class kotlin…` | two Kotlin stdlib versions | Kotlin BOM |
| `UnsatisfiedLinkError` | lib name, manifest meta-data, or ABI mismatch | `libphotocraft_android.so`, `android.app.lib_name=photocraft_android`, `arm64-v8a` |
| Black screen | no Vulkan/wgpu surface | logcat for wgpu/adapter errors; test the CPU canvas |
| APK ~395 MB | unstripped debug `.so` | `llvm-strip --strip-unneeded` before packaging |
| `adb: no devices` | USB debugging off, RSA not accepted, cable | re-plug, accept the prompt, `adb kill-server` |
| Activity closes after the splash | Java crash in `onCreate` | `adb logcat -b crash`; see the theme entry |

## 11. What is left

- Flush autosaves on `onPause`; wire `onTrimMemory`.
- Image clipboard over JNI.
- Safe-area and IME insets, then a touch pass on panels/toolbar/menus.
- Palm rejection (`TOOL_TYPE_FINGER` filter while a pen is near).
- Folder pickers (`ACTION_OPEN_DOCUMENT_TREE`) and path-based linked files.
- Display a display name in the tab instead of the URI's document id (`Services` carries a path).
- A mobile memory budget (tile cache, FX budget, history depth) and a 24 MP device test.
- Release signing, AAB, CI (shared `craft-common-actions`), device matrix, crash reporting.

## 12. Upstreaming notes

- **`unsafe` policy:** `AGENTS.md` names `photocraft-tablet` as the only isolated `unsafe` crate.
  This port adds `crates/android-jni` for the same reason (JNI raw handles, `#[no_mangle]`). If the
  port goes upstream, that rule needs amending; there is no safe-only way to call the Activity from
  a native thread.
- **Layering:** register both crates in `xtask/src/layers.rs`; `cargo xtask layers` enforces it.
- **Host checks:** keep the Android-only code behind `#![cfg(target_os = "android")]` and
  target-specific dependencies so `cargo clippy --workspace --all-targets` and `cargo xtask wasm`
  stay green.
- **CI:** `AGENTS.md` says not to add CI in a feature PR; add an Android job to the shared
  `craft-common-actions` workflows instead (ubuntu-latest + `android-actions/setup-android` + NDK,
  caches for `~/.cargo` and `~/.gradle`, upload the APK).
- **Don't commit** `jniLibs/`, `app/build/`, keystores or the APK.

## 13. Pulling upstream updates

This clone keeps the original project at `upstream` and this port at `origin`
(`https://github.com/aminghaseminia/photocraft-android`):

```sh
git fetch upstream
git rebase upstream/main
git push
```

`pull.rebase` is set for this clone, so `git pull upstream main` rebases. The files both sides touch
are `Cargo.toml`, `Cargo.lock` and `xtask/src/layers.rs` (the new crates' entries); resolve those,
then rerun the Android build and reinstall. A fresh clone needs:

```sh
git remote add upstream https://github.com/storytold/photocraft
```

## Appendix A — file map

```
apps/photocraft-android/
  Cargo.toml                 cdylib, Android-only deps
  src/lib.rs                 modules, known gaps
  src/entry.rs               android_main (the only unsafe export)
  src/services.rs            Services, SAF polling, save/export, recovery, presets

crates/android-jni/
  Cargo.toml                 unsafe_code = "deny", jni/ndk-context/android-activity
  src/lib.rs                 #![cfg(android)], re-exports
  src/bridge.rs              #![allow(unsafe_code)], JNI calls + SAF helpers

packaging/android/
  README.md                  build/install/debug instructions
  settings.gradle, build.gradle, gradle.properties
  app/build.gradle           SDK levels, ABI, Kotlin BOM
  app/src/main/AndroidManifest.xml
  app/src/main/java/ai/storyteller/photocraft/MainActivity.java
  app/src/main/res/values/{themes,strings}.xml
  app/src/main/res/mipmap-*/ic_launcher.png
  gradle/wrapper/gradle-wrapper.properties
```

## Appendix B — versions used

| Tool | Version |
| --- | --- |
| Rust | 1.99.1, target `aarch64-linux-android` |
| JDK | Temurin 21.0.13 |
| Gradle | 8.9 |
| Android Gradle Plugin | 8.7.3 |
| SDK | platform 35, build-tools 35.0.0 |
| NDK | 27.2.12479018 (r27c) |
| cargo-ndk | 4.1.2 |
| eframe / egui | 0.37 (the `storytold/egui` fork) |
| jni | 0.22.4 |
| android-activity | 0.6.1 |
| games-activity | 4.4.0 |
| minSdk / targetSdk | 28 / 35 |
| Test device | Xiaomi 2406APNFAG, Android 15, arm64 |
