//! SQLite storage for conversations, distilled messages, and embeddings.
//! Supports style learning and semantic search for contextual reply generation.

use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::Mutex;

/// A single conversation record (raw OCR + LLM reply)
#[derive(Debug, Clone)]
pub struct Conversation {
    pub id: i64,
    pub app_name: String,
    pub ocr_text: String,
    pub reply: String,
    pub accepted: bool,
    pub distilled: bool,
    pub created_at: String,
}

/// A distilled message extracted from a conversation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DistilledMessage {
    pub id: i64,
    pub conversation_id: i64,
    pub app_name: String,
    pub sender: String,     // "user" or contact name
    pub content: String,    // the actual message text
    pub is_user: bool,      // whether this is the user's own message
    pub created_at: String,
}

/// Thread-safe storage handle
pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    /// Open (or create) the database
    pub fn open() -> Result<Self, String> {
        let db_path = Self::db_path();

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create data dir: {}", e))?;
        }

        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=3000;")
            .map_err(|e| format!("Failed to set pragmas: {}", e))?;

        let storage = Self { conn: Mutex::new(conn) };
        storage.migrate()?;
        storage.cleanup_old_records()?;

        log::info!("Storage opened: {:?}", db_path);
        Ok(storage)
    }

    fn db_path() -> PathBuf {
        let base = dirs::data_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Library/Application Support"));
        base.join("Compleo").join("compleo.db")
    }

    /// Create/migrate tables
    fn migrate(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();

        // Create base table without the distilled column first (for existing v0.1 databases)
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                app_name    TEXT NOT NULL,
                ocr_text    TEXT NOT NULL,
                reply       TEXT NOT NULL DEFAULT '',
                accepted    INTEGER NOT NULL DEFAULT 0,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        ).map_err(|e| format!("Create conversations failed: {}", e))?;

        // Add 'distilled' column if upgrading from v0.1 (silently ignore if already exists)
        let _ = conn.execute_batch(
            "ALTER TABLE conversations ADD COLUMN distilled INTEGER NOT NULL DEFAULT 0;"
        );

        // Create new tables for v0.2
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS distilled_messages (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id INTEGER NOT NULL REFERENCES conversations(id),
                app_name        TEXT NOT NULL,
                sender          TEXT NOT NULL,
                content         TEXT NOT NULL,
                is_user         INTEGER NOT NULL DEFAULT 0,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS embeddings (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                message_id  INTEGER NOT NULL REFERENCES distilled_messages(id),
                app_name    TEXT NOT NULL,
                content     TEXT NOT NULL,
                vector      BLOB NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_distilled_app ON distilled_messages(app_name);
            CREATE INDEX IF NOT EXISTS idx_embeddings_app ON embeddings(app_name);
            CREATE INDEX IF NOT EXISTS idx_conv_distilled ON conversations(distilled);
            "
        ).map_err(|e| format!("Migration v0.2 failed: {}", e))?;

        Ok(())
    }

    // ========== Conversations ==========

    /// Save a new conversation record. Returns the row id.
    pub fn save_conversation(
        &self,
        app_name: &str,
        ocr_text: &str,
        reply: &str,
    ) -> Result<i64, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conversations (app_name, ocr_text, reply) VALUES (?1, ?2, ?3)",
            params![app_name, ocr_text, reply],
        ).map_err(|e| format!("Insert failed: {}", e))?;
        Ok(conn.last_insert_rowid())
    }

    /// Mark a conversation as accepted
    pub fn mark_accepted(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE conversations SET accepted = 1 WHERE id = ?1",
            params![id],
        ).map_err(|e| format!("Update failed: {}", e))?;
        Ok(())
    }

    /// Get recent accepted replies for the same app (for style context)
    pub fn recent_accepted_replies(&self, app_name: &str, limit: usize) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT reply FROM conversations
             WHERE app_name = ?1 AND accepted = 1 AND reply != ''
             ORDER BY created_at DESC LIMIT ?2"
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        stmt.query_map(params![app_name, limit as i64], |row| row.get::<_, String>(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Get conversations that haven't been distilled yet
    pub fn undistilled_conversations(&self, limit: usize) -> Vec<Conversation> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, app_name, ocr_text, reply, accepted, distilled, created_at
             FROM conversations
             WHERE distilled = 0 AND ocr_text != ''
             ORDER BY created_at ASC LIMIT ?1"
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        stmt.query_map(params![limit as i64], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                app_name: row.get(1)?,
                ocr_text: row.get(2)?,
                reply: row.get(3)?,
                accepted: row.get::<_, i64>(4)? != 0,
                distilled: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Mark a conversation as distilled
    pub fn mark_distilled(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE conversations SET distilled = 1 WHERE id = ?1",
            params![id],
        ).map_err(|e| format!("Mark distilled failed: {}", e))?;
        Ok(())
    }

    /// Count undistilled conversations
    pub fn undistilled_count(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM conversations WHERE distilled = 0 AND ocr_text != ''",
            [], |row| row.get(0),
        ).unwrap_or(0)
    }

    // ========== Distilled Messages ==========

    /// Save distilled messages from a conversation
    pub fn save_distilled_messages(
        &self,
        conversation_id: i64,
        app_name: &str,
        messages: &[(String, String, bool)], // (sender, content, is_user)
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "INSERT INTO distilled_messages (conversation_id, app_name, sender, content, is_user)
             VALUES (?1, ?2, ?3, ?4, ?5)"
        ).map_err(|e| format!("Prepare failed: {}", e))?;

        for (sender, content, is_user) in messages {
            stmt.execute(params![conversation_id, app_name, sender, content, *is_user as i64])
                .map_err(|e| format!("Insert distilled msg failed: {}", e))?;
        }
        Ok(())
    }

    // ========== Embeddings ==========

    /// Save an embedding vector for a distilled message
    pub fn save_embedding(
        &self,
        message_id: i64,
        app_name: &str,
        content: &str,
        vector: &[f32],
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let blob = vector_to_blob(vector);
        conn.execute(
            "INSERT INTO embeddings (message_id, app_name, content, vector) VALUES (?1, ?2, ?3, ?4)",
            params![message_id, app_name, content, blob],
        ).map_err(|e| format!("Insert embedding failed: {}", e))?;
        Ok(())
    }

    /// Search for similar messages using cosine similarity.
    /// Returns (content, similarity_score) pairs, sorted by relevance.
    pub fn semantic_search(
        &self,
        query_vector: &[f32],
        app_name: &str,
        limit: usize,
    ) -> Vec<(String, f32)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT content, vector FROM embeddings WHERE app_name = ?1"
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let mut results: Vec<(String, f32)> = stmt.query_map(params![app_name], |row| {
            let content: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((content, blob))
        })
        .ok()
        .map(|rows| {
            rows.filter_map(|r| r.ok())
                .map(|(content, blob)| {
                    let stored_vec = blob_to_vector(&blob);
                    let sim = cosine_similarity(query_vector, &stored_vec);
                    (content, sim)
                })
                .collect()
        })
        .unwrap_or_default();

        // Sort by similarity descending, take top N
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        // Filter out low-relevance results (threshold: 0.7)
        results.retain(|(_, sim)| *sim > 0.7);
        results
    }

    /// Get distilled messages that don't have embeddings yet
    pub fn messages_without_embeddings(&self, limit: usize) -> Vec<(i64, String, String)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT dm.id, dm.app_name, dm.content
             FROM distilled_messages dm
             LEFT JOIN embeddings e ON e.message_id = dm.id
             WHERE e.id IS NULL AND dm.content != ''
             LIMIT ?1"
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        stmt.query_map(params![limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    // ========== Maintenance ==========

    /// Delete records older than 30 days
    fn cleanup_old_records(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute(
            "DELETE FROM conversations WHERE created_at < datetime('now', '-30 days')",
            [],
        ).map_err(|e| format!("Cleanup failed: {}", e))?;

        // Cascade: clean orphaned distilled messages and embeddings
        conn.execute(
            "DELETE FROM distilled_messages WHERE conversation_id NOT IN (SELECT id FROM conversations)",
            [],
        ).ok();
        conn.execute(
            "DELETE FROM embeddings WHERE message_id NOT IN (SELECT id FROM distilled_messages)",
            [],
        ).ok();

        if deleted > 0 {
            log::info!("Cleaned up {} old records (+ cascaded)", deleted);
        }
        Ok(())
    }

    /// Total conversation count
    pub fn count(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))
            .unwrap_or(0)
    }
}

// ========== Vector helpers ==========

fn vector_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn blob_to_vector(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}
