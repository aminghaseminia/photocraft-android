#![allow(unsafe_code)]
//! JNI calls into `MainActivity` (`packaging/android/app/src/main/java/.../MainActivity.java`).
//!
//! Every platform operation goes through an existing method on the activity instance, which
//! `android-activity` publishes through `ndk-context`. That avoids `FindClass` on a native thread
//! (which uses the bootstrap class loader and cannot see the app's classes) and avoids caching
//! class or object references of our own.
//!
//! The picker results are *polled*, not pushed back through a native callback: the Java side
//! stores the result and [`take_result`] drains it once per frame from `Services::os_events`.
//! That keeps this crate free of exported JNI functions and their name mangling.

use jni::objects::{JByteArray, JObject, JString};
use jni::{Env, JValue, JavaVM, jni_sig, jni_str};

/// Makes the process's `JavaVM` available to the `jni` crate. `android-activity` initialized
/// `ndk-context` before `android_main`, so both pointers are valid for the app's lifetime.
pub fn init() -> Result<(), String> {
    let vm = ndk_context::android_context().vm();
    // SAFETY: JNI has exactly one VM per process and `ndk-context` holds it; `from_raw` only
    // registers it as the crate's singleton.
    let _ = unsafe { JavaVM::from_raw(vm.cast()) };
    Ok(())
}

/// The activity, for the duration of one JNI call.
///
/// `ndk-context` keeps a global reference to it until the activity is destroyed, which happens
/// after `android_main` returns.
fn activity<'local>(env: &Env<'local>) -> JObject<'local> {
    let context = ndk_context::android_context();
    // SAFETY: see above; the global reference keeps the object alive.
    unsafe { JObject::from_raw(env, context.context().cast()) }
}

/// The app's private files directory (`Context.getFilesDir()`), where preferences live.
pub fn files_dir() -> Result<String, String> {
    let vm = JavaVM::singleton().map_err(|error| error.to_string())?;
    vm.attach_current_thread(|env| {
        let activity = activity(env);
        let value = env.call_method(&activity, jni_str!("photocraftFilesDir"), jni_sig!("()Ljava/lang/String;"), &[])?;
        let string: JString = env.cast_local::<JString>(value.l()?)?;
        string.try_to_string(env)
    })
    .map_err(|error| error.to_string())
}

/// Shows the Storage Access Framework's open-document picker. The answer arrives later through
/// [`take_result`].
pub fn open_documents(multiple: bool) -> Result<(), String> {
    let vm = JavaVM::singleton().map_err(|error| error.to_string())?;
    vm.attach_current_thread(|env| {
        let activity = activity(env);
        let args = [JValue::Bool(multiple)];
        env.call_method(&activity, jni_str!("photocraftOpenDocuments"), jni_sig!("(Z)V"), &args)?.v()
    })
    .map_err(|error| error.to_string())
}

/// Shows the Storage Access Framework's create-document picker for `name`.
pub fn create_document(name: &str, mime: &str) -> Result<(), String> {
    let vm = JavaVM::singleton().map_err(|error| error.to_string())?;
    vm.attach_current_thread(|env| {
        let activity = activity(env);
        let name = env.new_string(name)?;
        let mime = env.new_string(mime)?;
        let args = [JValue::Object(&name), JValue::Object(&mime)];
        env.call_method(&activity, jni_str!("photocraftCreateDocument"), jni_sig!("(Ljava/lang/String;Ljava/lang/String;)V"), &args)?.v()
    })
    .map_err(|error| error.to_string())
}

/// Drains one pending shell event: `""` (none), `"cancel"`, `"back"`, `"open:<uri>…"` (newline
/// separated) or `"save:<uri>"`.
pub fn take_result() -> Result<String, String> {
    let vm = JavaVM::singleton().map_err(|error| error.to_string())?;
    vm.attach_current_thread(|env| {
        let activity = activity(env);
        let value = env.call_method(&activity, jni_str!("photocraftTakeResult"), jni_sig!("()Ljava/lang/String;"), &[])?;
        let string: JString = env.cast_local::<JString>(value.l()?)?;
        string.try_to_string(env)
    })
    .map_err(|error| error.to_string())
}

/// The display name a content provider reports for `uri` (usually with its extension, which the
/// importer uses to pick a decoder).
pub fn document_name(uri: &str) -> Result<String, String> {
    let vm = JavaVM::singleton().map_err(|error| error.to_string())?;
    vm.attach_current_thread(|env| {
        let activity = activity(env);
        let uri = env.new_string(uri)?;
        let args = [JValue::Object(&uri)];
        let value = env.call_method(&activity, jni_str!("photocraftDocumentName"), jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"), &args)?;
        let string: JString = env.cast_local::<JString>(value.l()?)?;
        string.try_to_string(env)
    })
    .map_err(|error| error.to_string())
}

/// Reads a `content://` URI into memory.
pub fn read_all(uri: &str) -> Result<Vec<u8>, String> {
    let vm = JavaVM::singleton().map_err(|error| error.to_string())?;
    vm.attach_current_thread(|env| {
        let activity = activity(env);
        let uri = env.new_string(uri)?;
        let args = [JValue::Object(&uri)];
        let value = env.call_method(&activity, jni_str!("photocraftReadAll"), jni_sig!("(Ljava/lang/String;)[B"), &args)?;
        let bytes: JByteArray = env.cast_local::<JByteArray>(value.l()?)?;
        env.convert_byte_array(&bytes)
    })
    .map_err(|error| error.to_string())
}

/// Writes `bytes` to a `content://` URI, truncating whatever the provider had there.
pub fn write_all(uri: &str, bytes: &[u8]) -> Result<(), String> {
    let vm = JavaVM::singleton().map_err(|error| error.to_string())?;
    vm.attach_current_thread(|env| {
        let activity = activity(env);
        let uri = env.new_string(uri)?;
        let data = env.byte_array_from_slice(bytes)?;
        let args = [JValue::Object(&uri), JValue::Object(&data)];
        env.call_method(&activity, jni_str!("photocraftWriteAll"), jni_sig!("(Ljava/lang/String;[B)V"), &args)?.v()
    })
    .map_err(|error| error.to_string())
}

/// Opens `url` in the system browser.
pub fn open_url(url: &str) -> Result<(), String> {
    let vm = JavaVM::singleton().map_err(|error| error.to_string())?;
    vm.attach_current_thread(|env| {
        let activity = activity(env);
        let url = env.new_string(url)?;
        let args = [JValue::Object(&url)];
        env.call_method(&activity, jni_str!("photocraftOpenUrl"), jni_sig!("(Ljava/lang/String;)V"), &args)?.v()
    })
    .map_err(|error| error.to_string())
}
