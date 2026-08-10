package org.wezfurlong.wezterm

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat

/**
 * Keeps the terminal's processes alive while the app is not in the foreground.
 *
 * Surface recreation does not save you from process death: Android will kill a
 * backgrounded app and take the mux and every child shell with it, so
 * "check your email, come back, your build is still running" does not work by
 * default no matter how well the surface lifecycle is handled.
 *
 * A foreground service with an ongoing notification is what buys the process
 * the right to keep running. The partial wake lock is a separate concern: it
 * prevents the CPU being suspended while the screen is off, which is what would
 * otherwise stall a long-running command rather than kill it.
 *
 * The service does not host the mux itself -- that lives in the Activity's
 * process, which is the same process -- it exists to change how the system
 * treats that process.
 */
class MuxService : Service() {

    companion object {
        private const val CHANNEL_ID = "wezterm-mux"
        private const val NOTIFICATION_ID = 1

        /** Sent by the Activity to update what the notification says. */
        const val ACTION_UPDATE = "org.wezfurlong.wezterm.UPDATE"
        const val EXTRA_SUMMARY = "summary"

        /** Sent from the notification's action button. */
        const val ACTION_STOP = "org.wezfurlong.wezterm.STOP"
    }

    private var wakeLock: PowerManager.WakeLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()

        val powerManager = getSystemService(Context.POWER_SERVICE) as PowerManager
        wakeLock = powerManager.newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK,
            "wezterm:mux",
        ).apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopSelf()
            return START_NOT_STICKY
        }

        val summary = intent?.getStringExtra(EXTRA_SUMMARY)
            ?: getString(R.string.mux_running)

        startForeground(NOTIFICATION_ID, buildNotification(summary))

        // Deliberately not START_STICKY: if the system kills us, the shells are
        // gone anyway, so restarting an empty service would only produce a
        // misleading notification.
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        // Ordinarily the notification goes when the service does, but say so
        // explicitly: this runs on the way out of the app, and an ongoing
        // notification for a terminal that has exited is the one thing the
        // user cannot dismiss by hand.
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    private fun buildNotification(summary: String): Notification {
        val openActivity = PendingIntent.getActivity(
            this,
            0,
            Intent(this, WezTermActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            },
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, MuxService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(summary)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentIntent(openActivity)
            .addAction(0, getString(R.string.mux_stop), stop)
            .setOngoing(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return
        }
        val channel = NotificationChannel(
            CHANNEL_ID,
            getString(R.string.mux_channel_name),
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = getString(R.string.mux_channel_description)
            setShowBadge(false)
        }
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(channel)
    }
}
