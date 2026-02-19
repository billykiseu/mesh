package com.mesh.app

/**
 * JNI bridge to the Rust mesh-ffi library.
 * All native functions map to the extern "C" functions in mesh-ffi/src/lib.rs.
 */
object MeshBridge {

    init {
        System.loadLibrary("mesh_ffi")
    }

    // --- Core ---
    external fun meshInit(name: String, listenPort: Int, dataDir: String): Int
    external fun meshSendBroadcast(text: String): Int
    external fun meshSendDirect(destHex: String, text: String): Int
    external fun meshGetNodeId(): String?
    external fun meshGetNodeIdShort(): String?

    // --- File Transfer ---
    external fun meshSendFile(destHex: String, filePath: String): Int
    external fun meshAcceptFile(fileIdHex: String): Int

    // --- Voice ---
    external fun meshSendVoice(destHex: String?, audioData: ByteArray, durationMs: Int): Int

    // --- PTT / Calls ---
    external fun meshStartCall(peerHex: String): Int
    external fun meshEndCall(): Int
    external fun meshSendAudioFrame(peerHex: String, data: ByteArray): Int

    // --- Public Broadcast / SOS ---
    external fun meshSendPublicBroadcast(text: String): Int
    external fun meshSendSOS(text: String, lat: Double, lon: Double): Int

    // --- Profile ---
    external fun meshUpdateProfile(name: String, bio: String): Int

    // --- Admin ---
    external fun meshNuke(): Int
    external fun meshStop(): Int
    external fun meshGetStats(): Int
    external fun meshGetPeersList(): Int

    /**
     * Event types from the native mesh engine.
     * Event type codes:
     *   0=none, 1=peer_connected, 2=peer_disconnected, 3=message_received, 4=started,
     *   5=file_offered, 6=file_progress, 7=file_complete, 8=voice_received,
     *   9=profile_updated, 10=gateway_found, 11=stats, 12=sos_received,
     *   13=call_incoming, 14=audio_frame, 15=call_ended, 16=peer_list,
     *   17=public_broadcast, 18=gateway_lost, 19=nuked, 20=stopped
     */
    data class MeshEvent(
        val eventType: Int,
        val nodeId: String?,
        val data: String?,
        val senderName: String?,
        val extra: String?,
        val value: Long,
        val float1: Double,
        val float2: Double,
        val binaryData: ByteArray?
    ) {
        val isFileOffered get() = eventType == 5
        val isFileProgress get() = eventType == 6
        val isFileComplete get() = eventType == 7
        val isVoiceReceived get() = eventType == 8
        val isSOSReceived get() = eventType == 12
        val isPublicBroadcast get() = eventType == 17
        val isCallIncoming get() = eventType == 13
        val isCallEnded get() = eventType == 15
        val fileIdHex get() = extra
        val filename get() = data
        val fileSize get() = value
        val progressPct get() = value.toInt()
        val durationMs get() = value.toInt()
        val latitude get() = float1
        val longitude get() = float2
    }

    // --- Read Receipts / Typing ---
    external fun meshSendReadReceipt(destHex: String, msgIdHex: String): Int
    external fun meshSendTypingStart(destHex: String?): Int
    external fun meshSendTypingStop(destHex: String?): Int

    // --- Groups ---
    external fun meshJoinGroup(groupName: String): Int
    external fun meshLeaveGroup(groupName: String): Int
    external fun meshSendGroupMessage(groupName: String, text: String): Int

    // --- Triage / Resources / Check-In ---
    external fun meshSendTriage(level: Int, victimId: String, notes: String, lat: Double, lon: Double): Int
    external fun meshSendResourceRequest(category: String, description: String, urgency: Int, lat: Double, lon: Double, quantity: Int): Int
    external fun meshSendCheckIn(status: String, lat: Double, lon: Double, message: String): Int

    // --- Disappearing Messages / History ---
    external fun meshSendDisappearing(destHex: String?, text: String, ttlSeconds: Int): Int
    external fun meshLoadHistory(peerHex: String?, groupName: String?): Int

    external fun meshPollEvent(): MeshEvent?
}
