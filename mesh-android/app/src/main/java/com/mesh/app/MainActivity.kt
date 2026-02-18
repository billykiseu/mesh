package com.mesh.app

import android.Manifest
import android.content.*
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import android.provider.OpenableColumns
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.*
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.localbroadcastmanager.content.LocalBroadcastManager

class MainActivity : AppCompatActivity() {

    // --- Service binding ---
    private var meshService: MeshService? = null
    private var serviceBound = false

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
    private lateinit var contentFrame: FrameLayout

    // Tab buttons
    private lateinit var tabRadar: TextView
    private lateinit var tabChat: TextView
    private lateinit var tabPeers: TextView
    private lateinit var tabSettings: TextView

    // Chat tab views
    private lateinit var chatListView: ListView
    private lateinit var chatInput: EditText
    private lateinit var chatSendBtn: Button
    private lateinit var chatAttachBtn: Button

    // Peers tab views
    private lateinit var peersListView: ListView

    // Radar tab views
    private lateinit var radarText: TextView
    private lateinit var radarPeerCount: TextView

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

    data class PeerInfo(val nodeId: String, val displayName: String, var isGateway: Boolean = false, var bio: String = "")

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
                        peerEntries.removeAll { it.nodeId == nodeId }
                        updatePeerList()
                        updateHeader()
                        addChat("[-] ${nodeId?.take(8)} disconnected")
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
                        addChat("[Voice] ${senderName}: ${String.format("%.1f", secs)}s note")
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
                    12 -> { // SOS received
                        addChat("!!! SOS from ${senderName}: $data")
                    }
                    13 -> { // Incoming call
                        addChat("[Call] Incoming from $data")
                    }
                    15 -> { // Call ended
                        addChat("[Call] Ended")
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
                        updateHeader()
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
    }

    override fun onPause() {
        super.onPause()
        LocalBroadcastManager.getInstance(this).unregisterReceiver(meshEventReceiver)
    }

    override fun onStop() {
        super.onStop()
        if (serviceBound) {
            unbindService(serviceConnection)
            serviceBound = false
        }
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
        }

        headerStatus = TextView(this).apply {
            text = "Mesh"
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
        tabSettings = makeTabButton("Settings") { switchTab("settings") }

        tabBar.addView(tabRadar)
        tabBar.addView(tabChat)
        tabBar.addView(tabPeers)
        tabBar.addView(tabSettings)

        root.addView(tabBar)

        // Build tab content views
        buildChatView()
        buildPeersView()
        buildRadarView()
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
        }
        chatView.addView(chatListView)

        val inputRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setPadding(0, 8, 0, 0)
        }

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
            text = "Scanning for mesh nodes..."
            textSize = 14f
            gravity = Gravity.CENTER
            setLineSpacing(4f, 1.3f)
        }
        radarView.addView(radarText)

        val startBtn = Button(this).apply {
            text = "Start Mesh Node"
            setOnClickListener { startMeshService() }
            setPadding(32, 16, 32, 16)
        }
        radarView.addView(startBtn)
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
        val nameBtn = Button(this).apply {
            text = "Update"
            setOnClickListener {
                val name = nameInput.text.toString().trim()
                if (name.isNotEmpty()) {
                    MeshBridge.meshUpdateProfile(name, "")
                    Toast.makeText(this@MainActivity, "Profile updated", Toast.LENGTH_SHORT).show()
                }
            }
        }
        nameRow.addView(nameBtn)
        settingsLayout.addView(nameRow)

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

        addSettingsSection("Safety")

        val nukeBtn = Button(this).apply {
            text = "NUKE - Destroy All Data"
            setBackgroundColor(0xFFD32F2F.toInt())
            setTextColor(0xFFFFFFFF.toInt())
            setOnClickListener { confirmNuke() }
        }
        settingsLayout.addView(nukeBtn)

        addSettingsSection("About")

        val aboutText = TextView(this).apply {
            text = "Mesh Network v0.2.0\n" +
                    "Peer-to-peer mesh networking\n" +
                    "Text, Voice, Files, PTT\n" +
                    "End-to-end encrypted"
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

        // Detach views from their current parent before adding to contentFrame
        listOf(tabRadar, tabChat, tabPeers, tabSettings).forEach {
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
            "settings" -> {
                detachFromParent(settingsLayout)
                contentFrame.addView(settingsLayout)
                tabSettings.setTextColor(0xFF00D4FF.toInt())
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

    private fun sendMessage() {
        val text = chatInput.text.toString().trim()
        if (text.isEmpty()) return

        val target = dmTarget
        val result = if (target != null) {
            MeshBridge.meshSendDirect(target.nodeId, text)
        } else {
            MeshBridge.meshSendBroadcast(text)
        }

        if (result == 0) {
            val prefix = if (target != null) "[DM to ${target.displayName}]" else "[You]"
            addChat("$prefix $text")
            chatInput.text.clear()
        } else {
            addChat("[!] Failed to send message")
        }
    }

    private fun addChat(msg: String) {
        chatMessages.add(msg)
        chatAdapter.notifyDataSetChanged()
        chatListView.setSelection(chatMessages.size - 1)
    }

    private fun updateHeader() {
        val status = if (meshService?.isNodeRunning() == true) {
            "Mesh (${nodeIdShort ?: "..."})"
        } else {
            "Mesh - Stopped"
        }
        headerStatus.text = status
        headerPeers.text = "Peers: $peerCount"
        headerGateway.text = if (gatewayName != null) "GW: $gatewayName" else ""
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
        val status = if (meshService?.isNodeRunning() == true) {
            val lines = peerEntries.joinToString("\n") { p ->
                val gw = if (p.isGateway) " [Gateway]" else ""
                "  ${p.displayName}$gw"
            }
            if (lines.isEmpty()) "No peers found yet.\nMake sure another node is running nearby."
            else "Connected nodes:\n$lines"
        } else {
            "Tap 'Start Mesh Node' to begin"
        }
        radarText.text = status
    }

    private fun showPeerActions(peer: PeerInfo) {
        val items = arrayOf(
            "Send Direct Message",
            "Send File",
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
                        filePicker.launch(arrayOf("*/*"))
                    }
                    2 -> {
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
}
