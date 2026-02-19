package com.mesh.app

import android.annotation.SuppressLint
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothServerSocket
import android.bluetooth.BluetoothSocket
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.util.Log
import org.json.JSONObject
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.PrintWriter
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Bluetooth RFCOMM transport for mesh messaging.
 * Runs as a parallel Kotlin-layer transport alongside the Rust TCP mesh.
 * Uses a simple JSON-over-RFCOMM protocol (one JSON object per line).
 */
@SuppressLint("MissingPermission")
class BluetoothTransport(
    private val context: Context,
    private val localName: String
) {

    companion object {
        private const val TAG = "BluetoothTransport"
        private const val SERVICE_NAME = "MassKriticalMesh"
        val SERVICE_UUID: UUID = UUID.fromString("a1b2c3d4-e5f6-7890-abcd-ef1234567890")
        private const val DISCOVERY_INTERVAL_MS = 30_000L
    }

    interface Listener {
        fun onBtMessageReceived(type: String, sender: String, content: String, json: JSONObject)
        fun onBtPeerConnected(address: String, name: String)
        fun onBtPeerDisconnected(address: String, name: String)
        fun onBtStatusChanged(status: String)
    }

    data class ConnectedPeer(
        val address: String,
        val name: String,
        val socket: BluetoothSocket,
        val reader: BufferedReader,
        val writer: PrintWriter,
        val readThread: Thread
    )

    private val adapter: BluetoothAdapter? by lazy {
        val manager = context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
        manager?.adapter
    }

    private val peers = ConcurrentHashMap<String, ConnectedPeer>()
    private val threadPool = Executors.newCachedThreadPool()
    private val running = AtomicBoolean(false)
    private var serverSocket: BluetoothServerSocket? = null
    private var acceptThread: Thread? = null
    private var discoveryThread: Thread? = null
    private var listener: Listener? = null

    // Track addresses we're currently connecting to, to avoid duplicate attempts
    private val connectingAddresses = ConcurrentHashMap.newKeySet<String>()

    // BroadcastReceiver for discovery
    private val discoveryReceiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context?, intent: Intent?) {
            when (intent?.action) {
                BluetoothDevice.ACTION_FOUND -> {
                    val device = intent.getParcelableExtra<BluetoothDevice>(BluetoothDevice.EXTRA_DEVICE)
                    device?.let { tryConnect(it) }
                }
                BluetoothAdapter.ACTION_DISCOVERY_FINISHED -> {
                    // Restart discovery periodically if still running
                    if (running.get()) {
                        scheduleDiscovery()
                    }
                }
            }
        }
    }

    fun setListener(l: Listener) {
        listener = l
    }

    fun getPeerCount(): Int = peers.size

    fun getPeerNames(): List<String> = peers.values.map { it.name }

    /**
     * Start the Bluetooth transport: RFCOMM server + discovery of nearby devices.
     */
    fun start() {
        val bt = adapter
        if (bt == null || !bt.isEnabled) {
            listener?.onBtStatusChanged("Bluetooth not available")
            return
        }

        if (running.getAndSet(true)) return // already running

        listener?.onBtStatusChanged("BT starting...")

        // Start RFCOMM server
        startServer()

        // Try bonded devices first (faster than discovery)
        connectBondedDevices()

        // Register discovery receiver and start discovery
        val filter = IntentFilter().apply {
            addAction(BluetoothDevice.ACTION_FOUND)
            addAction(BluetoothAdapter.ACTION_DISCOVERY_FINISHED)
        }
        context.registerReceiver(discoveryReceiver, filter)
        startDiscovery()

        listener?.onBtStatusChanged("BT active")
    }

    /**
     * Stop the Bluetooth transport: close all connections and stop server.
     */
    fun stop() {
        if (!running.getAndSet(false)) return

        try { context.unregisterReceiver(discoveryReceiver) } catch (_: Exception) {}

        adapter?.cancelDiscovery()

        // Close server
        try { serverSocket?.close() } catch (_: Exception) {}
        serverSocket = null

        // Close all peer connections
        val peersCopy = ArrayList(peers.values)
        peers.clear()
        connectingAddresses.clear()
        for (peer in peersCopy) {
            closePeer(peer)
        }

        listener?.onBtStatusChanged("BT stopped")
    }

    /**
     * Send a text message to all connected BT peers.
     */
    fun sendText(sender: String, content: String) {
        val json = JSONObject().apply {
            put("type", "text")
            put("sender", sender)
            put("content", content)
        }
        broadcastJson(json, excludeAddress = null)
    }

    /**
     * Send an SOS to all connected BT peers.
     */
    fun sendSOS(sender: String, content: String, lat: Double, lon: Double) {
        val json = JSONObject().apply {
            put("type", "sos")
            put("sender", sender)
            put("content", content)
            put("lat", lat)
            put("lon", lon)
        }
        broadcastJson(json, excludeAddress = null)
    }

    /**
     * Send a check-in to all connected BT peers.
     */
    fun sendCheckIn(sender: String, status: String, content: String) {
        val json = JSONObject().apply {
            put("type", "checkin")
            put("sender", sender)
            put("status", status)
            put("content", content)
        }
        broadcastJson(json, excludeAddress = null)
    }

    /**
     * Send a broadcast message to all connected BT peers.
     */
    fun sendBroadcast(sender: String, content: String) {
        val json = JSONObject().apply {
            put("type", "broadcast")
            put("sender", sender)
            put("content", content)
        }
        broadcastJson(json, excludeAddress = null)
    }

    // --- Internal ---

    private fun startServer() {
        try {
            serverSocket = adapter?.listenUsingRfcommWithServiceRecord(SERVICE_NAME, SERVICE_UUID)
        } catch (e: Exception) {
            Log.e(TAG, "Failed to create RFCOMM server", e)
            listener?.onBtStatusChanged("BT server failed")
            return
        }

        acceptThread = Thread {
            while (running.get()) {
                try {
                    val socket = serverSocket?.accept() ?: break
                    handleIncomingConnection(socket)
                } catch (e: Exception) {
                    if (running.get()) {
                        Log.e(TAG, "Accept error", e)
                    }
                    break
                }
            }
        }.apply {
            name = "BT-Accept"
            isDaemon = true
            start()
        }
    }

    private fun handleIncomingConnection(socket: BluetoothSocket) {
        val device = socket.remoteDevice
        val address = device.address
        val name = device.name ?: address

        if (peers.containsKey(address)) {
            // Already connected, close duplicate
            try { socket.close() } catch (_: Exception) {}
            return
        }

        registerPeer(address, name, socket)
    }

    private fun connectBondedDevices() {
        val bonded = adapter?.bondedDevices ?: return
        for (device in bonded) {
            tryConnect(device)
        }
    }

    private fun tryConnect(device: BluetoothDevice) {
        val address = device.address
        // Skip if already connected or already trying
        if (peers.containsKey(address) || !connectingAddresses.add(address)) return

        threadPool.execute {
            try {
                adapter?.cancelDiscovery() // Must cancel before connect
                val socket = device.createRfcommSocketToServiceRecord(SERVICE_UUID)
                socket.connect()
                val name = device.name ?: address
                registerPeer(address, name, socket)
            } catch (_: Exception) {
                // Device doesn't have our service or is out of range - this is normal
            } finally {
                connectingAddresses.remove(address)
            }
        }
    }

    private fun registerPeer(address: String, name: String, socket: BluetoothSocket) {
        try {
            val reader = BufferedReader(InputStreamReader(socket.inputStream, Charsets.UTF_8))
            val writer = PrintWriter(socket.outputStream.bufferedWriter(Charsets.UTF_8), true)

            // Send identity
            val identityJson = JSONObject().apply {
                put("type", "identity")
                put("sender", localName)
            }
            writer.println(identityJson.toString())

            val readThread = Thread {
                readLoop(address, reader)
            }.apply {
                this.name = "BT-Read-$address"
                isDaemon = true
            }

            val peer = ConnectedPeer(address, name, socket, reader, writer, readThread)
            peers[address] = peer
            readThread.start()

            Log.i(TAG, "BT peer connected: $name ($address)")
            listener?.onBtPeerConnected(address, name)

        } catch (e: Exception) {
            Log.e(TAG, "Failed to register peer $address", e)
            try { socket.close() } catch (_: Exception) {}
        }
    }

    private fun readLoop(address: String, reader: BufferedReader) {
        try {
            while (running.get()) {
                val line = reader.readLine() ?: break
                if (line.isBlank()) continue
                try {
                    val json = JSONObject(line)
                    handleMessage(address, json)
                } catch (e: Exception) {
                    Log.w(TAG, "Invalid JSON from $address: $line")
                }
            }
        } catch (_: Exception) {
            // Connection lost
        } finally {
            removePeer(address)
        }
    }

    private fun handleMessage(senderAddress: String, json: JSONObject) {
        val type = json.optString("type", "text")
        val sender = json.optString("sender", "?")
        val content = json.optString("content", "")

        // Update peer name if we got an identity message
        if (type == "identity") {
            peers[senderAddress]?.let { existing ->
                peers[senderAddress] = existing.copy(name = sender)
            }
            listener?.onBtPeerConnected(senderAddress, sender)
            return
        }

        // Deliver to listener
        listener?.onBtMessageReceived(type, sender, content, json)

        // Relay broadcast/SOS/checkin to other BT peers (excluding sender)
        if (type in listOf("broadcast", "sos", "checkin")) {
            broadcastJson(json, excludeAddress = senderAddress)
        }
    }

    private fun broadcastJson(json: JSONObject, excludeAddress: String?) {
        val line = json.toString()
        for ((address, peer) in peers) {
            if (address == excludeAddress) continue
            threadPool.execute {
                try {
                    peer.writer.println(line)
                } catch (_: Exception) {
                    removePeer(address)
                }
            }
        }
    }

    private fun removePeer(address: String) {
        val peer = peers.remove(address) ?: return
        closePeer(peer)
        Log.i(TAG, "BT peer disconnected: ${peer.name} ($address)")
        listener?.onBtPeerDisconnected(address, peer.name)
    }

    private fun closePeer(peer: ConnectedPeer) {
        try { peer.reader.close() } catch (_: Exception) {}
        try { peer.writer.close() } catch (_: Exception) {}
        try { peer.socket.close() } catch (_: Exception) {}
    }

    private fun startDiscovery() {
        try {
            adapter?.startDiscovery()
        } catch (e: Exception) {
            Log.w(TAG, "Discovery start failed", e)
        }
    }

    private fun scheduleDiscovery() {
        discoveryThread = Thread {
            try {
                Thread.sleep(DISCOVERY_INTERVAL_MS)
                if (running.get()) {
                    connectBondedDevices()
                    startDiscovery()
                }
            } catch (_: InterruptedException) {}
        }.apply {
            name = "BT-Discovery"
            isDaemon = true
            start()
        }
    }
}
