//! Native dialogs, over JNI.
//!
//! The terminal, tab bar, key row and sidebar are all Rust-rendered inside the
//! GPU surface, which keeps one coordinate system for touch, resizing and
//! drawing. Text entry is the exception: a host editor needs form fields with
//! working IME editing, selection and a cursor, and a password field needs
//! masking and to be kept away from autofill and the IME's learned
//! vocabulary. Reimplementing all of that in the renderer to avoid a JNI call
//! would be a poor trade.
//!
//! So the Activity presents small dialogs and hands the answer back. An
//! `AlertDialog` is a window of its own above the surface, which is why this is
//! not the same proposition as wrapping the `GameActivity` surface in a
//! `DrawerLayout`: there is no surface z-order, gesture dispatch or
//! terminal-size synchronization to get wrong.
//!
//! The contract is deliberately two calls wide. Rust sends a JSON description
//! and a request id; the Activity answers with the same id and either a JSON
//! payload or a cancellation. Nothing in this module interprets either
//! document -- the schema belongs to the GUI, which is what builds and reads it.
//!
//! Request ids exist because the Activity can be destroyed and recreated at any
//! time. Ids are minted from a process-wide counter and removed from the pending
//! table when answered, so a duplicate or late callback finds nothing and is
//! dropped rather than resolving a request the user has since made. And because
//! a recreated Activity will never answer the dialogs its predecessor was
//! showing, registering a new `AndroidApp` fails every request still pending,
//! rather than leaving the caller waiting for a dialog that is gone with the
//! window it was in.

use super::app;
use crate::dialog::DialogOutcome;
use jni::objects::{JObject, JString};
use jni::refs::Global;
use jni::{jni_sig, jni_str, EnvUnowned, JValue, JavaVM};
use promise::{Future, Promise};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

static NEXT_REQUEST_ID: AtomicI64 = AtomicI64::new(1);
static PENDING: Mutex<Option<HashMap<i64, Promise<DialogOutcome>>>> = Mutex::new(None);

fn pending() -> std::sync::MutexGuard<'static, Option<HashMap<i64, Promise<DialogOutcome>>>> {
    PENDING.lock().unwrap_or_else(|err| err.into_inner())
}

/// Ask the Activity to present a dialog described by `spec`.
///
/// `spec` is passed through untouched; see the module comment on who owns its
/// schema.
pub fn request_dialog(spec: String) -> Future<DialogOutcome> {
    let mut promise = Promise::new();
    let future = promise.get_future().expect("new promise has a future");

    let app = match app::try_android_app() {
        Some(app) => app,
        None => {
            promise.err(anyhow::anyhow!("request_dialog: no AndroidApp"));
            return future;
        }
    };

    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    pending()
        .get_or_insert_with(HashMap::new)
        .insert(request_id, promise);

    app.clone().run_on_java_main_thread(Box::new(move || {
        if let Err(err) = show_dialog(&app, request_id, &spec) {
            // The Activity will never call back for a request it was never
            // told about, so fail it here rather than leaving the caller
            // waiting for a dialog that was never shown.
            log::error!("failed to show a native dialog: {err:#}");
            if let Some(mut promise) = take_pending(request_id) {
                promise.err(err);
            }
        }
    }));

    future
}

/// Fail every request still waiting for an answer.
///
/// Called when the `AndroidApp` is replaced: the dialogs a destroyed Activity
/// was showing went with its window, and no callback is coming for them.
pub fn cancel_pending(reason: &str) {
    let Some(map) = pending().take() else {
        return;
    };
    if map.is_empty() {
        return;
    }
    log::info!("failing {} pending dialog(s): {reason}", map.len());
    for (_, mut promise) in map {
        promise.err(anyhow::anyhow!("{reason}"));
    }
}

fn take_pending(request_id: i64) -> Option<Promise<DialogOutcome>> {
    pending().as_mut().and_then(|map| map.remove(&request_id))
}

fn show_dialog(
    app: &android_activity::AndroidApp,
    request_id: i64,
    spec: &str,
) -> anyhow::Result<()> {
    // Safety: android-activity guarantees this pointer is the process JavaVM.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };

    vm.attach_current_thread(|env| -> anyhow::Result<()> {
        // Safety: `activity_as_ptr` returns a JNI global reference we do not
        // own, so it is cast rather than wrapped in an owning handle.
        let activity_raw = app.activity_as_ptr() as jni::sys::jobject;
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_raw) }
            .map_err(|err| anyhow::anyhow!("casting the activity reference: {err}"))?;

        let spec = env.new_string(spec)?;
        env.call_method(
            activity.as_ref(),
            jni_str!("showNativeDialog"),
            jni_sig!("(JLjava/lang/String;)V"),
            &[JValue::Long(request_id), JValue::Object(&spec)],
        )?;
        Ok(())
    })
}

/// Called from `WezTermActivity` when a dialog is answered or dismissed.
///
/// `cancelled` rather than a nullable payload: a boolean cannot be confused
/// with an empty answer, and it keeps null handling out of the FFI boundary.
#[no_mangle]
pub extern "system" fn Java_org_wezfurlong_wezterm_WezTermActivity_nativeDialogResult<'caller>(
    mut unowned_env: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    request_id: jni::sys::jlong,
    cancelled: jni::sys::jboolean,
    payload: JString<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let outcome = if cancelled {
            DialogOutcome::Cancelled
        } else {
            DialogOutcome::Submitted(payload.try_to_string(env)?)
        };

        match take_pending(request_id) {
            Some(mut promise) => {
                promise.ok(outcome);
            }
            None => {
                // A duplicate, or a callback from an Activity that has since
                // been replaced. Dropping it is the point of the id: without
                // one it would answer whichever request happened to be in
                // flight.
                log::debug!("dropping a dialog result for unknown request {request_id}");
            }
        }
        Ok(())
    });
    outcome.resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// Hand text to whatever the user shares things with.
///
/// A share intent needs no storage permission and no file picker, which matters
/// because the app can reach neither: there is no path a user could pick that the
/// app may write, and asking for one would mean a permission for a feature that
/// exports a list of host names.
pub fn share_text(subject: &str, text: &str) {
    let app = match app::try_android_app() {
        Some(app) => app,
        None => {
            log::error!("share_text: no AndroidApp");
            return;
        }
    };

    let subject = subject.to_string();
    let text = text.to_string();
    app.clone().run_on_java_main_thread(Box::new(move || {
        if let Err(err) = share_text_impl(&app, &subject, &text) {
            log::error!("failed to share text: {err:#}");
        }
    }));
}

fn share_text_impl(
    app: &android_activity::AndroidApp,
    subject: &str,
    text: &str,
) -> anyhow::Result<()> {
    // Safety: android-activity guarantees this pointer is the process JavaVM.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };

    vm.attach_current_thread(|env| -> anyhow::Result<()> {
        let activity_raw = app.activity_as_ptr() as jni::sys::jobject;
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_raw) }
            .map_err(|err| anyhow::anyhow!("casting the activity reference: {err}"))?;

        let subject = env.new_string(subject)?;
        let text = env.new_string(text)?;
        env.call_method(
            activity.as_ref(),
            jni_str!("shareText"),
            jni_sig!("(Ljava/lang/String;Ljava/lang/String;)V"),
            &[JValue::Object(&subject), JValue::Object(&text)],
        )?;
        Ok(())
    })
}
