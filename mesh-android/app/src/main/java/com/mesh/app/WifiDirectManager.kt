package com.mesh.app

import android.annotation.SuppressLint
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.wifi.p2p.WifiP2pConfig
import android.net.wifi.p2p.WifiP2pDevice
import android.net.wifi.p2p.WifiP2pInfo
import android.net.wifi.p2p.WifiP2pManager
import android.util.Log

/**
 * WiFi Direct P2P manager.
 * Creates a P2P local network between phones. Once connected, the existing
 * Rust UDP discovery (port 7331) + TCP mesh (port 7332) works over the
 * P2P interface automatically — no special bridging needed.
 */
@SuppressLint("MissingPermission")
class WifiDirectManager(private val context: Context) {

    companion object {
        private const val TAG = "WifiDirectManager"
    }

    interface Listener {
        fun onWifiDirectStatusChanged(enabled: Boolean)
        fun onWifiDirectPeersFound(peers: List<WifiP2pDevice>)
        fun onWifiDirectConnected(info: WifiP2pInfo)
        fun onWifiDirectDisconnected()
        fun onWifiDirectStatus(status: String)
    }

    private var manager: WifiP2pManager? = null
    private var channel: WifiP2pManager.Channel? = null
    private var listener: Listener? = null
    private var isActive = false
    private var isGroupOwner = false
    private var discoveredPeers = listOf<WifiP2pDevice>()

    val receiver: BroadcastReceiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context?, intent: Intent?) {
            when (intent?.action) {
                WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION -> {
                    val state = intent.getIntExtra(
                        WifiP2pManager.EXTRA_WIFI_STATE,
                        WifiP2pManager.WIFI_P2P_STATE_DISABLED
                    )
                    val enabled = state == WifiP2pManager.WIFI_P2P_STATE_ENABLED
                    listener?.onWifiDirectStatusChanged(enabled)
                    if (!enabled) {
                        listener?.onWifiDirectStatus("WiFi Direct: Disabled")
                    }
                }

                WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION -> {
                    requestPeers()
                }

                WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION -> {
                    requestConnectionInfo()
                }

                WifiP2pManager.WIFI_P2P_THIS_DEVICE_CHANGED_ACTION -> {
                    // Our device info changed — could update display name
                }
            }
        }
    }

    fun setListener(l: Listener) {
        listener = l
    }

    fun getIntentFilter(): IntentFilter {
        return IntentFilter().apply {
            addAction(WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_PEERS_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_THIS_DEVICE_CHANGED_ACTION)
        }
    }

    /**
     * Initialize WiFi Direct manager and channel.
     */
    fun init() {
        manager = context.getSystemService(Context.WIFI_P2P_SERVICE) as? WifiP2pManager
        channel = manager?.initialize(context, context.mainLooper, null)
    }

    /**
     * Start WiFi Direct: discover peers and auto-create group if none found.
     */
    fun start() {
        if (isActive) return
        isActive = true

        val mgr = manager
        val ch = channel
        if (mgr == null || ch == null) {
            init()
        }

        discoverPeers()
        listener?.onWifiDirectStatus("WiFi Direct: Discovering...")
    }

    /**
     * Stop WiFi Direct: remove group and stop discovery.
     */
    fun stop() {
        if (!isActive) return
        isActive = false

        val mgr = manager ?: return
        val ch = channel ?: return

        mgr.stopPeerDiscovery(ch, null)
        mgr.removeGroup(ch, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                isGroupOwner = false
                listener?.onWifiDirectStatus("WiFi Direct: Off")
                listener?.onWifiDirectDisconnected()
            }
            override fun onFailure(reason: Int) {
                listener?.onWifiDirectStatus("WiFi Direct: Off")
            }
        })
    }

    /**
     * Discover nearby WiFi Direct peers.
     */
    fun discoverPeers() {
        val mgr = manager ?: return
        val ch = channel ?: return

        mgr.discoverPeers(ch, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                Log.i(TAG, "Peer discovery started")
            }
            override fun onFailure(reason: Int) {
                Log.w(TAG, "Peer discovery failed: reason=$reason")
                // If discovery fails, try creating a group instead
                if (isActive) {
                    createGroup()
                }
            }
        })
    }

    /**
     * Create a WiFi Direct group (become group owner / act as hotspot).
     */
    fun createGroup() {
        val mgr = manager ?: return
        val ch = channel ?: return

        mgr.createGroup(ch, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                isGroupOwner = true
                Log.i(TAG, "Group created — this device is group owner")
                listener?.onWifiDirectStatus("WiFi Direct: Group Owner")
            }
            override fun onFailure(reason: Int) {
                Log.w(TAG, "Create group failed: reason=$reason")
                listener?.onWifiDirectStatus("WiFi Direct: Group failed")
            }
        })
    }

    /**
     * Connect to a discovered WiFi Direct peer.
     */
    fun connectToPeer(device: WifiP2pDevice) {
        val mgr = manager ?: return
        val ch = channel ?: return

        val config = WifiP2pConfig().apply {
            deviceAddress = device.deviceAddress
        }

        mgr.connect(ch, config, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                Log.i(TAG, "Connecting to ${device.deviceName}...")
                listener?.onWifiDirectStatus("WiFi Direct: Connecting...")
            }
            override fun onFailure(reason: Int) {
                Log.w(TAG, "Connect failed: reason=$reason")
                listener?.onWifiDirectStatus("WiFi Direct: Connect failed")
            }
        })
    }

    /**
     * Auto-connect to the first available peer, or create group if none found.
     */
    fun autoConnect() {
        val available = discoveredPeers.filter {
            it.status == WifiP2pDevice.AVAILABLE
        }
        if (available.isNotEmpty()) {
            connectToPeer(available.first())
        } else {
            createGroup()
        }
    }

    fun getStatusText(): String {
        return when {
            !isActive -> "WiFi Direct: Off"
            isGroupOwner -> "WiFi Direct: Group Owner"
            discoveredPeers.isNotEmpty() -> "WiFi Direct: ${discoveredPeers.size} peers"
            else -> "WiFi Direct: Discovering..."
        }
    }

    // --- Internal ---

    private fun requestPeers() {
        val mgr = manager ?: return
        val ch = channel ?: return

        mgr.requestPeers(ch) { peerList ->
            discoveredPeers = peerList.deviceList.toList()
            Log.i(TAG, "Found ${discoveredPeers.size} WiFi Direct peers")
            listener?.onWifiDirectPeersFound(discoveredPeers)

            // Auto-connect if we have available peers and aren't already connected
            if (isActive && discoveredPeers.any { it.status == WifiP2pDevice.AVAILABLE }) {
                autoConnect()
            }
        }
    }

    private fun requestConnectionInfo() {
        val mgr = manager ?: return
        val ch = channel ?: return

        mgr.requestConnectionInfo(ch) { info ->
            if (info?.groupFormed == true) {
                isGroupOwner = info.isGroupOwner
                val role = if (info.isGroupOwner) "Group Owner" else "Connected"
                Log.i(TAG, "WiFi Direct: $role, host=${info.groupOwnerAddress}")
                listener?.onWifiDirectConnected(info)
                listener?.onWifiDirectStatus("WiFi Direct: $role")
            } else {
                if (isActive) {
                    listener?.onWifiDirectDisconnected()
                    listener?.onWifiDirectStatus("WiFi Direct: Disconnected")
                    // Re-discover
                    discoverPeers()
                }
            }
        }
    }
}
