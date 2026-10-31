# Android (APK) port assessment

> Written 2026-10-31 against `main` at clone time. Not committed upstream; a planning note for a
> downstream Android build.

## Verdict (tablet)

**Viable on a 10"+ Android tablet in landscape — and the scaffold is now in the tree.** The hard
part was never rendering (eframe/egui already runs on Android) or the engine (100% pure Rust,
portable to `wasm32`). It is the mobile platform surface: file pickers, storage, lifecycle and a UI
designed around a 1440 × 900 desktop window.

Tablet is the right target. The layout needs 760 × 480 pt minimum and reads best at 1280+ pt wide;
a 10" tablet in landscape is about 1280 × 800 pt, i.e. the desktop window it was designed for. A
phone in portrait is ~390 pt wide and stays a design project. V1 therefore locks the activity to
landscape.

Rough effort for one experienced Rust + Android developer, full time:

| Milestone | State | Estimate |
| --- | --- | --- |
| Debug APK that launches | **scaffolded** (`apps/photocraft-android`, `packaging/android`) | 3–5 days (done) |
| Open, edit, save, export on a tablet | **scaffolded** (SAF open/save, prefs, autosave, presets, back) | 1–2 weeks (done, unbuilt) |
| Daily-driver tablet build | clipboard images, suspend flush, safe-area insets, touch polish, CI, device testing | 3–6 weeks |
| Play-Store-ready | signing, AAB, 16 KB page size, attribution, crash reporting, device matrix | +1–2 weeks |
| Phone-portrait quality | touch-first layout, panel redesign, gesture model | months |

### What the scaffold already does

- `apps/photocraft-android`: `android_main`, eframe on GameActivity + wgpu, the same `PhotocraftApp`.
- `crates/android-jni`: the safe wrapper over the JNI calls (isolated `unsafe`, see below).
- `packaging/android`: Gradle project, `MainActivity` (SAF pickers, result polling, back gesture),
  manifest (no permissions), icon.
- `Services`: File › Open and Save As through the Storage Access Framework (multi-select,
  display names, save-back to the content URI), preferences, **crash-recovery autosave**, the
  **brush preset store**, link opening and the back gesture → normal close path.
- Pen pressure works without extra code: winit forwards `MotionEvent` pressure as `Touch.force`,
  which `input/stylus` already consumes.

### Not in the scaffold

- Image clipboard (text copy/paste works in-app through egui's simulated clipboard).
- Flushing autosaves on `onPause` (autosaves are still periodic; process death can lose the last
  interval).
- Safe-area insets and IME insets (winit has no API; system bars are hidden instead).
- Folder pickers, printing, display profiles, update checks, the control/MCP server.
- Path-based siblings for linked smart objects (content URIs only).

### A note on `unsafe`

JNI needs raw `JavaVM`/`jobject` handles, so the bridge lives in
`crates/android-jni` — a second isolated `unsafe` helper next to `photocraft-tablet`
(`unsafe_code = "deny"`, every block with a `SAFETY:` comment, safe public API). AGENTS.md currently
names `photocraft-tablet` as the only exception, so upstreaming this needs that rule amended; there
is no safe-only way to reach the activity from a native thread.

## What already works in our favour

- **No C/C++ in the core.** The workspace is pure Rust except `ring` (via `craft-update`'s rustls),
  which cross-compiles with the NDK. That is the single biggest cross-compilation risk removed.
- **eframe/egui support Android.** The pinned fork (`storytold/egui`) keeps upstream's
  `android-game-activity` / `android-native-activity` features. `egui-winit` already cfg's `arboard`
  out on Android and falls back to a simulated clipboard, so the `clipboard` feature is safe.
  `eframe` drops and recreates the wgpu window on `suspended()`/`resumed()`.
- **`Services` is the port seam.** Every platform dependency of the UI (file dialogs, clipboard,
  prefs, autosave, printers, beeps, URL opening, native menus) is an injected callback in
  `photocraft_ui_egui::Services`. The web shell (`apps/photocraft-web/src/web.rs`, 586 lines) proves
  a non-desktop shell works; an Android shell is the same shape, not a fork of the app.
- **Touch is partially handled already.** egui maps single touch to pointer input and long-press to
  a secondary click. The app consumes pinch (`canvas/wheel_nav.rs`), two-finger rotate
  (`canvas/rotate_view.rs`) and has browser touch de-duplication (`input/touch_twins.rs`). winit on
  Android reports `MotionEvent` pressure as `Touch.force`, which `input/stylus` can use.
- **CPU fallback exists.** `--safe-gpu` / `PHOTOCRAFT_CPU_CANVAS` runs the CPU compositor for devices
  where Vulkan misbehaves, and eframe's `memory_warning` hook is available to drop caches.
- **Autosave/recovery exists** (`.pcraft` recovery store), so process death is survivable once it is
  wired to `onPause`.

## What is missing (the actual work)

### 1. A new app crate, not changes to `apps/photocraft` — **done**

`apps/photocraft-android` has `crate-type = ["cdylib", "lib"]`, an `android_main`, and
`eframe::NativeOptions { android_app: Some(app), .. }`. It depends on `photocraft-ui-egui` and the
engine, not on `rfd`, `arboard`, `open`, `photocraft-tablet`, `craft-update` or the desktop
`services.rs`.

Notes:

- `accesskit` (enabled by the UI crate) forces **`android-game-activity`**;
  `android-native-activity` + accesskit is a `compile_error!` in eframe.
- The workspace forbids `unsafe_code`; `#[unsafe(no_mangle)]` and JNI need an explicit allow, so the
  new crate should not inherit `[lints] workspace = true` wholesale.
- Desktop-only abstractions in the UI crate are gated on `not(target_arch = "wasm32")`, which is
  *true* on Android. The main ones to re-gate to "desktop": `craft-ui/native-windows`, the
  `CUSTOM_TITLEBAR` assumptions, float-to-OS-window viewports (`show_viewport_immediate` in Camera
  Raw, screen picker, tab tear-off), and printing (`craft-print` / `photocraft-ipp` compile but
  cannot print).

### 2. Storage and the Settings directory — **done**

`apps/photocraft-android/src/services.rs` resolves `Context.getFilesDir()` over JNI once and keeps
preferences, `Presets/`, `Recovery/` and eframe's `ui.ron` there; `std::fs` works inside it.
Arbitrary user files still go through SAF (below) because of scoped storage.

### 3. Storage Access Framework (SAF) — **done**

`MainActivity` starts `ACTION_OPEN_DOCUMENT` (multi-select) and `ACTION_CREATE_DOCUMENT`, keeps the
persistable read permission, and reports the result through a polled string (`open:…`, `save:…`,
`cancel`, `back`). Rust reads the provider's display name (so the importer picks the right codec)
and the bytes, and writes back through `openOutputStream`. Both sides live in
`packaging/android/.../MainActivity.java` and `crates/android-jni` + `services.rs`.

Remaining SAF gaps: `ACTION_OPEN_DOCUMENT_TREE` (folder pickers) and content URIs cannot stand in
for sibling paths in linked smart objects.

### 4. Lifecycle, back button, insets

- **Autosave is wired** (`RecoveryStore` in the app's files directory), but **`onPause` does not
  flush it yet**: eframe's `suspended()` only saves eframe's own persistence (`ui.ron`). Add an
  `ActivityLifecycleCallbacks` hook (or an `on_suspend` hook in the egui fork) that calls
  `Services::flush_autosaves`.
- **Back is done.** `MainActivity.onBackPressed` sets a flag that Rust drains as `OsEvent::Quit`, so
  back goes through the normal close prompt instead of killing the activity.
- **Safe areas are not done.** winit still has no safe-area insets
  (rust-windowing/winit#3910); the app hides the system bars and draws behind the cutout. Reading
  `WindowInsets` over JNI is the next step for notches and gesture bars.
- **Assume process death.** Android may kill a paused app; autosave makes that survivable, but the
  last autosave interval can be lost until `onPause` is handled.

### 5. Clipboard, URL, beep, updates

- Clipboard images: not wired. `arboard` has no Android backend and `egui-winit` simulates only
  *text*. Image copy/paste across apps needs `ClipboardManager` + `Bitmap` over JNI.
- `open_url`: **done** (`ACTION_VIEW`).
- Beep and update checks: omitted (updates belong to the store).

### 6. UI on a tablet

- `CUSTOM_TITLEBAR` is off and the in-window menus are used (they already exist for Windows/Linux,
  and the app supports `--in-window-menus`). `app.custom_titlebar = false` and
  `app.integrated_titlebar = false` are set in `entry.rs`.
- The minimum window size is 760 × 480 pt. A 10" tablet in landscape is ~1280 × 800 pt — the same
  shape as the 1440 × 900 desktop window — so the dock layout fits. V1 locks the activity to
  `sensorLandscape`; remove that line in `AndroidManifest.xml` once panels reflow for portrait.
- Hover tooltips, 1 px hit targets, dense panels and keyboard-only commands still need a touch pass
  (the app already has long-press context menus and pinch/rotate gestures). This is the difference
  between "runs" and "usable"; budget for it.
- **Palm rejection is not implemented.** winit forwards `MotionEvent` pressure but not
  `getToolType()`, so a resting palm arrives as a finger (with pressure) and can paint. For a
  tablet artist this is the most important missing input feature after the clipboard; filtering
  `TOOL_TYPE_STYLUS`/`TOOL_TYPE_ERASER` and suppressing `TOOL_TYPE_FINGER` while a pen is near would
  fit in `crates/android-jni` + `MainActivity` (an `onTouchEvent`/`dispatchTouchEvent` filter) or in
  a winit fork that forwards tool type.
- The back gesture is wired, but there is no undo/redo gesture and no two-finger tap; consider
  mapping them once the basics are stable.

### 7. GPU and memory

- wgpu/Vulkan works on most 2017+ devices; keep the CPU canvas as the fallback and consider the
  `glow` renderer for old GPUs (the engine's GPU canvas needs wgpu, so glow only helps the chrome).
- The GPU compositor has a 1.5 GB FX cache budget and the compositor keeps whole-document snapshots.
  Android will kill the process well before desktop-class documents are happy. Add a mobile memory
  budget (smaller tile cache, lower FX budget, lower history depth) and test with 24 MP photos.
  `android:largeHeap="true"` is set in the manifest, but it is not a substitute for a budget.
- `memory_warning` (onTrimMemory) should drop caches and pause background jobs; eframe already
  exposes the event.

### 8. Packaging and CI

- **Scaffolded:** `packaging/android` builds with cargo-ndk + Gradle. Targets: `aarch64-linux-android`.
- NDK r27+; the scaffold sets minSdk 28 and targetSdk 35.
- Google Play requires **16 KB page-size** support for native libraries on Android 15+ — add
  `RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384"` with older linkers (NDK r28 defaults to it)
  and verify with `zipalign -c -P 16`.
- Ship as AAB with per-ABI splits; arm64-only APK is ~40–70 MB (wgpu + fonts + all codecs).
- **Still to add:** a CI job (ubuntu-latest + `android-actions/setup-android` + NDK, caches for
  `~/.cargo` and `~/.gradle`, upload the APK). CI rules in AGENTS.md say not to add workflows in a
  feature PR, so this belongs in the shared `craft-common-actions` first.
- Ship `ATTRIBUTION.md`/`NOTICE` in the APK assets; the licences are permissive but attribution is
  required. No storage or network permission is declared, which keeps the data-safety form trivial.

## Remaining work, in order

1. **Build it once** and fix the compile/packaging details the scaffold could not verify without a
   toolchain (the `jni` 0.22 call sites are the most likely to need a touch). Regenerate
   `Cargo.lock` (new package + `android_logger`) and commit it.
2. **Flush autosaves on `onPause`**, then wire `onTrimMemory` to drop caches. These two make the app
   safe to background on a tablet.
3. **Image clipboard** over JNI (`ClipboardManager` + `Bitmap`), then folder pickers if needed.
4. **Insets + IME**, then a touch pass on the panels, toolbar and menus at 1280 × 800.
5. **Device matrix and performance**: 24 MP edits, a 4 GB device, a mid-range Mali/Adreno GPU.
6. **CI + release signing + AAB**, and the store listing.

## Environment this machine still needs

Verified on the clone: no Rust toolchain, no JDK, no Android SDK/NDK.

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add aarch64-linux-android
brew install --cask temurin@21
brew install --cask android-commandlinetools
sdkmanager --sdk_root="$HOME/Library/Android/sdk" --install \
  "platform-tools" "build-tools;35.0.0" "platforms;android-35" "ndk;27.2.12479018"
cargo install cargo-ndk
brew install gradle   # once, to generate packaging/android/gradlew
```

Then:

```sh
cd packaging/android
gradle wrapper --gradle-version 8.9
cargo ndk -t arm64-v8a -o app/src/main/jniLibs \
  build --manifest-path ../../apps/photocraft-android/Cargo.toml
./gradlew installDebug
```
