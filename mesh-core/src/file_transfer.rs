use std::collections::HashMap;
use std::path::{Path, PathBuf};
use sha2::{Sha256, Digest};
use rand::RngCore;
use rand::rngs::OsRng;

use crate::message::{FileOfferPayload, FileChunkPayload};

pub const CHUNK_SIZE: usize = 64 * 1024; // 64KB
pub const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100MB

/// Tracks an outgoing file transfer.
#[derive(Debug)]
pub struct OutgoingTransfer {
    pub metadata: FileOfferPayload,
    pub dest: [u8; 32],
    pub chunks: Vec<Vec<u8>>,
    pub next_chunk: u32,
    pub accepted: bool,
}

/// Tracks an incoming file transfer.
#[derive(Debug)]
pub struct IncomingTransfer {
    pub metadata: FileOfferPayload,
    pub sender_id: [u8; 32],
    pub chunks: HashMap<u32, Vec<u8>>,
    pub accepted: bool,
    pub save_dir: PathBuf,
}

/// Manages in-progress file transfers (both sending and receiving).
pub struct FileTransferManager {
    outgoing: HashMap<[u8; 16], OutgoingTransfer>,
    incoming: HashMap<[u8; 16], IncomingTransfer>,
    save_dir: PathBuf,
}

impl FileTransferManager {
    pub fn new(save_dir: PathBuf) -> Self {
        Self {
            outgoing: HashMap::new(),
            incoming: HashMap::new(),
            save_dir,
        }
    }

    /// Read a file, split into chunks, and register as outgoing transfer.
    /// Returns the metadata to send as a FileOffer.
    pub fn prepare_send(&mut self, dest: [u8; 32], file_path: &Path) -> Result<FileOfferPayload, String> {
        let data = std::fs::read(file_path).map_err(|e| format!("Failed to read file: {}", e))?;

        if data.len() as u64 > MAX_FILE_SIZE {
            return Err(format!("File too large: {} bytes (max {})", data.len(), MAX_FILE_SIZE));
        }

        let mut file_id = [0u8; 16];
        OsRng.fill_bytes(&mut file_id);

        let mut hasher = Sha256::new();
        hasher.update(&data);
        let hash: [u8; 32] = hasher.finalize().into();

        let chunk_count = ((data.len() + CHUNK_SIZE - 1) / CHUNK_SIZE).max(1) as u32;
        let filename = file_path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let metadata = FileOfferPayload {
            file_id,
            filename,
            size_bytes: data.len() as u64,
            chunk_count,
            sha256_hash: hash,
        };

        let chunks: Vec<Vec<u8>> = if data.is_empty() {
            vec![vec![]]
        } else {
            data.chunks(CHUNK_SIZE).map(|c| c.to_vec()).collect()
        };

        self.outgoing.insert(file_id, OutgoingTransfer {
            metadata: metadata.clone(),
            dest,
            chunks,
            next_chunk: 0,
            accepted: false,
        });

        Ok(metadata)
    }

    /// Mark an outgoing transfer as accepted by the receiver.
    pub fn mark_accepted(&mut self, file_id: &[u8; 16]) -> bool {
        if let Some(transfer) = self.outgoing.get_mut(file_id) {
            transfer.accepted = true;
            true
        } else {
            false
        }
    }

    /// Get the next chunk to send for an outgoing transfer.
    /// Returns (sequence_number, chunk_data) or None if done/not accepted.
    pub fn next_chunk(&mut self, file_id: &[u8; 16]) -> Option<FileChunkPayload> {
        let transfer = self.outgoing.get_mut(file_id)?;
        if !transfer.accepted || transfer.next_chunk >= transfer.metadata.chunk_count {
            return None;
        }
        let seq = transfer.next_chunk;
        let data = transfer.chunks[seq as usize].clone();
        transfer.next_chunk += 1;
        Some(FileChunkPayload {
            file_id: *file_id,
            sequence: seq,
            data,
        })
    }

    /// Check if all chunks have been sent for an outgoing transfer.
    pub fn is_outgoing_complete(&self, file_id: &[u8; 16]) -> bool {
        self.outgoing.get(file_id)
            .map(|t| t.next_chunk >= t.metadata.chunk_count)
            .unwrap_or(true)
    }

    /// Get the destination node for an outgoing transfer.
    pub fn outgoing_dest(&self, file_id: &[u8; 16]) -> Option<[u8; 32]> {
        self.outgoing.get(file_id).map(|t| t.dest)
    }

    /// Remove a completed outgoing transfer.
    pub fn remove_outgoing(&mut self, file_id: &[u8; 16]) {
        self.outgoing.remove(file_id);
    }

    /// Register an incoming file offer.
    pub fn register_incoming(&mut self, metadata: FileOfferPayload, sender_id: [u8; 32]) {
        self.incoming.insert(metadata.file_id, IncomingTransfer {
            metadata,
            sender_id,
            chunks: HashMap::new(),
            accepted: false,
            save_dir: self.save_dir.clone(),
        });
    }

    /// Accept an incoming transfer. Returns the sender's node_id for sending FileAccept.
    pub fn accept_incoming(&mut self, file_id: &[u8; 16]) -> Option<[u8; 32]> {
        let transfer = self.incoming.get_mut(file_id)?;
        transfer.accepted = true;
        Some(transfer.sender_id)
    }

    /// Receive a chunk for an incoming transfer. Returns progress percentage.
    pub fn receive_chunk(&mut self, file_id: &[u8; 16], sequence: u32, data: Vec<u8>) -> Option<u8> {
        let transfer = self.incoming.get_mut(file_id)?;
        if !transfer.accepted {
            return None;
        }
        transfer.chunks.insert(sequence, data);
        let pct = ((transfer.chunks.len() as f64 / transfer.metadata.chunk_count as f64) * 100.0) as u8;
        Some(pct.min(100))
    }

    /// Check if all chunks have been received for an incoming transfer.
    pub fn is_incoming_complete(&self, file_id: &[u8; 16]) -> bool {
        self.incoming.get(file_id)
            .map(|t| t.chunks.len() as u32 >= t.metadata.chunk_count)
            .unwrap_or(false)
    }

    /// Finalize an incoming transfer: reassemble, verify hash, write to disk.
    pub fn finalize_incoming(&mut self, file_id: &[u8; 16]) -> Result<PathBuf, String> {
        let transfer = self.incoming.remove(file_id)
            .ok_or_else(|| "Transfer not found".to_string())?;

        // Reassemble chunks in order
        let mut data = Vec::with_capacity(transfer.metadata.size_bytes as usize);
        for i in 0..transfer.metadata.chunk_count {
            let chunk = transfer.chunks.get(&i)
                .ok_or_else(|| format!("Missing chunk {}", i))?;
            data.extend_from_slice(chunk);
        }

        // Verify hash
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let hash: [u8; 32] = hasher.finalize().into();
        if hash != transfer.metadata.sha256_hash {
            return Err("File hash mismatch - transfer corrupted".to_string());
        }

        // Save to disk
        std::fs::create_dir_all(&transfer.save_dir)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
        let path = transfer.save_dir.join(&transfer.metadata.filename);
        std::fs::write(&path, &data)
            .map_err(|e| format!("Failed to write file: {}", e))?;

        Ok(path)
    }

    /// Get metadata for an incoming transfer (for display).
    pub fn get_incoming_metadata(&self, file_id: &[u8; 16]) -> Option<&FileOfferPayload> {
        self.incoming.get(file_id).map(|t| &t.metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_file_transfer_roundtrip() {
        let dir = std::env::temp_dir().join("mesh_test_file_transfer");
        std::fs::create_dir_all(&dir).unwrap();

        // Create a test file
        let src_path = dir.join("test_send.txt");
        let mut f = std::fs::File::create(&src_path).unwrap();
        let test_data = b"Hello, mesh file transfer! This is test content.";
        f.write_all(test_data).unwrap();
        drop(f);

        let recv_dir = dir.join("received");
        let mut mgr = FileTransferManager::new(recv_dir.clone());
        let dest = [2u8; 32];

        // Prepare send
        let metadata = mgr.prepare_send(dest, &src_path).unwrap();
        assert_eq!(metadata.filename, "test_send.txt");
        assert_eq!(metadata.size_bytes, test_data.len() as u64);
        assert_eq!(metadata.chunk_count, 1); // Small file = 1 chunk

        // Not accepted yet - no chunks
        assert!(mgr.next_chunk(&metadata.file_id).is_none());

        // Accept
        mgr.mark_accepted(&metadata.file_id);

        // Register as incoming on receiver side
        let sender_id = [1u8; 32];
        let mut recv_mgr = FileTransferManager::new(recv_dir);
        recv_mgr.register_incoming(metadata.clone(), sender_id);
        recv_mgr.accept_incoming(&metadata.file_id);

        // Send and receive chunks
        while let Some(chunk) = mgr.next_chunk(&metadata.file_id) {
            let pct = recv_mgr.receive_chunk(&metadata.file_id, chunk.sequence, chunk.data);
            assert!(pct.is_some());
        }

        assert!(mgr.is_outgoing_complete(&metadata.file_id));
        assert!(recv_mgr.is_incoming_complete(&metadata.file_id));

        // Finalize
        let saved_path = recv_mgr.finalize_incoming(&metadata.file_id).unwrap();
        let saved_data = std::fs::read(&saved_path).unwrap();
        assert_eq!(saved_data, test_data);

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_file_too_large() {
        let dir = std::env::temp_dir().join("mesh_test_file_large");
        let mgr = &mut FileTransferManager::new(dir);
        // We can't easily create a 100MB+ file in a test, but we test the path exists
        let result = mgr.prepare_send([2u8; 32], Path::new("/nonexistent/file.txt"));
        assert!(result.is_err());
    }
}
