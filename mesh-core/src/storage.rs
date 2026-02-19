use std::path::Path;
use anyhow::{Result, Context};
use rusqlite::{Connection, params};

/// A stored chat message.
#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub id: i64,
    pub msg_id: [u8; 32],
    pub sender_id: [u8; 32],
    pub sender_name: String,
    pub content: String,
    pub msg_type: String,
    pub group_name: Option<String>,
    pub destination: Option<[u8; 32]>,
    pub timestamp: i64,
    pub is_outgoing: bool,
    pub read: bool,
    pub delivered: bool,
    pub disappear_at: Option<i64>,
    pub extra_json: Option<String>,
}

/// A saved contact.
#[derive(Debug, Clone)]
pub struct Contact {
    pub node_id: [u8; 32],
    pub display_name: String,
    pub nickname: Option<String>,
    pub bio: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub is_favorite: bool,
    pub safety_number: Option<String>,
}

impl Contact {
    /// Returns nickname if set, otherwise display_name.
    pub fn effective_name(&self) -> &str {
        self.nickname.as_deref().unwrap_or(&self.display_name)
    }
}

/// SQLite-backed persistence for messages, contacts, and groups.
pub struct MeshStorage {
    db: Connection,
}

impl MeshStorage {
    /// Open (or create) the database in the given directory.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir).context("Failed to create data directory")?;
        let db_path = data_dir.join("masskritical.db");
        let db = Connection::open(&db_path).context("Failed to open database")?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let storage = Self { db };
        storage.create_tables()?;
        Ok(storage)
    }

    fn create_tables(&self) -> Result<()> {
        self.db.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                msg_id BLOB NOT NULL,
                sender_id BLOB NOT NULL,
                sender_name TEXT NOT NULL,
                content TEXT NOT NULL,
                msg_type TEXT NOT NULL,
                group_name TEXT,
                destination BLOB,
                timestamp INTEGER NOT NULL,
                is_outgoing INTEGER NOT NULL DEFAULT 0,
                read INTEGER NOT NULL DEFAULT 0,
                delivered INTEGER NOT NULL DEFAULT 0,
                disappear_at INTEGER,
                extra_json TEXT
            );

            CREATE TABLE IF NOT EXISTS contacts (
                node_id BLOB PRIMARY KEY,
                display_name TEXT NOT NULL,
                nickname TEXT,
                bio TEXT DEFAULT '',
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                is_favorite INTEGER DEFAULT 0,
                safety_number TEXT
            );

            CREATE TABLE IF NOT EXISTS groups (
                name TEXT PRIMARY KEY,
                joined_at INTEGER NOT NULL,
                is_muted INTEGER DEFAULT 0
            );",
        )?;
        Ok(())
    }

    // --- Messages ---

    pub fn save_message(&self, msg: &StoredMessage) -> Result<i64> {
        self.db.execute(
            "INSERT INTO messages (msg_id, sender_id, sender_name, content, msg_type, group_name, destination, timestamp, is_outgoing, read, delivered, disappear_at, extra_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                msg.msg_id.as_slice(),
                msg.sender_id.as_slice(),
                msg.sender_name,
                msg.content,
                msg.msg_type,
                msg.group_name,
                msg.destination.map(|d| d.to_vec()),
                msg.timestamp,
                msg.is_outgoing as i32,
                msg.read as i32,
                msg.delivered as i32,
                msg.disappear_at,
                msg.extra_json,
            ],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    pub fn get_messages(&self, limit: usize, before_id: Option<i64>) -> Result<Vec<StoredMessage>> {
        let sql = if before_id.is_some() {
            "SELECT id, msg_id, sender_id, sender_name, content, msg_type, group_name, destination, timestamp, is_outgoing, read, delivered, disappear_at, extra_json
             FROM messages WHERE id < ?1 ORDER BY id DESC LIMIT ?2"
        } else {
            "SELECT id, msg_id, sender_id, sender_name, content, msg_type, group_name, destination, timestamp, is_outgoing, read, delivered, disappear_at, extra_json
             FROM messages ORDER BY id DESC LIMIT ?2"
        };
        let mut stmt = self.db.prepare(sql)?;
        let bid = before_id.unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![bid, limit as i64], |row| {
            Ok(Self::row_to_message(row))
        })?;
        let mut msgs: Vec<StoredMessage> = rows.filter_map(|r| r.ok()).collect();
        msgs.reverse();
        Ok(msgs)
    }

    pub fn get_dm_history(&self, peer: &[u8; 32], limit: usize) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.db.prepare(
            "SELECT id, msg_id, sender_id, sender_name, content, msg_type, group_name, destination, timestamp, is_outgoing, read, delivered, disappear_at, extra_json
             FROM messages WHERE group_name IS NULL AND (sender_id = ?1 OR destination = ?1) ORDER BY id DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![peer.as_slice(), limit as i64], |row| {
            Ok(Self::row_to_message(row))
        })?;
        let mut msgs: Vec<StoredMessage> = rows.filter_map(|r| r.ok()).collect();
        msgs.reverse();
        Ok(msgs)
    }

    pub fn get_group_history(&self, group: &str, limit: usize) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.db.prepare(
            "SELECT id, msg_id, sender_id, sender_name, content, msg_type, group_name, destination, timestamp, is_outgoing, read, delivered, disappear_at, extra_json
             FROM messages WHERE group_name = ?1 ORDER BY id DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![group, limit as i64], |row| {
            Ok(Self::row_to_message(row))
        })?;
        let mut msgs: Vec<StoredMessage> = rows.filter_map(|r| r.ok()).collect();
        msgs.reverse();
        Ok(msgs)
    }

    pub fn mark_read(&self, msg_id: &[u8; 32]) -> Result<()> {
        self.db.execute("UPDATE messages SET read = 1 WHERE msg_id = ?1", params![msg_id.as_slice()])?;
        Ok(())
    }

    pub fn mark_delivered(&self, msg_id: &[u8; 32]) -> Result<()> {
        self.db.execute("UPDATE messages SET delivered = 1 WHERE msg_id = ?1", params![msg_id.as_slice()])?;
        Ok(())
    }

    pub fn delete_expired(&self) -> Result<u32> {
        let now = chrono::Utc::now().timestamp_millis();
        let count = self.db.execute(
            "DELETE FROM messages WHERE disappear_at IS NOT NULL AND disappear_at <= ?1",
            params![now],
        )?;
        Ok(count as u32)
    }

    fn row_to_message(row: &rusqlite::Row) -> StoredMessage {
        let msg_id_blob: Vec<u8> = row.get(1).unwrap_or_default();
        let sender_id_blob: Vec<u8> = row.get(2).unwrap_or_default();
        let dest_blob: Option<Vec<u8>> = row.get(7).unwrap_or(None);

        let mut msg_id = [0u8; 32];
        if msg_id_blob.len() == 32 { msg_id.copy_from_slice(&msg_id_blob); }
        let mut sender_id = [0u8; 32];
        if sender_id_blob.len() == 32 { sender_id.copy_from_slice(&sender_id_blob); }
        let destination = dest_blob.and_then(|b| {
            if b.len() == 32 { let mut a = [0u8; 32]; a.copy_from_slice(&b); Some(a) } else { None }
        });

        StoredMessage {
            id: row.get(0).unwrap_or(0),
            msg_id,
            sender_id,
            sender_name: row.get(3).unwrap_or_default(),
            content: row.get(4).unwrap_or_default(),
            msg_type: row.get(5).unwrap_or_default(),
            group_name: row.get(6).unwrap_or(None),
            destination,
            timestamp: row.get(8).unwrap_or(0),
            is_outgoing: row.get::<_, i32>(9).unwrap_or(0) != 0,
            read: row.get::<_, i32>(10).unwrap_or(0) != 0,
            delivered: row.get::<_, i32>(11).unwrap_or(0) != 0,
            disappear_at: row.get(12).unwrap_or(None),
            extra_json: row.get(13).unwrap_or(None),
        }
    }

    // --- Contacts ---

    pub fn save_contact(&self, contact: &Contact) -> Result<()> {
        self.db.execute(
            "INSERT INTO contacts (node_id, display_name, nickname, bio, first_seen, last_seen, is_favorite, safety_number)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(node_id) DO UPDATE SET display_name=?2, bio=?4, last_seen=?6",
            params![
                contact.node_id.as_slice(),
                contact.display_name,
                contact.nickname,
                contact.bio,
                contact.first_seen,
                contact.last_seen,
                contact.is_favorite as i32,
                contact.safety_number,
            ],
        )?;
        Ok(())
    }

    pub fn get_contacts(&self) -> Result<Vec<Contact>> {
        let mut stmt = self.db.prepare(
            "SELECT node_id, display_name, nickname, bio, first_seen, last_seen, is_favorite, safety_number FROM contacts ORDER BY last_seen DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            let node_id_blob: Vec<u8> = row.get(0)?;
            let mut node_id = [0u8; 32];
            if node_id_blob.len() == 32 { node_id.copy_from_slice(&node_id_blob); }
            Ok(Contact {
                node_id,
                display_name: row.get(1)?,
                nickname: row.get(2)?,
                bio: row.get(3).unwrap_or_default(),
                first_seen: row.get(4)?,
                last_seen: row.get(5)?,
                is_favorite: row.get::<_, i32>(6).unwrap_or(0) != 0,
                safety_number: row.get(7)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn set_nickname(&self, node_id: &[u8; 32], nickname: &str) -> Result<()> {
        self.db.execute(
            "UPDATE contacts SET nickname = ?1 WHERE node_id = ?2",
            params![nickname, node_id.as_slice()],
        )?;
        Ok(())
    }

    pub fn get_contact(&self, node_id: &[u8; 32]) -> Result<Option<Contact>> {
        let mut stmt = self.db.prepare(
            "SELECT node_id, display_name, nickname, bio, first_seen, last_seen, is_favorite, safety_number FROM contacts WHERE node_id = ?1"
        )?;
        let mut rows = stmt.query_map(params![node_id.as_slice()], |row| {
            let node_id_blob: Vec<u8> = row.get(0)?;
            let mut nid = [0u8; 32];
            if node_id_blob.len() == 32 { nid.copy_from_slice(&node_id_blob); }
            Ok(Contact {
                node_id: nid,
                display_name: row.get(1)?,
                nickname: row.get(2)?,
                bio: row.get(3).unwrap_or_default(),
                first_seen: row.get(4)?,
                last_seen: row.get(5)?,
                is_favorite: row.get::<_, i32>(6).unwrap_or(0) != 0,
                safety_number: row.get(7)?,
            })
        })?;
        Ok(rows.next().and_then(|r| r.ok()))
    }

    // --- Groups ---

    pub fn join_group(&self, name: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        self.db.execute(
            "INSERT OR IGNORE INTO groups (name, joined_at) VALUES (?1, ?2)",
            params![name, now],
        )?;
        Ok(())
    }

    pub fn leave_group(&self, name: &str) -> Result<()> {
        self.db.execute("DELETE FROM groups WHERE name = ?1", params![name])?;
        Ok(())
    }

    pub fn get_groups(&self) -> Result<Vec<String>> {
        let mut stmt = self.db.prepare("SELECT name FROM groups ORDER BY joined_at")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn is_in_group(&self, name: &str) -> Result<bool> {
        let count: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM groups WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_storage() -> MeshStorage {
        let dir = std::env::temp_dir().join(format!("mesh_storage_test_{}", rand::random::<u32>()));
        MeshStorage::open(&dir).unwrap()
    }

    #[test]
    fn test_save_and_get_message() {
        let storage = temp_storage();
        let msg = StoredMessage {
            id: 0,
            msg_id: [1u8; 32],
            sender_id: [2u8; 32],
            sender_name: "Alice".into(),
            content: "Hello".into(),
            msg_type: "text".into(),
            group_name: None,
            destination: None,
            timestamp: 1000,
            is_outgoing: false,
            read: false,
            delivered: false,
            disappear_at: None,
            extra_json: None,
        };
        let id = storage.save_message(&msg).unwrap();
        assert!(id > 0);

        let msgs = storage.get_messages(10, None).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "Hello");
    }

    #[test]
    fn test_contacts() {
        let storage = temp_storage();
        let contact = Contact {
            node_id: [3u8; 32],
            display_name: "Bob".into(),
            nickname: None,
            bio: "hi".into(),
            first_seen: 1000,
            last_seen: 2000,
            is_favorite: false,
            safety_number: None,
        };
        storage.save_contact(&contact).unwrap();
        let contacts = storage.get_contacts().unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].display_name, "Bob");

        storage.set_nickname(&[3u8; 32], "Bobby").unwrap();
        let c = storage.get_contact(&[3u8; 32]).unwrap().unwrap();
        assert_eq!(c.nickname.as_deref(), Some("Bobby"));
        assert_eq!(c.effective_name(), "Bobby");
    }

    #[test]
    fn test_groups() {
        let storage = temp_storage();
        storage.join_group("rescue-team").unwrap();
        assert!(storage.is_in_group("rescue-team").unwrap());
        let groups = storage.get_groups().unwrap();
        assert_eq!(groups, vec!["rescue-team"]);

        storage.leave_group("rescue-team").unwrap();
        assert!(!storage.is_in_group("rescue-team").unwrap());
    }
}
