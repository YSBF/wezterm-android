package org.wezfurlong.wezterm

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import androidx.core.view.WindowCompat
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

        // Draw behind the system bars; the Rust side accounts for insets when
        // laying out the terminal grid.
        WindowCompat.setDecorFitsSystemWindows(window, false)

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

    private fun startMuxService() {
        val intent = Intent(this, MuxService::class.java)
        ContextCompat.startForegroundService(this, intent)
    }
}
