//! SQLite storage for conversations and recommendations.
//! Records every OCR + LLM interaction to learn user style over time.

use rusqlite::{params, Connection, Result as SqlResult};
use std::path::PathBuf;
use std::sync::Mutex;

/// A single conversation record
#[derive(Debug, Clone)]
pub struct Conversation {
    pub id: i64,
    pub app_name: String,
    pub ocr_text: String,
    pub reply: String,
    pub accepted: bool,
    pub created_at: String,
}

/// Thread-safe storage handle
pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    /// Open (or create) the database at ~/Library/Application Support/Compleo/compleo.db
    pub fn open() -> Result<Self, String> {
        let db_path = Self::db_path();

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create data dir: {}", e))?;
        }

        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        // Enable WAL mode for better concurrent performance
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

    /// Create tables if they don't exist
    fn migrate(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                app_name    TEXT NOT NULL,
                ocr_text    TEXT NOT NULL,
                reply       TEXT NOT NULL DEFAULT '',
                accepted    INTEGER NOT NULL DEFAULT 0,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        ).map_err(|e| format!("Migration failed: {}", e))?;
        Ok(())
    }

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

    /// Mark the most recent conversation as accepted (user pasted the reply)
    pub fn mark_accepted(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE conversations SET accepted = 1 WHERE id = ?1",
            params![id],
        ).map_err(|e| format!("Update failed: {}", e))?;
        Ok(())
    }

    /// Get recent accepted replies for the same app (for style context).
    /// Returns up to `limit` most recent accepted replies.
    pub fn recent_accepted_replies(&self, app_name: &str, limit: usize) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT reply FROM conversations
             WHERE app_name = ?1 AND accepted = 1 AND reply != ''
             ORDER BY created_at DESC LIMIT ?2"
        ).unwrap_or_else(|_| {
            conn.prepare("SELECT '' WHERE 0").unwrap()
        });

        stmt.query_map(params![app_name, limit as i64], |row| row.get::<_, String>(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Delete records older than 30 days
    fn cleanup_old_records(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute(
            "DELETE FROM conversations WHERE created_at < datetime('now', '-30 days')",
            [],
        ).map_err(|e| format!("Cleanup failed: {}", e))?;

        if deleted > 0 {
            log::info!("Cleaned up {} old records", deleted);
        }
        Ok(())
    }

    /// Total conversation count (for stats/debugging)
    pub fn count(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))
            .unwrap_or(0)
    }
}
