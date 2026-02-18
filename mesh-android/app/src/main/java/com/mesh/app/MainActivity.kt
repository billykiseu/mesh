package com.mesh.app

import android.Manifest
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.widget.*
import androidx.appcompat.app.AppCompatActivity
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

class MainActivity : AppCompatActivity() {

    private lateinit var statusText: TextView
    private lateinit var peerCountText: TextView
    private lateinit var messageList: ListView
    private lateinit var messageInput: EditText
    private lateinit var sendButton: Button
    private lateinit var startButton: Button

    private val messages = mutableListOf<String>()
    private lateinit var messageAdapter: ArrayAdapter<String>
    private var peerCount = 0

    private val meshEventReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            val action = intent?.getStringExtra("action") ?: return
            val nodeId = intent.getStringExtra("node_id")
            val data = intent.getStringExtra("data")
            val senderName = intent.getStringExtra("sender_name")

            runOnUiThread {
                when (action) {
                    "started" -> {
                        statusText.text = "Running (${nodeId?.take(8)})"
                        startButton.isEnabled = false
                    }
                    "peer_connected" -> {
                        peerCount++
                        peerCountText.text = "Peers: $peerCount"
                        addMessage("[+] ${data ?: "Unknown"} connected")
                    }
                    "peer_disconnected" -> {
                        peerCount = maxOf(0, peerCount - 1)
                        peerCountText.text = "Peers: $peerCount"
                        addMessage("[-] ${nodeId?.take(8)} disconnected")
                    }
                    "message_received" -> {
                        addMessage("[${senderName ?: "?"}] $data")
                    }
                }
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Build UI programmatically (minimal, no XML layout needed)
        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(16, 16, 16, 16)
        }

        statusText = TextView(this).apply {
            text = "Status: Stopped"
            textSize = 16f
        }
        layout.addView(statusText)

        peerCountText = TextView(this).apply {
            text = "Peers: 0"
            textSize = 14f
        }
        layout.addView(peerCountText)

        startButton = Button(this).apply {
            text = "Start Mesh Node"
            setOnClickListener { startMeshService() }
        }
        layout.addView(startButton)

        messageAdapter = ArrayAdapter(this, android.R.layout.simple_list_item_1, messages)
        messageList = ListView(this).apply {
            adapter = messageAdapter
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                0, 1f
            )
        }
        layout.addView(messageList)

        val inputLayout = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
        }

        messageInput = EditText(this).apply {
            hint = "Type a message..."
            layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
        }
        inputLayout.addView(messageInput)

        sendButton = Button(this).apply {
            text = "Send"
            setOnClickListener { sendMessage() }
        }
        inputLayout.addView(sendButton)

        layout.addView(inputLayout)
        setContentView(layout)

        requestPermissions()
    }

    override fun onResume() {
        super.onResume()
        val filter = IntentFilter("com.mesh.app.MESH_EVENT")
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(meshEventReceiver, filter, RECEIVER_NOT_EXPORTED)
        } else {
            registerReceiver(meshEventReceiver, filter)
        }
    }

    override fun onPause() {
        super.onPause()
        unregisterReceiver(meshEventReceiver)
    }

    private fun startMeshService() {
        val intent = Intent(this, MeshService::class.java).apply {
            putExtra("name", Build.MODEL)
            putExtra("port", 7332)
        }
        ContextCompat.startForegroundService(this, intent)
        statusText.text = "Status: Starting..."
    }

    private fun sendMessage() {
        val text = messageInput.text.toString().trim()
        if (text.isEmpty()) return

        val result = MeshBridge.meshSendBroadcast(text)
        if (result == 0) {
            addMessage("[You] $text")
            messageInput.text.clear()
        } else {
            addMessage("[!] Failed to send message")
        }
    }

    private fun addMessage(msg: String) {
        messages.add(msg)
        messageAdapter.notifyDataSetChanged()
        messageList.setSelection(messages.size - 1)
    }

    private fun requestPermissions() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS)
                != PackageManager.PERMISSION_GRANTED) {
                ActivityCompat.requestPermissions(
                    this,
                    arrayOf(Manifest.permission.POST_NOTIFICATIONS),
                    1001
                )
            }
        }
    }
}
