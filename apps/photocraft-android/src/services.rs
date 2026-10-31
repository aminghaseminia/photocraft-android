//! The Android `Services`: imports/exports, the Storage Access Framework dialogs, preferences and
//! the back button. Everything the UI knows about the platform lives here, exactly as the desktop
//! (`apps/photocraft/src/services.rs`) and web (`apps/photocraft-web/src/web.rs`) shells do it.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use photocraft_codecs::{ChannelLayout, EncodeOptions, Image};
use photocraft_format::RecoveryStore;
use photocraft_ui_egui::{FileDialogAnswer, FileDialogReply, FileDialogRequest, OsEvent, Recoverable, Services};

use photocraft_android_jni as bridge;

/// The app's private files directory (`Context.getFilesDir()`), resolved once.
pub fn config_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = PathBuf::from(bridge::files_dir().ok()?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    })
    .clone()
}

fn prefs_file() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("preferences.json"))
}

/// A file dialog the Java picker is showing, waiting for its answer.
struct Pending {
    reply: FileDialogReply,
    kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Open,
    Save,
}

/// One picker at a time (the UI enforces that too).
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// URIs the save picker returned. The app appends the format's extension to the document path
/// (`…/document/1297` becomes `…/document/1297.psd`), but the Storage Access Framework granted
/// access to the URI *without* it. [`base_uri`] maps the document path back to the granted URI.
static SAVED_URIS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn remember_saved_uri(uri: &str) {
    let mut saved = SAVED_URIS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !saved.iter().any(|known| known == uri) {
        if saved.len() >= 64 {
            saved.remove(0);
        }
        saved.push(uri.to_string());
    }
}

/// The granted URI behind a document path the app may have given an extension.
fn base_uri(path: &str) -> String {
    let saved = SAVED_URIS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    saved
        .iter()
        .find(|uri| path == uri.as_str() || path.strip_prefix(uri.as_str()).is_some_and(|rest| rest.starts_with('.')))
        .cloned()
        .unwrap_or_else(|| path.to_string())
}

fn pending() -> MutexGuard<'static, Option<Pending>> {
    PENDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn take_pending() -> Option<Pending> {
    pending().take()
}

fn fail(error: &str) {
    if let Some(pending) = take_pending() {
        pending.reply.send(Some(FileDialogAnswer::Unavailable(error.to_string())));
    }
}

/// Starts a picker. The answer is delivered by [`poll`], not by this call.
fn show_file_dialog(request: FileDialogRequest, _parent: Option<&eframe::Frame>, reply: FileDialogReply) {
    match request {
        FileDialogRequest::Open { multiple, .. } => {
            *pending() = Some(Pending { reply, kind: Kind::Open });
            if let Err(error) = bridge::open_documents(multiple) {
                fail(&format!("The Android file picker could not open: {error}"));
            }
        }
        FileDialogRequest::Save { suggested } => {
            let name = Path::new(&suggested).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or(suggested);
            *pending() = Some(Pending { reply, kind: Kind::Save });
            // The document providers keep the title's extension, so `export` can pick the format
            // from the display name later (the URI itself carries no extension).
            if let Err(error) = bridge::create_document(&name, "application/octet-stream") {
                fail(&format!("The Android save picker could not open: {error}"));
            }
        }
        // A folder URI is not a filesystem path, and no engine command can use one yet.
        FileDialogRequest::Folder { .. } => reply.send(Some(FileDialogAnswer::Unavailable("Choosing a folder is not available on Android yet".into()))),
    }
}

/// Answers the pending dialog and turns the back button into the app's normal close path.
/// Called once per frame through `Services::os_events`.
fn poll() -> Vec<OsEvent> {
    let result = match bridge::take_result() {
        Ok(result) => result,
        Err(error) => {
            log::warn!("JNI result poll failed: {error}");
            return Vec::new();
        }
    };
    match result.as_str() {
        "" => {}
        // Android's back gesture: ask the app to close, which prompts about unsaved documents.
        "back" => return vec![OsEvent::Quit],
        "cancel" => {
            if let Some(pending) = take_pending() {
                pending.reply.send(None);
            }
        }
        other => match other.split_once(':') {
            Some(("open", uris)) => answer_open(uris),
            Some(("save", uri)) => {
                if let Some(pending) = take_pending() {
                    debug_assert_eq!(pending.kind, Kind::Save);
                    remember_saved_uri(uri);
                    pending.reply.send(Some(FileDialogAnswer::SaveTo(uri.to_string())));
                }
            }
            _ => log::warn!("unknown result from the Android shell: {other:?}"),
        },
    }
    Vec::new()
}

fn answer_open(uris: &str) {
    // Keep the reply in `PENDING` until the worker takes it, so a failed spawn can still answer.
    let wrong_kind = {
        let held = pending();
        match held.as_ref() {
            Some(waiting) if waiting.kind == Kind::Open => false,
            Some(_) => true,
            None => return,
        }
    };
    if wrong_kind {
        if let Some(waiting) = take_pending() {
            waiting.reply.send(None);
        }
        return;
    }

    let uris: Vec<String> = uris.split('\n').filter(|uri| !uri.is_empty()).map(str::to_string).collect();
    let spawned = std::thread::Builder::new().name("android-open".into()).spawn(move || {
        let Some(waiting) = take_pending() else { return };
        let mut files = Vec::new();
        let mut failure = None;
        for uri in &uris {
            match bridge::read_all(uri) {
                Ok(bytes) => {
                    let name = bridge::document_name(uri).ok().filter(|name| !name.is_empty()).unwrap_or_else(|| name_from_uri(uri));
                    files.push((name, bytes));
                }
                Err(error) => {
                    failure = Some(format!("Could not read {uri}: {error}"));
                    break;
                }
            }
        }
        match failure {
            Some(error) => waiting.reply.send(Some(FileDialogAnswer::Unavailable(error))),
            None => match files.len() {
                0 => waiting.reply.send(None),
                1 => {
                    let (name, bytes) = files.remove(0);
                    waiting.reply.send(Some(FileDialogAnswer::Contents(name, bytes)));
                }
                _ => waiting.reply.send(Some(FileDialogAnswer::ContentsMany(files))),
            },
        }
    });
    if let Err(error) = spawned {
        if let Some(waiting) = take_pending() {
            waiting.reply.send(Some(FileDialogAnswer::Unavailable(format!("Could not start the reader thread: {error}"))));
        }
    }
}

/// A last-resort display name when the provider reports none.
fn name_from_uri(uri: &str) -> String {
    uri.rsplit('/').next().filter(|name| !name.is_empty()).unwrap_or("document").to_string()
}

/// Extensions `photocraft_io::export` understands. The app appends the chosen format's extension
/// to the document path, which makes the path the format's source of truth.
const EXPORT_EXTS: &[&str] = &[
    "psd", "psb", "pdf", "pcraft", "ora", "png", "jpg", "jpeg", "jxl", "webp", "tif", "tiff", "tga", "bmp", "dib", "gif", "ico", "exr", "pnm", "hdr", "fits",
    "dpx", "dds", "cin", "iff",
];

fn has_export_ext(path: &str) -> bool {
    Path::new(path).extension().and_then(|ext| ext.to_str()).is_some_and(|ext| EXPORT_EXTS.iter().any(|known| known.eq_ignore_ascii_case(ext)))
}

/// `foo.psd (1)`: the provider's way of not overwriting an existing document. The suffix sits
/// after the extension, so the format must be read before it.
fn strip_copy_suffix(name: &str) -> &str {
    if let Some(open) = name.rfind(" (") {
        if name.ends_with(')') && name[open + 2..name.len() - 1].chars().all(|c| c.is_ascii_digit()) {
            return &name[..open];
        }
    }
    name
}

/// The export's format comes from the file name's extension, and a `content://` URI has none.
/// Ask the provider for the display name the create-document picker was given.
fn export_name(path: &str) -> String {
    if path.starts_with("content://") {
        // The app appends the format's extension (`.psd`, `.png`, …) to the document path.
        if has_export_ext(path) {
            return path.to_string();
        }
        let uri = base_uri(path);
        if let Ok(name) = bridge::document_name(&uri) {
            let name = strip_copy_suffix(&name);
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    path.to_string()
}

fn import(name: &str, bytes: &[u8], options: &photocraft_io::ImportOptions) -> Result<(photocraft_doc::Document, Vec<String>), String> {
    photocraft_io::import_with_options(name, bytes, options).map(|result| (result.document, result.warnings)).map_err(|error| error.to_string())
}

fn export(doc: &photocraft_doc::Document, path: &str, settings: &photocraft_ui_egui::ExportSettings) -> Result<(Vec<u8>, Vec<String>), String> {
    let name = export_name(path);
    photocraft_io::export(doc, &name, &settings.io_options()).map(|result| (result.bytes, result.warnings)).map_err(|error| error.to_string())
}

fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    if path.starts_with("content://") {
        return bridge::write_all(&base_uri(path), bytes);
    }
    photocraft_format::atomic_write(Path::new(path), bytes).map_err(|error| error.to_string())
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let image = Image::from_u8(width, height, ChannelLayout::Rgba, rgba.to_vec()).map_err(|error| error.to_string())?;
    photocraft_codecs::encode(&image, photocraft_codecs::Format::Png, &EncodeOptions::default()).map_err(|error| error.to_string())
}

fn save_prefs(text: &str) -> Result<(), String> {
    let path = prefs_file().ok_or("no settings directory")?;
    photocraft_format::atomic_write(&path, text.as_bytes()).map_err(|error| error.to_string())
}

/// The share of a save's progress bar that encoding fills; writing fills the rest.
const ENCODE_SHARE: f32 = 0.85;

/// A save on a worker thread (`Services::save_file`): encode, then write. Cancelling while
/// encoding leaves the target untouched; once writing starts the save can't be cancelled.
fn save_through(
    doc: &photocraft_doc::Document,
    path: &str,
    settings: &photocraft_ui_egui::ExportSettings,
    ctl: &photocraft_ui_egui::jobs_ui::SaveCtl,
    write: impl Fn(&str, &[u8], &dyn Fn(f32)) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    ctl.progress(0.0, "Encoding");
    let options = settings.io_options();
    let name = export_name(path);
    let cancelled = || ctl.check().is_err();
    let progress = |fraction: f32| ctl.progress(fraction * ENCODE_SHARE, "");
    let interrupt = photocraft_raster::Interrupt::new(&cancelled, &progress);
    let result = photocraft_io::export_with(doc, &name, &options, &interrupt).map_err(|error| {
        log::error!("save: export failed for {name:?}: {error}");
        error.to_string()
    })?;
    log::info!("save: encoded {} bytes for {name:?}", result.bytes.len());
    ctl.commit()?;
    ctl.progress(ENCODE_SHARE, "Writing");
    write(path, &result.bytes, &|fraction| ctl.progress(ENCODE_SHARE + (0.99 - ENCODE_SHARE) * fraction, "")).map_err(|error| {
        log::error!("save: write failed for {path}: {error}");
        error
    })?;
    ctl.progress(1.0, "");
    Ok(result.warnings)
}

fn save_file(
    doc: &photocraft_doc::Document,
    path: &str,
    settings: &photocraft_ui_egui::ExportSettings,
    ctl: &photocraft_ui_egui::jobs_ui::SaveCtl,
) -> Result<Vec<String>, String> {
    save_through(doc, path, settings, ctl, |path, bytes, progress| {
        if path.starts_with("content://") {
            progress(1.0);
            bridge::write_all(&base_uri(path), bytes)
        } else {
            photocraft_format::atomic_write_progress(Path::new(path), bytes, progress).map_err(|error| error.to_string())
        }
    })
}

/// Brush presets, actions and patterns, loaded in the background and attached by the app when
/// they arrive (session-only otherwise).
pub fn presets() -> Option<std::sync::mpsc::Receiver<photocraft_engine::preset_store::Opened>> {
    config_dir().map(|dir| photocraft_engine::preset_store::open_dir_async(dir.join("Presets")))
}

type SharedRecovery = Rc<RefCell<Option<RecoveryStore>>>;

fn with_store<R>(store: &SharedRecovery, f: impl FnOnce(&mut RecoveryStore) -> R) -> Result<R, String> {
    let mut slot = store.try_borrow_mut().map_err(|_| "crash recovery is busy".to_string())?;
    Ok(f(slot.as_mut().ok_or("no settings directory")?))
}

/// Crash-recovery autosaves into the app's private files directory. Android can kill a paused
/// process, so this is not optional on a tablet.
fn recovery_services(dir: Option<PathBuf>) -> Services {
    let recover_dir = dir.clone();
    let store: SharedRecovery = Rc::new(RefCell::new(dir.map(RecoveryStore::new)));
    let (s1, s2, s4, s5) = (store.clone(), store.clone(), store.clone(), store.clone());
    Services {
        autosave: Some(Box::new(move |doc: &Arc<photocraft_doc::Document>, revision: u64, path: Option<&str>| {
            with_store(&s1, |store| store.autosave_checked(doc, revision, path.map(str::to_string)))?
        })),
        autosave_results: Some(Box::new(move || with_store(&s4, RecoveryStore::take_completed).unwrap_or_default())),
        flush_autosaves: Some(Box::new(move || with_store(&s5, RecoveryStore::flush).unwrap_or_default())),
        discard_autosave: Some(Box::new(move |id: u64| {
            let _ = with_store(&s2, |store| store.discard(id));
        })),
        recover: Some(Box::new(move || {
            let Some(dir) = recover_dir.as_ref() else { return Vec::new() };
            photocraft_format::list_recovery(dir)
                .into_iter()
                .map(|entry| Recoverable {
                    key: entry.info.key.clone(),
                    name: entry.info.document_name.clone(),
                    path: entry.info.original_path.clone(),
                    load: Box::new(move || photocraft_format::recover(&entry).map_err(|error| error.to_string())),
                })
                .collect()
        })),
        adopt_autosave: Some(Box::new(move |id: u64, key: &str| {
            let _ = with_store(&store, |store| store.adopt(id, key));
        })),
        ..Default::default()
    }
}

pub fn services() -> Services {
    Services {
        import: Some(Box::new(import)),
        export: Some(Box::new(export)),
        save_file: Some(Arc::new(save_file)),
        file_dialog: Some(Box::new(show_file_dialog)),
        write: Some(Box::new(write)),
        // Two URIs name the same document only when they map to the same granted URI.
        same_file: Some(Box::new(|a, b| base_uri(a) == base_uri(b))),
        encode_png: Some(Box::new(encode_png)),
        open_url: Some(Box::new(|url| bridge::open_url(url))),
        os_events: Some(Box::new(poll)),
        load_prefs: Some(Box::new(|| std::fs::read_to_string(prefs_file()?).ok())),
        save_prefs: Some(Box::new(save_prefs)),
        // Not wired on Android yet: the image clipboard, folder pickers, display profiles,
        // printing, update checks, the control server and flushing autosaves on `onPause`.
        ..recovery_services(config_dir().map(|dir| dir.join("Recovery")))
    }
}
