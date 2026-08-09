//! Clipboard access via `android.content.ClipboardManager`.
//!
//! The NDK exposes no clipboard API at all, so this is the one part of the
//! backend that has to go through JNI. Both directions are marshalled onto the
//! Java main thread: `getSystemService` historically required a thread with a
//! Java `Looper`, and `android_main` runs on a native thread that has an
//! `ALooper` but no Java one.
//!
//! wezterm distinguishes `Clipboard` from `PrimarySelection`; Android has only
//! one clipboard, so both alias it. Aliasing rather than erroring on primary
//! selection means middle-click paste and the usual X11-flavoured key
//! assignments still do something sensible instead of failing.

use super::app;
use jni::objects::{JObject, JString};
use jni::refs::Global;
use jni::{jni_sig, jni_str, JValue, JavaVM};
use promise::{Future, Promise};

/// The label attached to clips we place on the clipboard. Android 13+ shows
/// this in the copy confirmation toast.
const CLIP_LABEL: &str = "wezterm";

pub fn set_clipboard(text: String) {
    let app = match app::try_android_app() {
        Some(app) => app.clone(),
        None => {
            log::error!("set_clipboard: no AndroidApp");
            return;
        }
    };

    app.clone().run_on_java_main_thread(Box::new(move || {
        if let Err(err) = set_clipboard_impl(&app, &text) {
            log::error!("failed to set the clipboard: {err:#}");
        }
    }));
}

pub fn get_clipboard() -> Future<String> {
    let mut promise = Promise::new();
    let future = promise.get_future().expect("new promise has a future");

    let app = match app::try_android_app() {
        Some(app) => app.clone(),
        None => {
            promise.err(anyhow::anyhow!("get_clipboard: no AndroidApp"));
            return future;
        }
    };

    app.clone().run_on_java_main_thread(Box::new(move || {
        let mut promise = promise;
        match get_clipboard_impl(&app) {
            Ok(text) => {
                promise.ok(text);
            }
            Err(err) => {
                promise.err(err);
            }
        }
    }));

    future
}

/// Obtain the `ClipboardManager` for the activity.
///
/// Safety: `app.activity_as_ptr()` returns a JNI global reference that we do
/// not own, so it is cast rather than wrapped in an owning handle.
fn with_clipboard_manager<T, F>(app: &android_activity::AndroidApp, f: F) -> anyhow::Result<T>
where
    F: FnOnce(&mut jni::Env, &JObject) -> anyhow::Result<T>,
{
    // Safety: android-activity guarantees this pointer is the process JavaVM.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };

    vm.attach_current_thread(|env| -> anyhow::Result<T> {
        let activity_raw = app.activity_as_ptr() as jni::sys::jobject;
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_raw) }
            .map_err(|err| anyhow::anyhow!("casting the activity reference: {err}"))?;

        let service_name = env.new_string("clipboard")?;
        let manager = env
            .call_method(
                activity.as_ref(),
                jni_str!("getSystemService"),
                jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
                &[JValue::Object(&service_name)],
            )?
            .l()?;

        if manager.is_null() {
            anyhow::bail!("getSystemService(clipboard) returned null");
        }

        f(env, &manager)
    })
}

fn set_clipboard_impl(app: &android_activity::AndroidApp, text: &str) -> anyhow::Result<()> {
    with_clipboard_manager(app, |env, manager| {
        let label = env.new_string(CLIP_LABEL)?;
        let content = env.new_string(text)?;

        let clip_data_class = env.find_class(jni_str!("android/content/ClipData"))?;
        let clip = env
            .call_static_method(
                &clip_data_class,
                jni_str!("newPlainText"),
                jni_sig!(
                    "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;"
                ),
                &[JValue::Object(&label), JValue::Object(&content)],
            )?
            .l()?;

        env.call_method(
            manager,
            jni_str!("setPrimaryClip"),
            jni_sig!("(Landroid/content/ClipData;)V"),
            &[JValue::Object(&clip)],
        )?;

        Ok(())
    })
}

fn get_clipboard_impl(app: &android_activity::AndroidApp) -> anyhow::Result<String> {
    with_clipboard_manager(app, |env, manager| {
        let clip = env
            .call_method(
                manager,
                jni_str!("getPrimaryClip"),
                jni_sig!("()Landroid/content/ClipData;"),
                &[],
            )?
            .l()?;

        // Null is the normal result when the clipboard is empty, or when the
        // app does not have focus: since Android 10 a background app is not
        // permitted to read the clipboard at all.
        if clip.is_null() {
            return Ok(String::new());
        }

        let count = env
            .call_method(&clip, jni_str!("getItemCount"), jni_sig!("()I"), &[])?
            .i()?;
        if count <= 0 {
            return Ok(String::new());
        }

        let item = env
            .call_method(
                &clip,
                jni_str!("getItemAt"),
                jni_sig!("(I)Landroid/content/ClipData$Item;"),
                &[JValue::Int(0)],
            )?
            .l()?;

        // coerceToText rather than getText: it renders URIs and intents as
        // text too, which is what a paste into a terminal should produce.
        let activity_raw = app.activity_as_ptr() as jni::sys::jobject;
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_raw) }
            .map_err(|err| anyhow::anyhow!("casting the activity reference: {err}"))?;

        let text = env
            .call_method(
                &item,
                jni_str!("coerceToText"),
                jni_sig!("(Landroid/content/Context;)Ljava/lang/CharSequence;"),
                &[JValue::Object(activity.as_ref())],
            )?
            .l()?;

        if text.is_null() {
            return Ok(String::new());
        }

        let text = env
            .call_method(
                &text,
                jni_str!("toString"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;

        let text: JString = env.cast_local::<JString>(text)?;
        Ok(text.try_to_string(env)?)
    })
}
