package com.mesh.app

import android.app.*
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.localbroadcastmanager.content.LocalBroadcastManager
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Foreground service that keeps the mesh node running in the background.
 * Uses LocalBroadcastManager (fix for Android 14+ RECEIVER_EXPORTED issue).
 * Buffers last state so new activities can query immediately.
 */
class MeshService : Service() {

    companion object {
        const val TAG = "MeshService"
        const val CHANNEL_ID = "mesh_service_channel"
        const val NOTIFICATION_ID = 1
        const val ACTION_STOP = "com.mesh.app.STOP"
        const val BROADCAST_ACTION = "com.mesh.app.MESH_EVENT"
    }

    private val pollExecutor = Executors.newSingleThreadScheduledExecutor()
    private var isRunning = false
    private var peerCount = 0
    private var nodeId: String? = null
    private var lastStatus = "stopped"

    // Buffer recent events for late-binding activities
    private val recentEvents = CopyOnWriteArrayList<MeshBridge.MeshEvent>()

    // Binder for direct service communication
    private val binder = MeshBinder()

    inner class MeshBinder : Binder() {
        fun getService(): MeshService = this@MeshService
    }

    fun isNodeRunning() = isRunning
    fun getNodeId() = nodeId
    fun getPeerCount() = peerCount
    fun getLastStatus() = lastStatus
    fun getRecentEvents(): List<MeshBridge.MeshEvent> = recentEvents.toList()

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopMeshNode()
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
            lastStatus = "running"
            Log.i(TAG, "Mesh node started successfully")
            updateNotification("Mesh node running")
            startEventPolling()
        } else {
            Log.e(TAG, "Failed to start mesh node")
            lastStatus = "failed"
            updateNotification("Mesh node failed to start")
            stopSelf()
        }
    }

    private fun stopMeshNode() {
        if (isRunning) {
            MeshBridge.meshStop()
            isRunning = false
            lastStatus = "stopped"
        }
    }

    private fun startEventPolling() {
        pollExecutor.scheduleWithFixedDelay({
            try {
                var event = MeshBridge.meshPollEvent()
                while (event != null && event.eventType != 0) {
                    handleEvent(event)
                    event = MeshBridge.meshPollEvent()
                }
            } catch (e: Exception) {
                Log.e(TAG, "Event polling error", e)
            }
        }, 100, 100, TimeUnit.MILLISECONDS)
    }

    private fun handleEvent(event: MeshBridge.MeshEvent) {
        // Buffer event for late-binding activities
        recentEvents.add(event)
        if (recentEvents.size > 100) {
            recentEvents.removeAt(0)
        }

        when (event.eventType) {
            1 -> { // Peer connected
                peerCount++
                Log.i(TAG, "Peer connected: ${event.data} (${event.nodeId?.take(8)})")
                updateNotification("Peers: $peerCount")
            }
            2 -> { // Peer disconnected
                peerCount = maxOf(0, peerCount - 1)
                Log.i(TAG, "Peer disconnected: ${event.nodeId?.take(8)}")
                updateNotification("Peers: $peerCount")
            }
            3 -> Log.i(TAG, "Message from ${event.senderName}: ${event.data}")
            4 -> { // Started
                nodeId = event.nodeId
                Log.i(TAG, "Node started: ${event.nodeId}")
            }
            5 -> Log.i(TAG, "File offered: ${event.data} from ${event.senderName}")
            8 -> Log.i(TAG, "Voice note from ${event.senderName}")
            12 -> { // SOS
                Log.w(TAG, "SOS from ${event.senderName}: ${event.data}")
                showSOSNotification(event)
            }
            19 -> { // Nuked
                isRunning = false
                lastStatus = "nuked"
                stopSelf()
            }
            20 -> { // Stopped
                isRunning = false
                lastStatus = "stopped"
            }
        }

        // Broadcast to activity via LocalBroadcastManager
        sendLocalEvent(event)
    }

    private fun sendLocalEvent(event: MeshBridge.MeshEvent) {
        val intent = Intent(BROADCAST_ACTION).apply {
            putExtra("event_type", event.eventType)
            putExtra("node_id", event.nodeId)
            putExtra("data", event.data)
            putExtra("sender_name", event.senderName)
            putExtra("extra", event.extra)
            putExtra("value", event.value)
            putExtra("float1", event.float1)
            putExtra("float2", event.float2)
            event.binaryData?.let { putExtra("binary_data", it) }
        }
        LocalBroadcastManager.getInstance(this).sendBroadcast(intent)
    }

    private fun showSOSNotification(event: MeshBridge.MeshEvent) {
        val notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("SOS Alert!")
            .setContentText("${event.senderName}: ${event.data}")
            .setSmallIcon(android.R.drawable.ic_dialog_alert)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setAutoCancel(true)
            .build()
        val manager = getSystemService(NotificationManager::class.java)
        manager.notify(NOTIFICATION_ID + 1, notification)
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

        val openIntent = Intent(this, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP
        }
        val openPendingIntent = PendingIntent.getActivity(
            this, 0, openIntent, PendingIntent.FLAG_IMMUTABLE
        )

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Mesh Network")
            .setContentText(text)
            .setSmallIcon(android.R.drawable.ic_menu_share)
            .setContentIntent(openPendingIntent)
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Stop", stopPendingIntent)
            .setOngoing(true)
            .build()
    }

    private fun updateNotification(text: String) {
        val notification = buildNotification(text)
        val manager = getSystemService(NotificationManager::class.java)
        manager.notify(NOTIFICATION_ID, notification)
    }

    override fun onDestroy() {
        stopMeshNode()
        pollExecutor.shutdown()
        // Notify activities the service stopped
        val intent = Intent(BROADCAST_ACTION).apply {
            putExtra("event_type", 20)
        }
        LocalBroadcastManager.getInstance(this).sendBroadcast(intent)
        super.onDestroy()
    }
}
