package com.mesh.app

import android.app.*
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Foreground service that keeps the mesh node running in the background.
 */
class MeshService : Service() {

    companion object {
        const val TAG = "MeshService"
        const val CHANNEL_ID = "mesh_service_channel"
        const val NOTIFICATION_ID = 1
        const val ACTION_STOP = "com.mesh.app.STOP"
    }

    private val pollExecutor = Executors.newSingleThreadScheduledExecutor()
    private var isRunning = false

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopSelf()
            return START_NOT_STICKY
        }

        val name = intent?.getStringExtra("name") ?: "MeshNode"
        val port = intent?.getIntExtra("port", 7332) ?: 7332

        val notification = buildNotification("Mesh node starting...")

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }

        if (!isRunning) {
            startMeshNode(name, port)
        }

        return START_STICKY
    }

    private fun startMeshNode(name: String, port: Int) {
        val dataDir = filesDir.absolutePath
        val result = MeshBridge.meshInit(name, port, dataDir)

        if (result == 0) {
            isRunning = true
            Log.i(TAG, "Mesh node started successfully")
            updateNotification("Mesh node running")
            startEventPolling()
        } else {
            Log.e(TAG, "Failed to start mesh node")
            updateNotification("Mesh node failed to start")
            stopSelf()
        }
    }

    private fun startEventPolling() {
        pollExecutor.scheduleWithFixedDelay({
            try {
                val event = MeshBridge.meshPollEvent()
                if (event != null && event.eventType != 0) {
                    handleEvent(event)
                }
            } catch (e: Exception) {
                Log.e(TAG, "Event polling error", e)
            }
        }, 100, 100, TimeUnit.MILLISECONDS)
    }

    private fun handleEvent(event: MeshBridge.MeshEvent) {
        when (event.eventType) {
            1 -> { // Peer connected
                Log.i(TAG, "Peer connected: ${event.data} (${event.nodeId?.take(8)})")
                updateNotification("Connected to peer: ${event.data}")
                sendEventBroadcast("peer_connected", event)
            }
            2 -> { // Peer disconnected
                Log.i(TAG, "Peer disconnected: ${event.nodeId?.take(8)}")
                sendEventBroadcast("peer_disconnected", event)
            }
            3 -> { // Message received
                Log.i(TAG, "Message from ${event.senderName}: ${event.data}")
                sendEventBroadcast("message_received", event)
            }
            4 -> { // Started
                Log.i(TAG, "Node started: ${event.nodeId}")
                sendEventBroadcast("started", event)
            }
        }
    }

    private fun sendEventBroadcast(action: String, event: MeshBridge.MeshEvent) {
        val intent = Intent("com.mesh.app.MESH_EVENT").apply {
            putExtra("action", action)
            putExtra("node_id", event.nodeId)
            putExtra("data", event.data)
            putExtra("sender_name", event.senderName)
        }
        sendBroadcast(intent)
    }

    private fun createNotificationChannel() {
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Mesh Network Service",
            NotificationManager.IMPORTANCE_LOW
        ).apply {
            description = "Keeps the mesh network node running"
        }
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(channel)
    }

    private fun buildNotification(text: String): Notification {
        val stopIntent = Intent(this, MeshService::class.java).apply {
            action = ACTION_STOP
        }
        val stopPendingIntent = PendingIntent.getService(
            this, 0, stopIntent, PendingIntent.FLAG_IMMUTABLE
        )

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Mesh Network")
            .setContentText(text)
            .setSmallIcon(android.R.drawable.ic_menu_share)
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Stop", stopPendingIntent)
            .setOngoing(true)
            .build()
    }

    private fun updateNotification(text: String) {
        val notification = buildNotification(text)
        val manager = getSystemService(NotificationManager::class.java)
        manager.notify(NOTIFICATION_ID, notification)
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        isRunning = false
        pollExecutor.shutdown()
        super.onDestroy()
    }
}
