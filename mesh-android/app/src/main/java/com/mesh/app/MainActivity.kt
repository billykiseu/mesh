package com.mesh.app

import android.Manifest
import android.content.*
import android.content.pm.PackageManager
import android.graphics.BitmapFactory
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.net.Uri
import android.net.wifi.p2p.WifiP2pDevice
import android.net.wifi.p2p.WifiP2pInfo
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import android.provider.OpenableColumns
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.widget.*
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.localbroadcastmanager.content.LocalBroadcastManager
import org.json.JSONArray
import org.json.JSONObject

class MainActivity : AppCompatActivity(), BluetoothTransport.Listener, WifiDirectManager.Listener {

    // --- Service binding ---
    private var meshService: MeshService? = null
    private var serviceBound = false

    // --- Bluetooth transport ---
    private var bluetoothTransport: BluetoothTransport? = null
    private var btPeerCount = 0

    // --- WiFi Direct ---
    private var wifiDirectManager: WifiDirectManager? = null
    private var wifiDirectStatus = ""

    private val serviceConnection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, service: IBinder?) {
            val binder = service as MeshService.MeshBinder
            meshService = binder.getService()
            serviceBound = true
            restoreStateFromService()
        }
        override fun onServiceDisconnected(name: ComponentName?) {
            serviceBound = false
            meshService = null
        }
    }

    // --- UI elements ---
    private lateinit var headerStatus: TextView
    private lateinit var headerPeers: TextView
    private lateinit var headerGateway: TextView
    private lateinit var headerConnectivity: TextView
    private lateinit var contentFrame: FrameLayout

    // Tab buttons
    private lateinit var tabRadar: TextView
    private lateinit var tabChat: TextView
    private lateinit var tabPeers: TextView
    private lateinit var tabEmergency: TextView
    private lateinit var tabSettings: TextView

    // Chat tab views
    private lateinit var chatListView: ListView
    private lateinit var chatInput: EditText
    private lateinit var chatSendBtn: Button
    private lateinit var chatAttachBtn: Button
    private lateinit var chatMicBtn: Button
    private lateinit var chatCallBtn: Button
    private lateinit var callBannerLayout: LinearLayout
    private lateinit var callBannerText: TextView

    // Peers tab views
    private lateinit var peersListView: ListView

    // Radar tab views
    private lateinit var radarText: TextView
    private lateinit var radarPeerCount: TextView
    private lateinit var radarActionBtn: Button

    // Settings tab views
    private lateinit var settingsLayout: LinearLayout

    // State
    private val chatMessages = mutableListOf<String>()
    private lateinit var chatAdapter: ArrayAdapter<String>
    private val peerEntries = mutableListOf<PeerInfo>()
    private lateinit var peerAdapter: ArrayAdapter<String>
    private val peerDisplayList = mutableListOf<String>()
    private var peerCount = 0
    private var gatewayName: String? = null
    private var currentTab = "chat"
    private var dmTarget: PeerInfo? = null
    private var nodeIdShort: String? = null
    private var activeInterface: String = ""

    // Voice note state
    private var isRecording = false
    private var audioRecord: AudioRecord? = null
    private var recordThread: Thread? = null
    private var recordedAudio: ByteArray? = null

    // Voice call state
    private var inCall: PeerInfo? = null
    private var callAudioRecord: AudioRecord? = null
    private var callAudioTrack: AudioTrack? = null
    private var callRecordThread: Thread? = null
    private var callPlayThread: Thread? = null
    private var callActive = false
    private val callPlaybackBuffer = java.util.concurrent.LinkedBlockingQueue<ByteArray>()

    // Received voice notes for playback
    private val voiceNotes = mutableListOf<VoiceNote>()

    // Group chat state
    private val joinedGroups = mutableListOf<String>()
    private var activeGroup: String? = null
    private val groupMessages = mutableMapOf<String, MutableList<String>>()

    // Typing indicator state
    private val typingPeers = mutableMapOf<String, Long>() // nodeId -> timestamp

    // Emergency state
    private val triageLog = mutableListOf<String>()
    private val resourceLog = mutableListOf<String>()
    private val safetyRoster = mutableMapOf<String, String>() // nodeId -> "name: status"

    // Disappearing messages
    private val disappearingMsgs = mutableListOf<Triple<String, String, Long>>() // msg, text, expiry_time

    data class PeerInfo(val nodeId: String, val displayName: String, var isGateway: Boolean = false, var bio: String = "")
    data class VoiceNote(val sender: String, val audioData: ByteArray, val durationMs: Long)

    // File picker
    private val filePicker = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri?.let { handleFilePicked(it) }
    }

    // --- Broadcast receiver ---
    private val meshEventReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            val eventType = intent?.getIntExtra("event_type", 0) ?: return
            val nodeId = intent.getStringExtra("node_id")
            val data = intent.getStringExtra("data")
            val senderName = intent.getStringExtra("sender_name")
            val extra = intent.getStringExtra("extra")
            val value = intent.getLongExtra("value", 0)

            runOnUiThread {
                when (eventType) {
                    4 -> { // Started
                        nodeIdShort = nodeId?.take(8)
                        updateHeader()
                        updateRadar()
                    }
                    1 -> { // Peer connected
                        peerCount++
                        peerEntries.add(PeerInfo(nodeId ?: "", data ?: "Unknown"))
                        updatePeerList()
                        updateHeader()
                        addChat("[+] ${data ?: "Unknown"} connected")
                    }
                    2 -> { // Peer disconnected
                        peerCount = maxOf(0, peerCount - 1)
                        val disconnectedId = nodeId
                        peerEntries.removeAll { it.nodeId == disconnectedId }
                        updatePeerList()
                        updateHeader()
                        addChat("[-] ${nodeId?.take(8)} disconnected")
                        // End call if this peer disconnected
                        if (inCall?.nodeId == disconnectedId) {
                            endCall()
                        }
                    }
                    3 -> { // Message received
                        addChat("[${senderName ?: "?"}] $data")
                    }
                    5 -> { // File offered
                        addChat("[File] ${senderName} offers: $data (${formatSize(value)})")
                        showFileOfferDialog(senderName ?: "?", data ?: "?", value, extra ?: "")
                    }
                    6 -> { // File progress
                        addChat("[File] Transfer: ${value}%")
                    }
                    7 -> { // File complete
                        addChat("[File] Received: $data")
                    }
                    8 -> { // Voice received
                        val secs = value.toFloat() / 1000f
                        val binaryData = intent.getByteArrayExtra("binary_data")
                        addChat("[Voice] ${senderName}: ${String.format("%.1f", secs)}s - tap to play")
                        if (binaryData != null && binaryData.isNotEmpty()) {
                            voiceNotes.add(VoiceNote(senderName ?: "?", binaryData, value))
                            // Store index for playback on tap
                            val noteIdx = voiceNotes.size - 1
                            chatMessages[chatMessages.size - 1] = "[Voice] ${senderName}: ${String.format("%.1f", secs)}s [Play #$noteIdx]"
                            chatAdapter.notifyDataSetChanged()
                        }
                    }
                    9 -> { // Profile updated
                        peerEntries.find { it.nodeId == nodeId }?.let {
                            val idx = peerEntries.indexOf(it)
                            peerEntries[idx] = it.copy(displayName = data ?: it.displayName, bio = extra ?: "")
                            updatePeerList()
                        }
                    }
                    10 -> { // Gateway found
                        gatewayName = data
                        peerEntries.find { it.nodeId == nodeId }?.isGateway = true
                        updatePeerList()
                        updateHeader()
                    }
                    11 -> { // Stats (includes connectivity info)
                        data?.let { parseStatsJson(it) }
                    }
                    12 -> { // SOS received
                        addChat("!!! SOS from ${senderName}: $data")
                    }
                    13 -> { // Incoming call
                        val peerName = data ?: "Unknown"
                        // If we already initiated a call to this peer, this is just
                        // the other side's CallStart response — ignore it
                        if (inCall != null) {
                            // Already in a call, skip
                        } else {
                            addChat("[Call] Incoming from $peerName")
                            showIncomingCallDialog(nodeId ?: "", peerName)
                        }
                    }
                    14 -> { // Audio frame
                        val binaryData = intent.getByteArrayExtra("binary_data")
                        if (binaryData != null) {
                            callPlaybackBuffer.offer(binaryData)
                        }
                    }
                    15 -> { // Call ended
                        addChat("[Call] Ended")
                        endCall()
                    }
                    17 -> { // Public broadcast
                        addChat("[PUBLIC] ${senderName}: $data")
                    }
                    18 -> { // Gateway lost
                        gatewayName = null
                        peerEntries.find { it.nodeId == nodeId }?.isGateway = false
                        updatePeerList()
                        updateHeader()
                    }
                    20 -> { // Stopped
                        peerCount = 0
                        peerEntries.clear()
                        updatePeerList()
                        updateHeader()
                        updateRadar()
                    }
                    21 -> { // MessageDelivered
                        // Could update message status indicators
                    }
                    22 -> { // TypingStarted
                        val name = senderName ?: nodeId?.take(8) ?: "?"
                        typingPeers[nodeId ?: ""] = System.currentTimeMillis()
                        updateTypingIndicator()
                    }
                    23 -> { // TypingStopped
                        typingPeers.remove(nodeId ?: "")
                        updateTypingIndicator()
                    }
                    24 -> { // GroupMessageReceived
                        val group = extra ?: "?"
                        val msg = "[$group] ${senderName ?: "?"}: $data"
                        addChat(msg)
                        groupMessages.getOrPut(group) { mutableListOf() }.add(msg)
                    }
                    25 -> { // GroupJoined
                        addChat("[Group] ${senderName ?: nodeId?.take(8)} joined $extra")
                    }
                    26 -> { // GroupLeft
                        addChat("[Group] ${nodeId?.take(8)} left $extra")
                    }
                    27 -> { // TriageReceived
                        val msg = "[TRIAGE] ${senderName}: $data"
                        addChat(msg)
                        triageLog.add(msg)
                    }
                    28 -> { // ResourceRequestReceived
                        val msg = "[RESOURCE] ${senderName}: $data"
                        addChat(msg)
                        resourceLog.add(msg)
                    }
                    29 -> { // CheckInReceived
                        val msg = "[CHECK-IN] ${senderName}: $data"
                        addChat(msg)
                        safetyRoster[nodeId ?: ""] = "${senderName}: $data"
                    }
                    30 -> { // DisappearingReceived
                        val ttl = value
                        addChat("[Disappearing ${ttl}s] ${senderName}: $data")
                        disappearingMsgs.add(Triple(
                            "${senderName}: $data",
                            data ?: "",
                            System.currentTimeMillis() + ttl * 1000
                        ))
                    }
                    31 -> { // HistoryLoaded
                        // Could populate chat from stored messages
                    }
                }
            }
        }
    }

    // --- Lifecycle ---

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        buildUI()
        requestPermissions()
        switchTab("chat")
    }

    override fun onStart() {
        super.onStart()
        Intent(this, MeshService::class.java).also {
            bindService(it, serviceConnection, Context.BIND_AUTO_CREATE)
        }
    }

    override fun onResume() {
        super.onResume()
        LocalBroadcastManager.getInstance(this)
            .registerReceiver(meshEventReceiver, IntentFilter(MeshService.BROADCAST_ACTION))
        // Register WiFi Direct receiver
        wifiDirectManager?.let {
            registerReceiver(it.receiver, it.getIntentFilter())
        }
    }

    override fun onPause() {
        super.onPause()
        LocalBroadcastManager.getInstance(this).unregisterReceiver(meshEventReceiver)
        // Unregister WiFi Direct receiver
        wifiDirectManager?.let {
            try { unregisterReceiver(it.receiver) } catch (_: Exception) {}
        }
    }

    override fun onStop() {
        super.onStop()
        if (serviceBound) {
            unbindService(serviceConnection)
            serviceBound = false
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        stopRecording()
        endCall()
        bluetoothTransport?.stop()
        wifiDirectManager?.stop()
    }

    // --- Build UI ---

    private fun buildUI() {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
        }

        // Header bar
        val header = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setPadding(16, 12, 16, 12)
            setBackgroundColor(0xFF1A1A2E.toInt())
            gravity = Gravity.CENTER_VERTICAL
        }

        // Header logo
        val headerLogo = ImageView(this).apply {
            setImageResource(R.mipmap.ic_launcher)
            layoutParams = LinearLayout.LayoutParams(64, 64).apply { marginEnd = 8 }
            scaleType = ImageView.ScaleType.FIT_CENTER
        }
        header.addView(headerLogo)

        headerStatus = TextView(this).apply {
            text = "MassKritical"
            textSize = 16f
            setTextColor(0xFF00D4FF.toInt())
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }
        header.addView(headerStatus)

        headerPeers = TextView(this).apply {
            text = "Peers: 0"
            textSize = 14f
            setTextColor(0xFF4CAF50.toInt())
            setPadding(16, 0, 16, 0)
        }
        header.addView(headerPeers)

        headerGateway = TextView(this).apply {
            text = ""
            textSize = 12f
            setTextColor(0xFFFFD700.toInt())
        }
        header.addView(headerGateway)

        headerConnectivity = TextView(this).apply {
            text = ""
            textSize = 11f
            setTextColor(0xFF4CAF50.toInt())
            setPadding(8, 0, 0, 0)
        }
        header.addView(headerConnectivity)

        root.addView(header)

        // Content frame
        contentFrame = FrameLayout(this).apply {
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f
            )
        }
        root.addView(contentFrame)

        // Bottom tab bar
        val tabBar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setBackgroundColor(0xFF1A1A2E.toInt())
            setPadding(0, 8, 0, 8)
        }

        tabRadar = makeTabButton("Radar") { switchTab("radar") }
        tabChat = makeTabButton("Chat") { switchTab("chat") }
        tabPeers = makeTabButton("Peers") { switchTab("peers") }
        tabEmergency = makeTabButton("SOS") { switchTab("emergency") }
        tabSettings = makeTabButton("Settings") { switchTab("settings") }

        tabBar.addView(tabRadar)
        tabBar.addView(tabChat)
        tabBar.addView(tabPeers)
        tabBar.addView(tabEmergency)
        tabBar.addView(tabSettings)

        root.addView(tabBar)

        // Build tab content views
        buildChatView()
        buildPeersView()
        buildRadarView()
        buildEmergencyView()
        buildSettingsView()

        setContentView(root)
    }

    private fun makeTabButton(label: String, onClick: () -> Unit): TextView {
        return TextView(this).apply {
            text = label
            textSize = 14f
            setTextColor(0xFFAAAAAA.toInt())
            gravity = Gravity.CENTER
            setPadding(8, 12, 8, 12)
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
            setOnClickListener { onClick() }
        }
    }

    // --- Chat Tab ---

    private lateinit var chatView: LinearLayout

    private fun buildChatView() {
        chatView = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(8, 8, 8, 8)
        }

        chatAdapter = ArrayAdapter(this, android.R.layout.simple_list_item_1, chatMessages)
        chatListView = ListView(this).apply {
            adapter = chatAdapter
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f
            )
            dividerHeight = 0
            setOnItemClickListener { _, _, position, _ ->
                val msg = chatMessages.getOrNull(position) ?: return@setOnItemClickListener
                // Check for voice note playback
                val playMatch = Regex("\\[Play #(\\d+)]").find(msg)
                if (playMatch != null) {
                    val idx = playMatch.groupValues[1].toIntOrNull()
                    if (idx != null && idx < voiceNotes.size) {
                        playVoiceNote(voiceNotes[idx])
                    }
                }
            }
        }
        chatView.addView(chatListView)

        // Call banner (hidden by default, shown during active call)
        callBannerLayout = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            visibility = View.GONE
            setBackgroundColor(0xFF2D1B4E.toInt())
            setPadding(12, 10, 12, 10)
            gravity = Gravity.CENTER_VERTICAL
        }
        callBannerText = TextView(this).apply {
            text = ""
            textSize = 14f
            setTextColor(0xFFE0E0E0.toInt())
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }
        callBannerLayout.addView(callBannerText)
        val endCallBtn = Button(this).apply {
            text = "End Call"
            setBackgroundColor(0xFFD32F2F.toInt())
            setTextColor(0xFFFFFFFF.toInt())
            setOnClickListener { endCall() }
        }
        callBannerLayout.addView(endCallBtn)
        chatView.addView(callBannerLayout)

        val inputRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setPadding(0, 8, 0, 0)
            gravity = Gravity.CENTER_VERTICAL
        }

        // Mic button for voice notes
        chatMicBtn = Button(this).apply {
            text = "Mic"
            setOnClickListener {
                if (isRecording) {
                    stopRecordingAndSend()
                } else {
                    startRecording()
                }
            }
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            )
        }
        inputRow.addView(chatMicBtn)

        // Call button — calls the current DM target
        chatCallBtn = Button(this).apply {
            text = "Call"
            setOnClickListener {
                val target = dmTarget
                if (target != null) {
                    startCall(target)
                } else if (peerEntries.size == 1) {
                    startCall(peerEntries.first())
                } else if (peerEntries.isNotEmpty()) {
                    showCallPeerPicker()
                } else {
                    Toast.makeText(this@MainActivity, "No peers to call", Toast.LENGTH_SHORT).show()
                }
            }
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            )
        }
        inputRow.addView(chatCallBtn)

        chatAttachBtn = Button(this).apply {
            text = "+"
            setOnClickListener { filePicker.launch(arrayOf("*/*")) }
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            )
        }
        inputRow.addView(chatAttachBtn)

        chatInput = EditText(this).apply {
            hint = "Type a message..."
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }
        inputRow.addView(chatInput)

        chatSendBtn = Button(this).apply {
            text = "Send"
            setOnClickListener { sendMessage() }
        }
        inputRow.addView(chatSendBtn)

        chatView.addView(inputRow)
    }

    private fun showCallPeerPicker() {
        val names = peerEntries.map { it.displayName }.toTypedArray()
        AlertDialog.Builder(this)
            .setTitle("Call who?")
            .setItems(names) { _, which ->
                if (which < peerEntries.size) {
                    startCall(peerEntries[which])
                }
            }
            .show()
    }

    private fun updateCallBanner() {
        val peer = inCall
        if (peer != null && callActive) {
            callBannerText.text = "In call with ${peer.displayName}"
            callBannerLayout.visibility = View.VISIBLE
        } else {
            callBannerLayout.visibility = View.GONE
        }
    }

    // --- Peers Tab ---

    private lateinit var peersView: LinearLayout

    private fun buildPeersView() {
        peersView = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(8, 8, 8, 8)
        }

        val title = TextView(this).apply {
            text = "Connected Peers"
            textSize = 18f
            setPadding(8, 8, 8, 16)
        }
        peersView.addView(title)

        peerAdapter = ArrayAdapter(this, android.R.layout.simple_list_item_1, peerDisplayList)
        peersListView = ListView(this).apply {
            adapter = peerAdapter
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f
            )
            setOnItemClickListener { _, _, position, _ ->
                if (position < peerEntries.size) {
                    showPeerActions(peerEntries[position])
                }
            }
        }
        peersView.addView(peersListView)
    }

    // --- Radar Tab ---

    private lateinit var radarView: LinearLayout

    private fun buildRadarView() {
        radarView = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setPadding(32, 32, 32, 32)
        }

        // Logo on radar page
        val radarLogo = ImageView(this).apply {
            setImageResource(R.mipmap.ic_launcher)
            layoutParams = LinearLayout.LayoutParams(128, 128).apply { bottomMargin = 16 }
            scaleType = ImageView.ScaleType.FIT_CENTER
        }
        radarView.addView(radarLogo)

        radarPeerCount = TextView(this).apply {
            text = "0"
            textSize = 72f
            gravity = Gravity.CENTER
            setTextColor(0xFF00D4FF.toInt())
        }
        radarView.addView(radarPeerCount)

        val label = TextView(this).apply {
            text = "Peers Nearby"
            textSize = 20f
            gravity = Gravity.CENTER
            setPadding(0, 0, 0, 32)
        }
        radarView.addView(label)

        radarText = TextView(this).apply {
            text = "Tap below to start the mesh node"
            textSize = 14f
            gravity = Gravity.CENTER
            setLineSpacing(4f, 1.3f)
        }
        radarView.addView(radarText)

        radarActionBtn = Button(this).apply {
            text = "Start Mesh Node"
            setOnClickListener { toggleMeshService() }
            setPadding(32, 16, 32, 16)
        }
        radarView.addView(radarActionBtn)
    }

    // --- Emergency Tab ---

    private lateinit var emergencyView: LinearLayout
    private lateinit var triageListView: ListView
    private lateinit var triageAdapter: ArrayAdapter<String>
    private lateinit var resourceListView: ListView
    private lateinit var resourceAdapter: ArrayAdapter<String>
    private lateinit var rosterText: TextView
    private val triageDisplay = mutableListOf<String>()
    private val resourceDisplay = mutableListOf<String>()

    private fun buildEmergencyView() {
        emergencyView = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(16, 16, 16, 16)
        }

        // "I'm OK" button - prominent
        val imOkBtn = Button(this).apply {
            text = "I'm OK"
            textSize = 20f
            setBackgroundColor(0xFF4CAF50.toInt())
            setTextColor(0xFFFFFFFF.toInt())
            setPadding(32, 24, 32, 24)
            setOnClickListener {
                MeshBridge.meshSendCheckIn("ok", 0.0, 0.0, "I'm OK")
                Toast.makeText(this@MainActivity, "Check-in sent", Toast.LENGTH_SHORT).show()
                addChat("[CHECK-IN] You: I'm OK")
            }
        }
        emergencyView.addView(imOkBtn)

        // "Need Help" button
        val helpBtn = Button(this).apply {
            text = "NEED HELP"
            textSize = 16f
            setBackgroundColor(0xFFFF5722.toInt())
            setTextColor(0xFFFFFFFF.toInt())
            setPadding(32, 16, 32, 16)
            setOnClickListener {
                MeshBridge.meshSendCheckIn("need_help", 0.0, 0.0, "Need assistance")
                Toast.makeText(this@MainActivity, "Help request sent", Toast.LENGTH_SHORT).show()
                addChat("[CHECK-IN] You: NEED HELP")
            }
        }
        val helpParams = LinearLayout.LayoutParams(
            LinearLayout.LayoutParams.MATCH_PARENT,
            LinearLayout.LayoutParams.WRAP_CONTENT
        ).apply { topMargin = 8 }
        helpBtn.layoutParams = helpParams
        emergencyView.addView(helpBtn)

        // Triage section
        val triageHeader = TextView(this).apply {
            text = "Triage Log"
            textSize = 16f
            setTextColor(0xFFFF4444.toInt())
            setPadding(0, 24, 0, 8)
        }
        emergencyView.addView(triageHeader)

        // Quick triage buttons row
        val triageRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
        }
        val triageLevels = listOf("RED" to 0xFFFF0000.toInt(), "YEL" to 0xFFFFEB3B.toInt(), "GRN" to 0xFF4CAF50.toInt(), "BLK" to 0xFF333333.toInt())
        triageLevels.forEachIndexed { idx, (label, color) ->
            val btn = Button(this).apply {
                text = label
                setBackgroundColor(color)
                setTextColor(0xFFFFFFFF.toInt())
                layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
                setOnClickListener { showTriageDialog(idx) }
            }
            triageRow.addView(btn)
        }
        emergencyView.addView(triageRow)

        triageAdapter = ArrayAdapter(this, android.R.layout.simple_list_item_1, triageDisplay)
        triageListView = ListView(this).apply {
            adapter = triageAdapter
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f
            )
            dividerHeight = 0
        }
        emergencyView.addView(triageListView)

        // Resource requests section
        val resHeader = TextView(this).apply {
            text = "Resource Requests"
            textSize = 16f
            setTextColor(0xFFFFD700.toInt())
            setPadding(0, 16, 0, 8)
        }
        emergencyView.addView(resHeader)

        val resBtn = Button(this).apply {
            text = "Request Resources"
            setOnClickListener { showResourceDialog() }
        }
        emergencyView.addView(resBtn)

        resourceAdapter = ArrayAdapter(this, android.R.layout.simple_list_item_1, resourceDisplay)
        resourceListView = ListView(this).apply {
            adapter = resourceAdapter
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f
            )
            dividerHeight = 0
        }
        emergencyView.addView(resourceListView)

        // Safety roster
        val rosterHeader = TextView(this).apply {
            text = "Safety Roster"
            textSize = 16f
            setTextColor(0xFF4CAF50.toInt())
            setPadding(0, 16, 0, 8)
        }
        emergencyView.addView(rosterHeader)

        rosterText = TextView(this).apply {
            text = "No check-ins received yet"
            textSize = 13f
            setLineSpacing(4f, 1.2f)
        }
        emergencyView.addView(rosterText)
    }

    private fun showTriageDialog(levelIdx: Int) {
        val levelNames = arrayOf("Red - Immediate", "Yellow - Delayed", "Green - Minor", "Black - Expectant")
        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(32, 16, 32, 16)
        }
        val victimInput = EditText(this).apply { hint = "Victim ID" }
        val notesInput = EditText(this).apply { hint = "Notes (injuries, situation)" }
        layout.addView(victimInput)
        layout.addView(notesInput)

        AlertDialog.Builder(this)
            .setTitle("Triage: ${levelNames[levelIdx]}")
            .setView(layout)
            .setPositiveButton("Send") { _, _ ->
                val victim = victimInput.text.toString().trim().ifEmpty { "unknown" }
                val notes = notesInput.text.toString().trim()
                MeshBridge.meshSendTriage(levelIdx, victim, notes, 0.0, 0.0)
                val msg = "[TRIAGE] ${levelNames[levelIdx]}: $victim - $notes"
                triageLog.add(msg)
                addChat(msg)
                updateEmergencyView()
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun showResourceDialog() {
        val categories = arrayOf("medical", "water", "food", "shelter", "evacuation", "power", "communication", "other")
        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(32, 16, 32, 16)
        }
        val catSpinner = Spinner(this).apply {
            adapter = ArrayAdapter(this@MainActivity, android.R.layout.simple_spinner_dropdown_item, categories)
        }
        val descInput = EditText(this).apply { hint = "Description" }
        val urgencyInput = EditText(this).apply {
            hint = "Urgency (1-5)"
            inputType = android.text.InputType.TYPE_CLASS_NUMBER
        }
        layout.addView(catSpinner)
        layout.addView(descInput)
        layout.addView(urgencyInput)

        AlertDialog.Builder(this)
            .setTitle("Request Resources")
            .setView(layout)
            .setPositiveButton("Send") { _, _ ->
                val cat = categories[catSpinner.selectedItemPosition]
                val desc = descInput.text.toString().trim()
                val urgency = urgencyInput.text.toString().toIntOrNull()?.coerceIn(1, 5) ?: 3
                MeshBridge.meshSendResourceRequest(cat, desc, urgency, 0.0, 0.0, 1)
                val msg = "[RESOURCE] $cat (urgency $urgency): $desc"
                resourceLog.add(msg)
                addChat(msg)
                updateEmergencyView()
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun updateEmergencyView() {
        triageDisplay.clear()
        triageDisplay.addAll(triageLog)
        triageAdapter.notifyDataSetChanged()

        resourceDisplay.clear()
        resourceDisplay.addAll(resourceLog)
        resourceAdapter.notifyDataSetChanged()

        if (safetyRoster.isNotEmpty()) {
            rosterText.text = safetyRoster.values.joinToString("\n")
        } else {
            rosterText.text = "No check-ins received yet"
        }
    }

    // --- Typing indicator ---

    private fun updateTypingIndicator() {
        val now = System.currentTimeMillis()
        // Remove expired typing indicators (>5 seconds old)
        typingPeers.entries.removeAll { now - it.value > 5000 }

        if (typingPeers.isEmpty()) return
        val names = typingPeers.keys.mapNotNull { id ->
            peerEntries.find { it.nodeId == id }?.displayName ?: id.take(8)
        }
        if (names.isNotEmpty()) {
            val typing = if (names.size == 1) "${names[0]} is typing..."
            else "${names.joinToString(", ")} are typing..."
            // Could show this in a dedicated TextView above input
        }
    }

    // --- Settings Tab ---

    private fun buildSettingsView() {
        settingsLayout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(24, 24, 24, 24)
        }

        addSettingsSection("Profile")

        val nameRow = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        val nameInput = EditText(this).apply {
            hint = "Display name"
            setText(Build.MODEL)
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }
        nameRow.addView(nameInput)

        val bioInput = EditText(this).apply {
            hint = "Bio"
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }

        val nameBtn = Button(this).apply {
            text = "Update"
            setOnClickListener {
                val name = nameInput.text.toString().trim()
                val bio = bioInput.text.toString().trim()
                if (name.isNotEmpty()) {
                    MeshBridge.meshUpdateProfile(name, bio)
                    Toast.makeText(this@MainActivity, "Profile updated", Toast.LENGTH_SHORT).show()
                }
            }
        }
        nameRow.addView(nameBtn)
        settingsLayout.addView(nameRow)

        val bioRow = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        bioRow.addView(bioInput)
        settingsLayout.addView(bioRow)

        addSettingsSection("Node Info")

        val nodeIdText = TextView(this).apply {
            text = "Node ID: ${MeshBridge.meshGetNodeId() ?: "Not started"}"
            textSize = 12f
            setPadding(0, 4, 0, 4)
            setOnClickListener {
                MeshBridge.meshGetNodeId()?.let { id ->
                    val clip = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                    clip.setPrimaryClip(ClipData.newPlainText("Node ID", id))
                    Toast.makeText(this@MainActivity, "Node ID copied", Toast.LENGTH_SHORT).show()
                }
            }
        }
        settingsLayout.addView(nodeIdText)

        val encText = TextView(this).apply {
            text = "Encryption: X25519 + ChaCha20-Poly1305"
            textSize = 12f
            setPadding(0, 4, 0, 16)
        }
        settingsLayout.addView(encText)

        addSettingsSection("Connectivity")

        val connectivityInfo = TextView(this).apply {
            text = "Active: detecting..."
            textSize = 13f
            tag = "connectivity_info"
            setPadding(0, 4, 0, 16)
        }
        settingsLayout.addView(connectivityInfo)

        // Bluetooth Mesh toggle
        val btPrefs = getSharedPreferences("mesh_transport_prefs", MODE_PRIVATE)
        val btRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setPadding(0, 4, 0, 8)
            gravity = Gravity.CENTER_VERTICAL
        }
        val btLabel = TextView(this).apply {
            text = "Bluetooth Mesh"
            textSize = 14f
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }
        btRow.addView(btLabel)
        val btSwitch = Switch(this).apply {
            isChecked = btPrefs.getBoolean("bt_enabled", false)
            tag = "bt_switch"
            setOnCheckedChangeListener { _, isChecked ->
                btPrefs.edit().putBoolean("bt_enabled", isChecked).apply()
                if (isChecked) {
                    enableBluetooth()
                } else {
                    disableBluetooth()
                }
            }
        }
        btRow.addView(btSwitch)
        settingsLayout.addView(btRow)

        val btStatusText = TextView(this).apply {
            text = ""
            textSize = 12f
            tag = "bt_status"
            setPadding(0, 0, 0, 8)
            setTextColor(0xFF888888.toInt())
        }
        settingsLayout.addView(btStatusText)

        // WiFi Direct toggle
        val wdRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setPadding(0, 4, 0, 8)
            gravity = Gravity.CENTER_VERTICAL
        }
        val wdLabel = TextView(this).apply {
            text = "WiFi Direct"
            textSize = 14f
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }
        wdRow.addView(wdLabel)
        val wdSwitch = Switch(this).apply {
            isChecked = btPrefs.getBoolean("wd_enabled", false)
            tag = "wd_switch"
            setOnCheckedChangeListener { _, isChecked ->
                btPrefs.edit().putBoolean("wd_enabled", isChecked).apply()
                if (isChecked) {
                    enableWifiDirect()
                } else {
                    disableWifiDirect()
                }
            }
        }
        wdRow.addView(wdSwitch)
        settingsLayout.addView(wdRow)

        val wdStatusText = TextView(this).apply {
            text = ""
            textSize = 12f
            tag = "wd_status"
            setPadding(0, 0, 0, 16)
            setTextColor(0xFF888888.toInt())
        }
        settingsLayout.addView(wdStatusText)

        addSettingsSection("Safety")

        val nukeBtn = Button(this).apply {
            text = "NUKE - Destroy All Data"
            setBackgroundColor(0xFFD32F2F.toInt())
            setTextColor(0xFFFFFFFF.toInt())
            setOnClickListener { confirmNuke() }
        }
        settingsLayout.addView(nukeBtn)

        addSettingsSection("Groups")

        val groupBtnRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setPadding(0, 4, 0, 8)
        }
        val joinGroupBtn = Button(this).apply {
            text = "Join Group"
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
            setOnClickListener { showJoinGroupDialog() }
        }
        groupBtnRow.addView(joinGroupBtn)
        val leaveGroupBtn = Button(this).apply {
            text = "Leave Group"
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
            setOnClickListener { showLeaveGroupDialog() }
        }
        groupBtnRow.addView(leaveGroupBtn)
        settingsLayout.addView(groupBtnRow)

        val groupInfo = TextView(this).apply {
            text = if (joinedGroups.isEmpty()) "No groups joined" else "Groups: ${joinedGroups.joinToString(", ") { "#$it" }}"
            textSize = 12f
            setPadding(0, 0, 0, 8)
            tag = "groups_info"
        }
        settingsLayout.addView(groupInfo)

        val activeGroupInfo = TextView(this).apply {
            text = if (activeGroup != null) "Active: #$activeGroup (messages go here)" else "No active group (messages go to main chat)"
            textSize = 12f
            setPadding(0, 0, 0, 16)
            tag = "active_group_info"
            setTextColor(0xFF00D4FF.toInt())
        }
        settingsLayout.addView(activeGroupInfo)

        addSettingsSection("About")

        val aboutText = TextView(this).apply {
            text = "MassKritical v0.3.0\n" +
                    "Disaster Recovery Mesh Network\n" +
                    "Text, Voice, Files, Groups, Triage, Check-In\n" +
                    "End-to-end encrypted\n" +
                    "Kraftbox 2026"
            textSize = 13f
            setLineSpacing(4f, 1.2f)
        }
        settingsLayout.addView(aboutText)
    }

    private fun addSettingsSection(title: String) {
        val header = TextView(this).apply {
            text = title
            textSize = 16f
            setPadding(0, 24, 0, 8)
            setTextColor(0xFF00D4FF.toInt())
        }
        settingsLayout.addView(header)
    }

    // --- Tab switching ---

    private fun switchTab(tab: String) {
        currentTab = tab
        contentFrame.removeAllViews()

        listOf(tabRadar, tabChat, tabPeers, tabEmergency, tabSettings).forEach {
            it.setTextColor(0xFFAAAAAA.toInt())
        }

        when (tab) {
            "radar" -> {
                detachFromParent(radarView)
                contentFrame.addView(radarView)
                tabRadar.setTextColor(0xFF00D4FF.toInt())
                updateRadar()
            }
            "chat" -> {
                detachFromParent(chatView)
                contentFrame.addView(chatView)
                tabChat.setTextColor(0xFF00D4FF.toInt())
            }
            "peers" -> {
                detachFromParent(peersView)
                contentFrame.addView(peersView)
                tabPeers.setTextColor(0xFF00D4FF.toInt())
            }
            "emergency" -> {
                detachFromParent(emergencyView)
                contentFrame.addView(emergencyView)
                tabEmergency.setTextColor(0xFFFF4444.toInt())
                updateEmergencyView()
            }
            "settings" -> {
                detachFromParent(settingsLayout)
                contentFrame.addView(settingsLayout)
                tabSettings.setTextColor(0xFF00D4FF.toInt())
                // Request stats to get connectivity info
                MeshBridge.meshGetStats()
            }
        }
    }

    private fun detachFromParent(view: View) {
        (view.parent as? ViewGroup)?.removeView(view)
    }

    // --- Actions ---

    private fun startMeshService() {
        val intent = Intent(this, MeshService::class.java).apply {
            putExtra("name", Build.MODEL)
            putExtra("port", 7332)
        }
        ContextCompat.startForegroundService(this, intent)
        headerStatus.text = "Starting..."
    }

    private fun stopMeshService() {
        val intent = Intent(this, MeshService::class.java).apply {
            action = MeshService.ACTION_STOP
        }
        startService(intent)
        peerCount = 0
        peerEntries.clear()
        updatePeerList()
        updateHeader()
        updateRadar()
    }

    private fun toggleMeshService() {
        if (meshService?.isNodeRunning() == true) {
            stopMeshService()
        } else {
            startMeshService()
        }
    }

    private fun sendMessage() {
        val text = chatInput.text.toString().trim()
        if (text.isEmpty()) return

        // Handle slash commands
        if (text.startsWith("/")) {
            handleSlashCommand(text)
            chatInput.text.clear()
            return
        }

        // Send to active group if set
        val group = activeGroup
        if (group != null) {
            val result = MeshBridge.meshSendGroupMessage(group, text)
            if (result == 0) {
                addChat("[$group] You: $text")
                chatInput.text.clear()
            } else {
                addChat("[!] Failed to send group message")
            }
            return
        }

        val target = dmTarget
        val result = if (target != null) {
            MeshBridge.meshSendDirect(target.nodeId, text)
        } else {
            MeshBridge.meshSendBroadcast(text)
        }

        // Also send via Bluetooth transport
        bluetoothTransport?.sendText(Build.MODEL, text)

        if (result == 0) {
            val prefix = if (target != null) "[DM to ${target.displayName}]" else "[You]"
            addChat("$prefix $text")
            chatInput.text.clear()
        } else if (bluetoothTransport != null && bluetoothTransport!!.getPeerCount() > 0) {
            // Rust mesh failed but BT sent successfully
            val prefix = if (target != null) "[DM to ${target.displayName}]" else "[You]"
            addChat("$prefix $text")
            chatInput.text.clear()
        } else {
            addChat("[!] Failed to send message")
        }
    }

    private fun handleSlashCommand(cmd: String) {
        val parts = cmd.split(" ", limit = 3)
        when (parts[0].lowercase()) {
            "/group" -> {
                if (parts.size < 3) {
                    addChat("[!] Usage: /group join|leave <name>")
                    return
                }
                val action = parts[1].lowercase()
                val name = parts[2]
                when (action) {
                    "join" -> {
                        MeshBridge.meshJoinGroup(name)
                        joinedGroups.add(name)
                        activeGroup = name
                        chatInput.hint = "Message to #$name..."
                        addChat("[*] Joined group #$name")
                    }
                    "leave" -> {
                        MeshBridge.meshLeaveGroup(name)
                        joinedGroups.remove(name)
                        if (activeGroup == name) {
                            activeGroup = null
                            chatInput.hint = "Type a message..."
                        }
                        addChat("[*] Left group #$name")
                    }
                    else -> addChat("[!] Usage: /group join|leave <name>")
                }
            }
            "/sos" -> {
                val msg = if (parts.size > 1) parts.drop(1).joinToString(" ") else "Emergency!"
                MeshBridge.meshSendSOS(msg, 0.0, 0.0)
                addChat("[SOS] Sent: $msg")
            }
            "/checkin" -> {
                val status = if (parts.size > 1) parts[1] else "ok"
                val msg = if (parts.size > 2) parts[2] else ""
                MeshBridge.meshSendCheckIn(status, 0.0, 0.0, msg)
                addChat("[CHECK-IN] $status $msg")
            }
            "/disappear" -> {
                if (parts.size < 3) {
                    addChat("[!] Usage: /disappear <seconds> <message>")
                    return
                }
                val ttl = parts[1].toIntOrNull() ?: 30
                val msg = parts[2]
                val destHex = dmTarget?.nodeId
                MeshBridge.meshSendDisappearing(destHex, msg, ttl)
                addChat("[Disappearing ${ttl}s] You: $msg")
            }
            "/broadcast" -> {
                val msg = if (parts.size > 1) parts.drop(1).joinToString(" ") else ""
                if (msg.isNotEmpty()) {
                    MeshBridge.meshSendPublicBroadcast(msg)
                    addChat("[PUBLIC] You: $msg")
                }
            }
            else -> addChat("[!] Unknown command: ${parts[0]}")
        }
    }

    private fun addChat(msg: String) {
        chatMessages.add(msg)
        chatAdapter.notifyDataSetChanged()
        chatListView.setSelection(chatMessages.size - 1)
    }

    private fun updateHeader() {
        val status = if (meshService?.isNodeRunning() == true) {
            "MassKritical (${nodeIdShort ?: "..."})"
        } else {
            "MassKritical - Stopped"
        }
        headerStatus.text = status
        val btSuffix = if (btPeerCount > 0) " | BT: $btPeerCount" else ""
        headerPeers.text = "Peers: $peerCount$btSuffix"
        headerGateway.text = if (gatewayName != null) "GW: $gatewayName" else ""
        headerConnectivity.text = activeInterface
    }

    private fun updatePeerList() {
        peerDisplayList.clear()
        peerEntries.forEach { p ->
            val gw = if (p.isGateway) " [GW]" else ""
            val bio = if (p.bio.isNotEmpty()) " - ${p.bio}" else ""
            peerDisplayList.add("${p.displayName} (${p.nodeId.take(8)})$gw$bio")
        }
        peerAdapter.notifyDataSetChanged()
    }

    private fun updateRadar() {
        radarPeerCount.text = "$peerCount"
        val running = meshService?.isNodeRunning() == true
        if (running) {
            radarActionBtn.text = "Stop Mesh Node"
            radarActionBtn.setBackgroundColor(0xFFD32F2F.toInt())
            radarActionBtn.setTextColor(0xFFFFFFFF.toInt())
            val lines = peerEntries.joinToString("\n") { p ->
                val gw = if (p.isGateway) " [Gateway]" else ""
                "  ${p.displayName}$gw"
            }
            radarText.text = if (lines.isEmpty()) {
                "Node running. Scanning for peers...\nMake sure another node is running nearby."
            } else {
                "Connected nodes:\n$lines"
            }
        } else {
            radarActionBtn.text = "Start Mesh Node"
            radarActionBtn.setBackgroundColor(0xFF4CAF50.toInt())
            radarActionBtn.setTextColor(0xFFFFFFFF.toInt())
            radarText.text = "Tap below to start the mesh node"
        }
    }

    private fun showPeerActions(peer: PeerInfo) {
        val items = arrayOf(
            "Send Direct Message",
            "Send File",
            "Start Voice Call",
            "View Node ID"
        )
        AlertDialog.Builder(this)
            .setTitle(peer.displayName)
            .setItems(items) { _, which ->
                when (which) {
                    0 -> {
                        dmTarget = peer
                        switchTab("chat")
                        chatInput.hint = "DM to ${peer.displayName}..."
                        addChat("[*] DM mode: ${peer.displayName}")
                    }
                    1 -> {
                        dmTarget = peer
                        filePicker.launch(arrayOf("*/*"))
                    }
                    2 -> {
                        startCall(peer)
                    }
                    3 -> {
                        val clip = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                        clip.setPrimaryClip(ClipData.newPlainText("Node ID", peer.nodeId))
                        Toast.makeText(this, "Node ID copied", Toast.LENGTH_SHORT).show()
                    }
                }
            }
            .show()
    }

    private fun showFileOfferDialog(sender: String, filename: String, size: Long, fileIdHex: String) {
        AlertDialog.Builder(this)
            .setTitle("File Offer")
            .setMessage("$sender wants to send:\n$filename (${formatSize(size)})")
            .setPositiveButton("Accept") { _, _ ->
                MeshBridge.meshAcceptFile(fileIdHex)
                addChat("[File] Accepted: $filename")
            }
            .setNegativeButton("Decline", null)
            .show()
    }

    private fun confirmNuke() {
        AlertDialog.Builder(this)
            .setTitle("NUKE - Destroy All Data")
            .setMessage("This will permanently destroy your identity key, all messages, and all received files.\n\nThis cannot be undone.")
            .setPositiveButton("DESTROY") { _, _ ->
                MeshBridge.meshNuke()
                Toast.makeText(this, "Identity destroyed", Toast.LENGTH_LONG).show()
                getSharedPreferences("mesh_pin_prefs", MODE_PRIVATE).edit().clear().apply()
                getSharedPreferences("mesh_onboarding", MODE_PRIVATE).edit().clear().apply()
                finish()
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun handleFilePicked(uri: Uri) {
        val target = dmTarget ?: run {
            if (peerEntries.isEmpty()) {
                Toast.makeText(this, "No peers to send to", Toast.LENGTH_SHORT).show()
                return
            }
            peerEntries.first()
        }

        try {
            val inputStream = contentResolver.openInputStream(uri) ?: return
            val fileName = getFileName(uri) ?: "file"
            val tempFile = java.io.File(filesDir, "send_$fileName")
            tempFile.outputStream().use { output ->
                inputStream.copyTo(output)
            }
            inputStream.close()

            val result = MeshBridge.meshSendFile(target.nodeId, tempFile.absolutePath)
            if (result == 0) {
                addChat("[File] Sending $fileName to ${target.displayName}")
            } else {
                addChat("[!] Failed to send file")
            }
        } catch (e: Exception) {
            addChat("[!] File error: ${e.message}")
        }
    }

    private fun getFileName(uri: Uri): String? {
        val cursor = contentResolver.query(uri, null, null, null, null) ?: return null
        cursor.use {
            if (it.moveToFirst()) {
                val idx = it.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (idx >= 0) return it.getString(idx)
            }
        }
        return null
    }

    // --- Voice Notes ---

    private fun startRecording() {
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
            != PackageManager.PERMISSION_GRANTED) {
            ActivityCompat.requestPermissions(this, arrayOf(Manifest.permission.RECORD_AUDIO), 2001)
            return
        }

        val sampleRate = 16000
        val bufSize = AudioRecord.getMinBufferSize(
            sampleRate,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT
        )

        try {
            audioRecord = AudioRecord(
                MediaRecorder.AudioSource.MIC,
                sampleRate,
                AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT,
                bufSize
            )
        } catch (e: SecurityException) {
            addChat("[!] Microphone permission denied")
            return
        }

        val buffer = java.io.ByteArrayOutputStream()
        isRecording = true
        chatMicBtn.text = "STOP"
        chatMicBtn.setTextColor(0xFFFF4444.toInt())

        audioRecord?.startRecording()
        recordThread = Thread {
            val readBuf = ByteArray(bufSize)
            while (isRecording) {
                val read = audioRecord?.read(readBuf, 0, readBuf.size) ?: break
                if (read > 0) {
                    buffer.write(readBuf, 0, read)
                }
            }
            recordedAudio = buffer.toByteArray()
        }
        recordThread?.start()
    }

    private fun stopRecording() {
        isRecording = false
        audioRecord?.stop()
        audioRecord?.release()
        audioRecord = null
        recordThread = null
        runOnUiThread {
            chatMicBtn.text = "Mic"
            chatMicBtn.setTextColor(0xFF000000.toInt())
        }
    }

    private fun stopRecordingAndSend() {
        stopRecording()
        val data = recordedAudio ?: return
        if (data.isEmpty()) {
            addChat("[!] No audio recorded")
            return
        }

        val durationMs = (data.size.toLong() * 1000) / (16000 * 2) // 16kHz, 16-bit (2 bytes per sample)
        val target = dmTarget
        val destHex = target?.nodeId

        val result = MeshBridge.meshSendVoice(destHex, data, durationMs.toInt())
        if (result == 0) {
            val destName = target?.displayName ?: "all"
            val secs = durationMs.toFloat() / 1000f
            addChat("[Voice] Sent to $destName (${String.format("%.1f", secs)}s)")
        } else {
            addChat("[!] Failed to send voice note")
        }
        recordedAudio = null
    }

    private fun playVoiceNote(note: VoiceNote) {
        Thread {
            val sampleRate = 16000
            val bufSize = AudioTrack.getMinBufferSize(
                sampleRate,
                AudioFormat.CHANNEL_OUT_MONO,
                AudioFormat.ENCODING_PCM_16BIT
            )
            val track = AudioTrack(
                AudioManager.STREAM_MUSIC,
                sampleRate,
                AudioFormat.CHANNEL_OUT_MONO,
                AudioFormat.ENCODING_PCM_16BIT,
                maxOf(bufSize, note.audioData.size),
                AudioTrack.MODE_STATIC
            )
            track.write(note.audioData, 0, note.audioData.size)
            track.play()
            // Wait for playback
            val playDurationMs = note.durationMs + 200
            Thread.sleep(playDurationMs)
            track.stop()
            track.release()
        }.start()
    }

    // --- Voice Calls ---

    private fun startCall(peer: PeerInfo) {
        if (inCall != null) {
            Toast.makeText(this, "Already in a call", Toast.LENGTH_SHORT).show()
            return
        }

        val result = MeshBridge.meshStartCall(peer.nodeId)
        if (result != 0) {
            addChat("[!] Failed to start call")
            return
        }

        inCall = peer
        callActive = true
        addChat("[Call] Calling ${peer.displayName}...")
        updateCallBanner()
        startCallAudio(peer)
    }

    private fun showIncomingCallDialog(peerId: String, peerName: String) {
        AlertDialog.Builder(this)
            .setTitle("Incoming Call")
            .setMessage("$peerName is calling you")
            .setPositiveButton("Accept") { _, _ ->
                val peer = peerEntries.find { it.nodeId == peerId }
                    ?: PeerInfo(peerId, peerName)
                inCall = peer
                callActive = true
                addChat("[Call] Accepted call from $peerName")
                updateCallBanner()
                startCallAudio(peer)
            }
            .setNegativeButton("Decline") { _, _ ->
                MeshBridge.meshEndCall()
                addChat("[Call] Declined call from $peerName")
            }
            .setCancelable(false)
            .show()
    }

    private fun startCallAudio(peer: PeerInfo) {
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
            != PackageManager.PERMISSION_GRANTED) {
            addChat("[!] Microphone permission needed for call")
            return
        }

        val sampleRate = 16000
        val inputBufSize = AudioRecord.getMinBufferSize(
            sampleRate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT
        )
        val outputBufSize = AudioTrack.getMinBufferSize(
            sampleRate, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT
        )

        // Validate buffer sizes
        if (inputBufSize <= 0 || outputBufSize <= 0) {
            addChat("[!] Audio not supported on this device")
            return
        }

        // Set audio mode and enable speakerphone
        val audioManager = getSystemService(Context.AUDIO_SERVICE) as AudioManager
        audioManager.mode = AudioManager.MODE_IN_COMMUNICATION
        audioManager.isSpeakerphoneOn = true

        // Capture thread: send 20ms frames (640 bytes = 320 samples)
        try {
            callAudioRecord = AudioRecord(
                MediaRecorder.AudioSource.VOICE_COMMUNICATION,
                sampleRate,
                AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT,
                maxOf(inputBufSize, 640)
            )
        } catch (e: Exception) {
            addChat("[!] Cannot access microphone: ${e.message}")
            audioManager.mode = AudioManager.MODE_NORMAL
            return
        }

        if (callAudioRecord?.state != AudioRecord.STATE_INITIALIZED) {
            addChat("[!] Failed to initialize audio recording")
            callAudioRecord?.release()
            callAudioRecord = null
            audioManager.mode = AudioManager.MODE_NORMAL
            return
        }

        callAudioRecord?.startRecording()
        callRecordThread = Thread {
            val frameBuf = ByteArray(640) // 20ms at 16kHz 16-bit mono
            while (callActive) {
                try {
                    val read = callAudioRecord?.read(frameBuf, 0, frameBuf.size) ?: break
                    if (read > 0) {
                        val frame = frameBuf.copyOf(read)
                        MeshBridge.meshSendAudioFrame(peer.nodeId, frame)
                    }
                } catch (_: Exception) { break }
            }
        }
        callRecordThread?.start()

        // Playback: use AudioTrack.Builder with AudioAttributes for speaker output
        try {
            val audioAttrs = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                .build()
            val audioFormat = AudioFormat.Builder()
                .setSampleRate(sampleRate)
                .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .build()
            callAudioTrack = AudioTrack.Builder()
                .setAudioAttributes(audioAttrs)
                .setAudioFormat(audioFormat)
                .setBufferSizeInBytes(maxOf(outputBufSize, 640))
                .setTransferMode(AudioTrack.MODE_STREAM)
                .build()
        } catch (e: Exception) {
            addChat("[!] Failed to create audio output: ${e.message}")
            callActive = false
            callAudioRecord?.stop()
            callAudioRecord?.release()
            callAudioRecord = null
            audioManager.mode = AudioManager.MODE_NORMAL
            return
        }

        callAudioTrack?.play()

        callPlayThread = Thread {
            val silence = ByteArray(640) // 20ms silence
            while (callActive) {
                try {
                    val frame = callPlaybackBuffer.poll(20, java.util.concurrent.TimeUnit.MILLISECONDS)
                    if (frame != null) {
                        callAudioTrack?.write(frame, 0, frame.size)
                    } else {
                        callAudioTrack?.write(silence, 0, silence.size)
                    }
                } catch (_: Exception) { break }
            }
        }
        callPlayThread?.start()

        updateCallBanner()
    }

    private fun endCall() {
        if (inCall == null) return

        callActive = false
        MeshBridge.meshEndCall()

        try { callAudioRecord?.stop() } catch (_: Exception) {}
        try { callAudioRecord?.release() } catch (_: Exception) {}
        callAudioRecord = null
        callRecordThread = null

        try { callAudioTrack?.stop() } catch (_: Exception) {}
        try { callAudioTrack?.release() } catch (_: Exception) {}
        callAudioTrack = null
        callPlayThread = null

        callPlaybackBuffer.clear()

        val audioManager = getSystemService(Context.AUDIO_SERVICE) as AudioManager
        audioManager.isSpeakerphoneOn = false
        audioManager.mode = AudioManager.MODE_NORMAL

        val peerName = inCall?.displayName ?: "?"
        inCall = null
        runOnUiThread {
            addChat("[Call] Ended with $peerName")
            updateCallBanner()
        }
    }

    // --- Connectivity / Stats parsing ---

    private fun parseStatsJson(json: String) {
        try {
            val obj = JSONObject(json)
            val activeIface = obj.optString("active_interface", "")
            val interfaces = obj.optJSONArray("interfaces")
            if (interfaces != null) {
                val sb = StringBuilder()
                for (i in 0 until interfaces.length()) {
                    val iface = interfaces.getJSONObject(i)
                    val name = iface.getString("name")
                    val type = iface.getString("type")
                    val ip = iface.getString("ip")
                    val active = iface.getBoolean("active")
                    val status = if (active) "ACTIVE" else "available"
                    sb.appendLine("[$type] $name: $ip ($status)")
                    if (active) {
                        activeInterface = "$type ($ip)"
                    }
                }
                // Update connectivity display in settings
                settingsLayout.findViewWithTag<TextView>("connectivity_info")?.text =
                    if (sb.isEmpty()) "No interfaces detected" else sb.toString()
                updateHeader()
            }
        } catch (_: Exception) {
            // Ignore parse errors for older stats format
        }
    }

    // --- State restore ---

    private fun restoreStateFromService() {
        meshService?.let { service ->
            nodeIdShort = service.getNodeId()?.take(8)
            peerCount = service.getPeerCount()
            updateHeader()

            service.getRecentEvents().forEach { event ->
                when (event.eventType) {
                    1 -> peerEntries.add(PeerInfo(event.nodeId ?: "", event.data ?: "Unknown"))
                    2 -> peerEntries.removeAll { it.nodeId == event.nodeId }
                    3 -> addChat("[${event.senderName ?: "?"}] ${event.data}")
                    10 -> {
                        gatewayName = event.data
                        peerEntries.find { it.nodeId == event.nodeId }?.isGateway = true
                    }
                }
            }
            updatePeerList()
            updateRadar()
        }
    }

    private fun requestPermissions() {
        val perms = mutableListOf<String>()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS)
                != PackageManager.PERMISSION_GRANTED
            ) {
                perms.add(Manifest.permission.POST_NOTIFICATIONS)
            }
        }
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
            != PackageManager.PERMISSION_GRANTED
        ) {
            perms.add(Manifest.permission.RECORD_AUDIO)
        }
        // Bluetooth permissions (Android 12+)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.BLUETOOTH_CONNECT)
                != PackageManager.PERMISSION_GRANTED
            ) {
                perms.add(Manifest.permission.BLUETOOTH_CONNECT)
            }
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.BLUETOOTH_SCAN)
                != PackageManager.PERMISSION_GRANTED
            ) {
                perms.add(Manifest.permission.BLUETOOTH_SCAN)
            }
        }
        // Fine location (needed for WiFi Direct and BT discovery on older Android)
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.ACCESS_FINE_LOCATION)
            != PackageManager.PERMISSION_GRANTED
        ) {
            perms.add(Manifest.permission.ACCESS_FINE_LOCATION)
        }
        if (perms.isNotEmpty()) {
            ActivityCompat.requestPermissions(this, perms.toTypedArray(), 1001)
        }
    }

    private fun formatSize(bytes: Long): String {
        return when {
            bytes < 1024 -> "$bytes B"
            bytes < 1024 * 1024 -> "${bytes / 1024} KB"
            else -> "${bytes / (1024 * 1024)} MB"
        }
    }

    // --- Group Chat ---

    private fun showJoinGroupDialog() {
        val input = EditText(this).apply {
            hint = "Group name"
            setPadding(32, 16, 32, 16)
        }
        AlertDialog.Builder(this)
            .setTitle("Join Group")
            .setView(input)
            .setPositiveButton("Join") { _, _ ->
                val name = input.text.toString().trim()
                if (name.isNotEmpty()) {
                    MeshBridge.meshJoinGroup(name)
                    if (!joinedGroups.contains(name)) joinedGroups.add(name)
                    activeGroup = name
                    chatInput.hint = "Message to #$name..."
                    addChat("[*] Joined group #$name")
                    updateGroupDisplay()
                }
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun showLeaveGroupDialog() {
        if (joinedGroups.isEmpty()) {
            Toast.makeText(this, "Not in any groups", Toast.LENGTH_SHORT).show()
            return
        }
        val names = joinedGroups.toTypedArray()
        AlertDialog.Builder(this)
            .setTitle("Leave Group")
            .setItems(names) { _, which ->
                val name = names[which]
                MeshBridge.meshLeaveGroup(name)
                joinedGroups.remove(name)
                if (activeGroup == name) {
                    activeGroup = if (joinedGroups.isNotEmpty()) joinedGroups.last() else null
                    chatInput.hint = if (activeGroup != null) "Message to #$activeGroup..." else "Type a message..."
                }
                addChat("[*] Left group #$name")
                updateGroupDisplay()
            }
            .show()
    }

    private fun updateGroupDisplay() {
        settingsLayout.findViewWithTag<TextView>("groups_info")?.text =
            if (joinedGroups.isEmpty()) "No groups joined"
            else "Groups: ${joinedGroups.joinToString(", ") { "#$it" }}"
        settingsLayout.findViewWithTag<TextView>("active_group_info")?.text =
            if (activeGroup != null) "Active: #$activeGroup (messages go here)"
            else "No active group (messages go to main chat)"
    }

    // --- Bluetooth Transport ---

    private fun enableBluetooth() {
        // Check permissions first (Android 12+)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.BLUETOOTH_CONNECT)
                != PackageManager.PERMISSION_GRANTED ||
                ContextCompat.checkSelfPermission(this, Manifest.permission.BLUETOOTH_SCAN)
                != PackageManager.PERMISSION_GRANTED
            ) {
                ActivityCompat.requestPermissions(
                    this,
                    arrayOf(Manifest.permission.BLUETOOTH_CONNECT, Manifest.permission.BLUETOOTH_SCAN),
                    1002
                )
                return
            }
        }

        val bt = BluetoothTransport(this, Build.MODEL)
        bt.setListener(this)
        bt.start()
        bluetoothTransport = bt
        addChat("[BT] Bluetooth mesh enabled")
    }

    private fun disableBluetooth() {
        bluetoothTransport?.stop()
        bluetoothTransport = null
        btPeerCount = 0
        updateHeader()
        settingsLayout.findViewWithTag<TextView>("bt_status")?.text = ""
        addChat("[BT] Bluetooth mesh disabled")
    }

    // BluetoothTransport.Listener

    override fun onBtMessageReceived(type: String, sender: String, content: String, json: JSONObject) {
        runOnUiThread {
            when (type) {
                "text" -> addChat("[BT] $sender: $content")
                "broadcast" -> addChat("[BT] $sender: $content")
                "sos" -> addChat("[BT] !!! SOS from $sender: $content")
                "checkin" -> {
                    val status = json.optString("status", "")
                    addChat("[BT] [CHECK-IN] $sender: $status $content")
                }
                else -> addChat("[BT] $sender: $content")
            }

            // Optionally forward BT messages into the TCP mesh
            if (type in listOf("broadcast", "sos", "checkin")) {
                MeshBridge.meshSendBroadcast("[BT relay] $sender: $content")
            }
        }
    }

    override fun onBtPeerConnected(address: String, name: String) {
        runOnUiThread {
            btPeerCount = bluetoothTransport?.getPeerCount() ?: 0
            meshService?.setBtPeerCount(btPeerCount)
            updateHeader()
            addChat("[BT+] $name connected")
        }
    }

    override fun onBtPeerDisconnected(address: String, name: String) {
        runOnUiThread {
            btPeerCount = bluetoothTransport?.getPeerCount() ?: 0
            meshService?.setBtPeerCount(btPeerCount)
            updateHeader()
            addChat("[BT-] $name disconnected")
        }
    }

    override fun onBtStatusChanged(status: String) {
        runOnUiThread {
            settingsLayout.findViewWithTag<TextView>("bt_status")?.text = status
        }
    }

    // --- WiFi Direct ---

    private fun enableWifiDirect() {
        val mgr = WifiDirectManager(this)
        mgr.setListener(this)
        mgr.init()
        mgr.start()
        wifiDirectManager = mgr
        registerReceiver(mgr.receiver, mgr.getIntentFilter())
        addChat("[WD] WiFi Direct enabled")
    }

    private fun disableWifiDirect() {
        wifiDirectManager?.let { mgr ->
            try { unregisterReceiver(mgr.receiver) } catch (_: Exception) {}
            mgr.stop()
        }
        wifiDirectManager = null
        wifiDirectStatus = ""
        settingsLayout.findViewWithTag<TextView>("wd_status")?.text = ""
        addChat("[WD] WiFi Direct disabled")
    }

    // WifiDirectManager.Listener

    override fun onWifiDirectStatusChanged(enabled: Boolean) {
        runOnUiThread {
            if (!enabled) {
                settingsLayout.findViewWithTag<TextView>("wd_status")?.text = "WiFi Direct: Disabled on device"
            }
        }
    }

    override fun onWifiDirectPeersFound(peers: List<WifiP2pDevice>) {
        runOnUiThread {
            if (peers.isNotEmpty()) {
                settingsLayout.findViewWithTag<TextView>("wd_status")?.text =
                    "WiFi Direct: ${peers.size} peer(s) found"
            }
        }
    }

    override fun onWifiDirectConnected(info: WifiP2pInfo) {
        runOnUiThread {
            val role = if (info.isGroupOwner) "Group Owner" else "Connected"
            wifiDirectStatus = "WiFi Direct: $role"
            settingsLayout.findViewWithTag<TextView>("wd_status")?.text = wifiDirectStatus
            addChat("[WD] $role — mesh will auto-discover over P2P interface")
        }
    }

    override fun onWifiDirectDisconnected() {
        runOnUiThread {
            wifiDirectStatus = ""
            settingsLayout.findViewWithTag<TextView>("wd_status")?.text = "WiFi Direct: Disconnected"
        }
    }

    override fun onWifiDirectStatus(status: String) {
        runOnUiThread {
            wifiDirectStatus = status
            settingsLayout.findViewWithTag<TextView>("wd_status")?.text = status
        }
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == 1002) {
            // BT permission result — retry enabling if granted
            if (grantResults.all { it == PackageManager.PERMISSION_GRANTED }) {
                enableBluetooth()
            } else {
                Toast.makeText(this, "Bluetooth permissions required", Toast.LENGTH_SHORT).show()
                settingsLayout.findViewWithTag<Switch>("bt_switch")?.isChecked = false
            }
        }
    }
}
