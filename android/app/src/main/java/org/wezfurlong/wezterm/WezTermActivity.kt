package org.wezfurlong.wezterm

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.View
import android.view.WindowManager
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import com.google.androidgamesdk.GameActivity

/**
 * The Activity that hosts wezterm.
 *
 * GameActivity rather than NativeActivity: NativeActivity's soft-keyboard
 * support is too thin for a terminal, while GameActivity ships GameTextInput,
 * which surfaces commit and composing-text events that map onto the paths the
 * Rust side already has for IME handling.
 *
 * Almost nothing happens here. The Activity's job is to load the native
 * library, keep the mux service alive, and get out of the way; the terminal
 * itself is drawn by Rust into the Activity's surface.
 */
class WezTermActivity : GameActivity() {

    companion object {
        init {
            // Matches android.app.lib_name in the manifest.
            System.loadLibrary("wezterm_gui")
        }
    }

    private val requestNotificationPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            // A denied permission is not fatal: the service still runs, the
            // user just does not see its notification. Starting it either way
            // is what keeps background shells alive.
            startMuxService()
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Drawing behind the system bars would put the tab bar under the status
        // clock and the extra-keys row under the gesture bar, and the native
        // side has no way to read the insets for itself. Asking for the decor
        // to fit the system windows is not enough: targeting API 35 opts into
        // Android 15's enforced edge-to-edge, which ignores that request. So
        // the insets are applied here instead, as padding on the content view
        // that holds GameActivity's SurfaceView.
        //
        // The IME is included so that the terminal is resized to sit above the
        // soft keyboard rather than behind it.
        WindowCompat.setDecorFitsSystemWindows(window, false)
        val content = findViewById<View>(android.R.id.content)
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
            val pad = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or
                    WindowInsetsCompat.Type.displayCutout() or
                    WindowInsetsCompat.Type.ime(),
            )
            view.setPadding(pad.left, pad.top, pad.right, pad.bottom)
            // The native side has no way to ask whether the keyboard is up, and
            // it needs to know: the row's keyboard button has to toggle against
            // the real state, not against a count of its own presses.
            nativeSoftKeyboardVisible(insets.isVisible(WindowInsetsCompat.Type.ime()))
            WindowInsetsCompat.CONSUMED
        }

        // A terminal is often watched rather than touched -- a running build,
        // a tailed log -- so do not let the screen blank while it is visible.
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        ensureNotificationPermission()
    }

    private fun ensureNotificationPermission() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            startMuxService()
            return
        }

        val granted = ContextCompat.checkSelfPermission(
            this,
            Manifest.permission.POST_NOTIFICATIONS,
        ) == PackageManager.PERMISSION_GRANTED

        if (granted) {
            startMuxService()
        } else {
            requestNotificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    override fun onDestroy() {
        // The service exists to stop the system reclaiming this process while
        // shells are running. Nothing stops it on its own: START_NOT_STICKY
        // only governs restart after the process dies, so a started foreground
        // service outlives the Activity, holding a wake lock and an ongoing
        // notification for a terminal that is gone. It has, in practice, been
        // taken down with the process; that is the system's choice to make and
        // not something to depend on.
        //
        // Only when the Activity is really going away. onDestroy also runs for
        // a configuration change that we do not handle in place, and there the
        // Activity is about to come straight back.
        if (isFinishing) {
            stopService(Intent(this, MuxService::class.java))
        }
        super.onDestroy()
    }

    /**
     * Present a dialog on behalf of the Rust side.
     *
     * Called over JNI from window/src/os/android/dialog.rs, already marshalled
     * onto the Java main thread. [requestId] identifies the request and is
     * echoed back with the answer: the Activity can be destroyed and recreated
     * at any time, and the id is what stops a late callback from a dialog in a
     * window that no longer exists resolving a request the user has since made.
     *
     * Exactly one [nativeDialogResult] follows each call, whatever happens,
     * because the Rust side is waiting for it.
     */
    fun showNativeDialog(requestId: Long, spec: String) {
        try {
            WezTermDialogs.show(this, spec) { values ->
                if (values == null) {
                    nativeDialogResult(requestId, true, "")
                } else {
                    nativeDialogResult(requestId, false, WezTermDialogs.encode(values))
                }
            }
        } catch (err: Exception) {
            // Anything at all: the caller must not be left waiting. Note that
            // this logs the failure and not the spec, which is fine either way
            // -- a spec carries labels and existing values, never an answer.
            android.util.Log.e("wezterm", "showNativeDialog failed", err)
            nativeDialogResult(requestId, true, "")
        }
    }

    /**
     * Offer text to whatever the user shares things with.
     *
     * Called over JNI from window/src/os/android/dialog.rs, already on the Java
     * main thread. This is how the host list leaves the app at all: the file it
     * is stored in lives in app-private storage, which nothing else can read, and
     * a share intent needs neither a storage permission nor a file picker.
     */
    fun shareText(subject: String, text: String) {
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_SUBJECT, subject)
            putExtra(Intent.EXTRA_TEXT, text)
        }
        try {
            startActivity(Intent.createChooser(send, subject))
        } catch (err: Exception) {
            // No app to share with, or the chooser was refused. The Rust side
            // falls back to the clipboard when this call reports nothing, so
            // there is nothing to do but say why.
            android.util.Log.w("wezterm", "could not start a share chooser", err)
        }
    }

    /** Implemented in window/src/os/android/ime.rs. */
    private external fun nativeSoftKeyboardVisible(visible: Boolean)

    /**
     * Implemented in window/src/os/android/dialog.rs.
     *
     * A boolean rather than a nullable payload, so that a cancellation cannot be
     * confused with an empty answer and null handling stays out of the FFI
     * boundary.
     */
    private external fun nativeDialogResult(requestId: Long, cancelled: Boolean, payload: String)

    private fun startMuxService() {
        val intent = Intent(this, MuxService::class.java)
        ContextCompat.startForegroundService(this, intent)
    }
}
