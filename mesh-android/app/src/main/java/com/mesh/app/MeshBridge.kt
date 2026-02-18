package com.mesh.app

/**
 * JNI bridge to the Rust mesh-ffi library.
 * All native functions map to the extern "C" functions in mesh-ffi/src/lib.rs.
 */
object MeshBridge {

    init {
        System.loadLibrary("mesh_ffi")
    }

    /**
     * Initialize and start the mesh node.
     * @param name Display name for this node
     * @param listenPort TCP port to listen on
     * @param dataDir Directory for storing identity keys
     * @return 0 on success, -1 on error
     */
    external fun meshInit(name: String, listenPort: Int, dataDir: String): Int

    /**
     * Send a broadcast text message to all peers.
     * @param text Message content
     * @return 0 on success, -1 on error
     */
    external fun meshSendBroadcast(text: String): Int

    /**
     * Send a direct message to a specific node.
     * @param destHex 64-character hex node ID
     * @param text Message content
     * @return 0 on success, -1 on error
     */
    external fun meshSendDirect(destHex: String, text: String): Int

    /**
     * Get the local node ID as a hex string.
     * @return Node ID hex string, or null if not initialized
     */
    external fun meshGetNodeId(): String?

    /**
     * Get the short node ID (first 8 hex chars).
     * @return Short node ID, or null if not initialized
     */
    external fun meshGetNodeIdShort(): String?

    /**
     * Event types from the native mesh engine.
     */
    data class MeshEvent(
        val eventType: Int,  // 0=none, 1=peer_connected, 2=peer_disconnected, 3=message, 4=started
        val nodeId: String?,
        val data: String?,
        val senderName: String?
    )

    /**
     * Poll for the next event (non-blocking).
     * @return Event or null if no events pending
     */
    external fun meshPollEvent(): MeshEvent?
}
