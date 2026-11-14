use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::chat::{
    Conversation, ConversationId, ConversationSummary, CostEstimate, MessageId, MessageRole,
    MessageStatus, StoredMessage, TokenUsage, ToolCall,
};
use crate::config::Protocol;

pub const SCHEMA_VERSION: i64 = 1;
pub const DEFAULT_MAX_CONVERSATIONS: u32 = 500;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("could not open the local database at {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("could not prepare the local database directory at {path}")]
    Directory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not update local database permissions at {path}")]
    Permissions {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("local database error")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not encode or decode local database data")]
    Json(#[from] serde_json::Error),
    #[error("local database contains an unsupported value: {0}")]
    InvalidData(String),
    #[error("local database schema version {found} is newer than this build supports")]
    UnsupportedSchema { found: i64 },
}

pub fn unix_time_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub struct Storage {
    connection: Connection,
}

pub struct NewMessage<'a> {
    pub role: MessageRole,
    pub content: &'a str,
    pub tool_calls: &'a [ToolCall],
    pub tool_call_id: Option<&'a str>,
    pub tool_name: Option<&'a str>,
    pub status: MessageStatus,
    pub usage: Option<&'a TokenUsage>,
    pub cost: Option<&'a CostEstimate>,
    pub provider_id: Option<u64>,
    pub protocol: Option<Protocol>,
    pub model: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageRow {
    pub provider_id: u64,
    pub protocol: Protocol,
    pub model: String,
    pub message_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub estimated_cost: f64,
    pub currency: Option<String>,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| StorageError::Directory {
                path: parent.to_owned(),
                source,
            })?;
            set_directory_permissions(parent).map_err(|source| StorageError::Permissions {
                path: parent.to_owned(),
                source,
            })?;
        }
        let connection = Connection::open(path).map_err(|source| StorageError::Open {
            path: path.to_owned(),
            source,
        })?;
        let storage = Self::from_connection(connection)?;
        set_file_permissions(path).map_err(|source| StorageError::Permissions {
            path: path.to_owned(),
            source,
        })?;
        Ok(storage)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, StorageError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        let mut storage = Self { connection };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&mut self) -> Result<(), StorageError> {
        let found: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if found > SCHEMA_VERSION {
            return Err(StorageError::UnsupportedSchema { found });
        }
        if found == 0 {
            self.connection.execute_batch(
                "
                BEGIN IMMEDIATE;
                CREATE TABLE conversations (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    title TEXT NOT NULL,
                    provider_id INTEGER NOT NULL,
                    protocol TEXT NOT NULL,
                    model TEXT NOT NULL,
                    system_prompt TEXT NOT NULL DEFAULT '',
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE INDEX idx_conversations_updated
                    ON conversations(updated_at DESC);
                CREATE TABLE messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    conversation_id INTEGER NOT NULL
                        REFERENCES conversations(id) ON DELETE CASCADE,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL DEFAULT '',
                    tool_calls_json TEXT,
                    tool_call_id TEXT,
                    tool_name TEXT,
                    status TEXT NOT NULL DEFAULT 'complete',
                    created_at INTEGER NOT NULL,
                    provider_id INTEGER,
                    protocol TEXT,
                    model TEXT,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cached_input_tokens INTEGER,
                    input_price_per_mtok REAL,
                    output_price_per_mtok REAL,
                    cached_input_price_per_mtok REAL,
                    currency TEXT,
                    estimated_cost REAL
                );
                CREATE INDEX idx_messages_conversation
                    ON messages(conversation_id, id);
                CREATE INDEX idx_messages_usage
                    ON messages(provider_id, protocol, model);
                CREATE TABLE tool_calls (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    conversation_id INTEGER NOT NULL
                        REFERENCES conversations(id) ON DELETE CASCADE,
                    message_id INTEGER
                        REFERENCES messages(id) ON DELETE CASCADE,
                    server_id INTEGER NOT NULL,
                    server_name TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    arguments_json TEXT NOT NULL,
                    result_text TEXT,
                    status TEXT NOT NULL,
                    error TEXT,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER
                );
                CREATE INDEX idx_tool_calls_conversation
                    ON tool_calls(conversation_id, id);
                PRAGMA user_version = 1;
                COMMIT;
                ",
            )?;
        }
        Ok(())
    }

    pub fn create_conversation(
        &self,
        provider_id: u64,
        protocol: Protocol,
        model: &str,
        title: &str,
        system_prompt: &str,
    ) -> Result<ConversationId, StorageError> {
        let now = unix_time_now();
        self.connection.execute(
            "INSERT INTO conversations
             (title, provider_id, protocol, model, system_prompt, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                title.trim(),
                provider_id as i64,
                protocol_str(protocol),
                model.trim(),
                system_prompt,
                now as i64,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn list_conversations(&self, limit: u32) -> Result<Vec<ConversationSummary>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT c.id, c.title, c.provider_id, c.model, c.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id)
             FROM conversations c
             ORDER BY c.updated_at DESC, c.id DESC
             LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok(ConversationSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                provider_id: from_i64(row.get(2)?, "provider_id")?,
                model: row.get(3)?,
                updated_at: from_i64(row.get(4)?, "updated_at")?,
                message_count: from_i64(row.get(5)?, "message_count")?,
            })
        })?;
        let mut conversations = Vec::new();
        for row in rows {
            conversations.push(row?);
        }
        Ok(conversations)
    }

    pub fn load_conversation(
        &self,
        id: ConversationId,
    ) -> Result<Option<Conversation>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, title, provider_id, protocol, model, system_prompt,
                    created_at, updated_at
             FROM conversations WHERE id = ?1",
        )?;
        let row = statement
            .query_row([id], |row| {
                let protocol: String = row.get(3)?;
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    provider_id: from_i64(row.get(2)?, "provider_id")?,
                    protocol: parse_protocol(&protocol)?,
                    model: row.get(4)?,
                    system_prompt: row.get(5)?,
                    created_at: from_i64(row.get(6)?, "created_at")?,
                    updated_at: from_i64(row.get(7)?, "updated_at")?,
                })
            })
            .optional()?;
        Ok(row)
    }

    pub fn load_messages(
        &self,
        conversation_id: ConversationId,
        limit: u32,
    ) -> Result<Vec<StoredMessage>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, role, content, tool_calls_json, tool_call_id, tool_name,
                    status, created_at, input_tokens, output_tokens,
                    cached_input_tokens, input_price_per_mtok,
                    output_price_per_mtok, cached_input_price_per_mtok,
                    currency, estimated_cost
             FROM (
                 SELECT * FROM messages
                 WHERE conversation_id = ?1
                 ORDER BY id DESC
                 LIMIT ?2
             ) ORDER BY id ASC",
        )?;
        let rows = statement.query_map(params![conversation_id, limit as i64], |row| {
            let role: String = row.get(1)?;
            let tool_calls_json: Option<String> = row.get(3)?;
            let status: String = row.get(6)?;
            let tool_calls = match tool_calls_json {
                Some(json) if !json.trim().is_empty() => {
                    serde_json::from_str::<Vec<ToolCall>>(&json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?
                }
                _ => Vec::new(),
            };
            let usage = match (
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<i64>>(10)?,
            ) {
                (None, None, None) => None,
                (input, output, cached) => Some(TokenUsage {
                    input_tokens: opt_i64(input),
                    output_tokens: opt_i64(output),
                    cached_input_tokens: opt_i64(cached),
                }),
            };
            let cost = match (
                row.get::<_, Option<f64>>(11)?,
                row.get::<_, Option<f64>>(12)?,
                row.get::<_, Option<f64>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<f64>>(15)?,
            ) {
                (Some(input), Some(output), cached, Some(currency), Some(amount)) => {
                    Some(CostEstimate {
                        amount,
                        currency,
                        input_price_per_mtok: input,
                        output_price_per_mtok: output,
                        cached_input_price_per_mtok: cached,
                    })
                }
                _ => None,
            };
            Ok(StoredMessage {
                id: row.get(0)?,
                role: parse_role(&role)?,
                content: row.get(2)?,
                tool_calls,
                tool_call_id: row.get(4)?,
                tool_name: row.get(5)?,
                status: parse_status(&status)?,
                created_at: from_i64(row.get(7)?, "created_at")?,
                usage,
                cost,
            })
        })?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row?);
        }
        Ok(messages)
    }

    pub fn append_message(
        &self,
        conversation_id: ConversationId,
        message: NewMessage<'_>,
    ) -> Result<MessageId, StorageError> {
        let now = unix_time_now();
        let tool_calls_json = if message.tool_calls.is_empty() {
            None
        } else {
            Some(serde_json::to_string(message.tool_calls)?)
        };
        let usage = message.usage;
        let cost = message.cost;
        self.connection.execute(
            "INSERT INTO messages
             (conversation_id, role, content, tool_calls_json, tool_call_id,
              tool_name, status, created_at, provider_id, protocol, model,
              input_tokens, output_tokens, cached_input_tokens,
              input_price_per_mtok, output_price_per_mtok,
              cached_input_price_per_mtok, currency, estimated_cost)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                conversation_id,
                message.role.as_str(),
                message.content,
                tool_calls_json,
                message.tool_call_id,
                message.tool_name,
                message.status.as_str(),
                now as i64,
                message.provider_id.map(|value| value as i64),
                message.protocol.map(protocol_str),
                message.model,
                usage.map(|usage| usage.input_tokens as i64),
                usage.map(|usage| usage.output_tokens as i64),
                usage.map(|usage| usage.cached_input_tokens as i64),
                cost.map(|cost| cost.input_price_per_mtok),
                cost.map(|cost| cost.output_price_per_mtok),
                cost.and_then(|cost| cost.cached_input_price_per_mtok),
                cost.map(|cost| cost.currency.clone()),
                cost.map(|cost| cost.amount),
            ],
        )?;
        let message_id = self.connection.last_insert_rowid();
        self.connection.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now as i64, conversation_id],
        )?;
        Ok(message_id)
    }

    pub fn rename_conversation(&self, id: ConversationId, title: &str) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title.trim(), unix_time_now() as i64, id],
        )?;
        Ok(())
    }

    pub fn update_conversation_model(
        &self,
        id: ConversationId,
        provider_id: u64,
        protocol: Protocol,
        model: &str,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE conversations SET provider_id = ?1, protocol = ?2, model = ?3,
                    updated_at = ?4 WHERE id = ?5",
            params![
                provider_id as i64,
                protocol_str(protocol),
                model,
                unix_time_now() as i64,
                id,
            ],
        )?;
        Ok(())
    }

    pub fn update_system_prompt(
        &self,
        id: ConversationId,
        system_prompt: &str,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE conversations SET system_prompt = ?1, updated_at = ?2 WHERE id = ?3",
            params![system_prompt, unix_time_now() as i64, id],
        )?;
        Ok(())
    }

    pub fn delete_conversation(&self, id: ConversationId) -> Result<(), StorageError> {
        self.connection
            .execute("DELETE FROM conversations WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn clear_all_history(&self) -> Result<(), StorageError> {
        self.connection
            .execute_batch("DELETE FROM conversations;")?;
        Ok(())
    }

    pub fn usage_rows(&self) -> Result<Vec<UsageRow>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT provider_id, protocol, model, COUNT(*),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cached_input_tokens), 0),
                    COALESCE(SUM(estimated_cost), 0),
                    MAX(currency)
             FROM messages
             WHERE role = 'assistant' AND provider_id IS NOT NULL
             GROUP BY provider_id, protocol, model
             ORDER BY provider_id, model",
        )?;
        let rows = statement.query_map([], |row| {
            let protocol: String = row.get(1)?;
            Ok(UsageRow {
                provider_id: from_i64(row.get(0)?, "provider_id")?,
                protocol: parse_protocol(&protocol)?,
                model: row.get(2)?,
                message_count: from_i64(row.get(3)?, "message_count")?,
                input_tokens: from_i64(row.get(4)?, "input_tokens")?,
                output_tokens: from_i64(row.get(5)?, "output_tokens")?,
                cached_input_tokens: from_i64(row.get(6)?, "cached_input_tokens")?,
                estimated_cost: row.get::<_, f64>(7)?,
                currency: row.get(8)?,
            })
        })?;
        let mut usage = Vec::new();
        for row in rows {
            usage.push(row?);
        }
        Ok(usage)
    }

    pub fn conversation_usage(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Option<(u64, u64, Option<String>, f64)>, StorageError> {
        let row = self
            .connection
            .query_row(
                "SELECT COALESCE(SUM(input_tokens), 0),
                        COALESCE(SUM(output_tokens), 0),
                        MAX(currency), COALESCE(SUM(estimated_cost), 0)
                 FROM messages
                 WHERE conversation_id = ?1 AND role = 'assistant'",
                [conversation_id],
                |row| {
                    Ok((
                        from_i64(row.get(0)?, "input_tokens")?,
                        from_i64(row.get(1)?, "output_tokens")?,
                        row.get(2)?,
                        row.get(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn conversation_count(&self) -> Result<u64, StorageError> {
        let count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))?;
        Ok(from_i64(count, "conversation_count")?)
    }

    pub fn prune(
        &self,
        retention_days: u32,
        max_conversations: u32,
    ) -> Result<usize, StorageError> {
        let cutoff = unix_time_now().saturating_sub(u64::from(retention_days) * 86_400);
        let mut removed = self.connection.execute(
            "DELETE FROM conversations WHERE updated_at < ?1",
            [cutoff as i64],
        )?;
        let count = self.conversation_count()?;
        if count > u64::from(max_conversations) {
            let excess = count - u64::from(max_conversations);
            removed += self.connection.execute(
                "DELETE FROM conversations WHERE id IN (
                    SELECT id FROM conversations ORDER BY updated_at ASC, id ASC LIMIT ?1
                )",
                [excess as i64],
            )?;
        }
        Ok(removed)
    }

    pub fn insert_tool_call(
        &self,
        conversation_id: ConversationId,
        message_id: Option<MessageId>,
        server_id: u64,
        server_name: &str,
        tool_name: &str,
        arguments_json: &str,
    ) -> Result<i64, StorageError> {
        self.connection.execute(
            "INSERT INTO tool_calls
             (conversation_id, message_id, server_id, server_name, tool_name,
              arguments_json, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
            params![
                conversation_id,
                message_id,
                server_id as i64,
                server_name,
                tool_name,
                arguments_json,
                unix_time_now() as i64,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn complete_tool_call(
        &self,
        id: i64,
        status: &str,
        result_text: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), StorageError> {
        self.connection.execute(
            "UPDATE tool_calls SET status = ?1, result_text = ?2, error = ?3,
                    completed_at = ?4 WHERE id = ?5",
            params![status, result_text, error, unix_time_now() as i64, id],
        )?;
        Ok(())
    }
}

fn protocol_str(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "open_ai_chat",
        Protocol::OpenAiResponses => "open_ai_responses",
        Protocol::AnthropicMessages => "anthropic_messages",
        Protocol::GeminiGenerateContent => "gemini_generate_content",
        Protocol::OllamaChat => "ollama_chat",
    }
}

fn parse_protocol(value: &str) -> Result<Protocol, rusqlite::Error> {
    match value {
        "open_ai_chat" => Ok(Protocol::OpenAiChat),
        "open_ai_responses" => Ok(Protocol::OpenAiResponses),
        "anthropic_messages" => Ok(Protocol::AnthropicMessages),
        "gemini_generate_content" => Ok(Protocol::GeminiGenerateContent),
        "ollama_chat" => Ok(Protocol::OllamaChat),
        other => Err(conversion_error(3, format!("unknown protocol {other}"))),
    }
}

fn parse_role(value: &str) -> Result<MessageRole, rusqlite::Error> {
    MessageRole::parse(value)
        .ok_or_else(|| conversion_error(1, format!("unknown message role {value}")))
}

fn parse_status(value: &str) -> Result<MessageStatus, rusqlite::Error> {
    MessageStatus::parse(value)
        .ok_or_else(|| conversion_error(6, format!("unknown message status {value}")))
}

fn conversion_error(column: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        Box::new(StorageError::InvalidData(message)),
    )
}

fn from_i64(value: i64, field: &str) -> Result<u64, rusqlite::Error> {
    u64::try_from(value).map_err(|_| conversion_error(0, format!("negative {field} value {value}")))
}

fn opt_i64(value: Option<i64>) -> u64 {
    value
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(0)
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_file_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ToolCall;

    fn new_message<'a>(role: MessageRole, content: &'a str) -> NewMessage<'a> {
        NewMessage {
            role,
            content,
            tool_calls: &[],
            tool_call_id: None,
            tool_name: None,
            status: MessageStatus::Complete,
            usage: None,
            cost: None,
            provider_id: None,
            protocol: None,
            model: None,
        }
    }

    #[test]
    fn creates_lists_and_loads_a_conversation() {
        let storage = Storage::open_in_memory().unwrap();
        let id = storage
            .create_conversation(1, Protocol::OpenAiChat, "auto", "First", "")
            .unwrap();
        storage
            .append_message(
                id,
                NewMessage {
                    role: MessageRole::User,
                    content: "hello",
                    ..new_message(MessageRole::User, "")
                },
            )
            .unwrap();
        storage
            .append_message(
                id,
                NewMessage {
                    role: MessageRole::Assistant,
                    content: "hi",
                    usage: Some(&TokenUsage {
                        input_tokens: 10,
                        output_tokens: 4,
                        cached_input_tokens: 2,
                    }),
                    cost: Some(&CostEstimate {
                        amount: 0.0001,
                        currency: "USD".into(),
                        input_price_per_mtok: 1.0,
                        output_price_per_mtok: 2.0,
                        cached_input_price_per_mtok: None,
                    }),
                    provider_id: Some(1),
                    protocol: Some(Protocol::OpenAiChat),
                    model: Some("auto"),
                    ..new_message(MessageRole::Assistant, "")
                },
            )
            .unwrap();

        let conversation = storage.load_conversation(id).unwrap().unwrap();
        assert_eq!(conversation.title, "First");
        let messages = storage.load_messages(id, 100).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].content, "hi");
        assert_eq!(messages[1].usage.as_ref().unwrap().input_tokens, 10);
        assert_eq!(messages[1].cost.as_ref().unwrap().currency, "USD");
        let list = storage.list_conversations(10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].message_count, 2);
        let usage = storage.usage_rows().unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].input_tokens, 10);
    }

    #[test]
    fn tool_calls_and_results_round_trip() {
        let storage = Storage::open_in_memory().unwrap();
        let id = storage
            .create_conversation(1, Protocol::OpenAiChat, "auto", "Tools", "")
            .unwrap();
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "files__read".into(),
            arguments: "{\"path\":\"/tmp/a\"}".into(),
        }];
        storage
            .append_message(
                id,
                NewMessage {
                    role: MessageRole::Assistant,
                    content: "checking",
                    tool_calls: &calls,
                    ..new_message(MessageRole::Assistant, "")
                },
            )
            .unwrap();
        let messages = storage.load_messages(id, 10).unwrap();
        assert_eq!(messages[0].tool_calls, calls);

        let tool_id = storage
            .insert_tool_call(id, Some(messages[0].id), 2, "Files", "files__read", "{}")
            .unwrap();
        storage
            .complete_tool_call(tool_id, "ok", Some("result"), None)
            .unwrap();
    }

    #[test]
    fn deleting_a_conversation_cascades_messages() {
        let storage = Storage::open_in_memory().unwrap();
        let id = storage
            .create_conversation(1, Protocol::OpenAiChat, "auto", "Delete", "")
            .unwrap();
        storage
            .append_message(id, new_message(MessageRole::User, "bye"))
            .unwrap();
        storage.delete_conversation(id).unwrap();
        assert_eq!(storage.conversation_count().unwrap(), 0);
        assert!(storage.load_messages(id, 10).unwrap().is_empty());
    }

    #[test]
    fn pruning_keeps_the_newest_conversations() {
        let storage = Storage::open_in_memory().unwrap();
        for index in 0..5 {
            let id = storage
                .create_conversation(
                    1,
                    Protocol::OpenAiChat,
                    "auto",
                    &format!("Conversation {index}"),
                    "",
                )
                .unwrap();
            storage
                .append_message(id, new_message(MessageRole::User, "x"))
                .unwrap();
        }
        storage.prune(365, 2).unwrap();
        assert_eq!(storage.conversation_count().unwrap(), 2);
        let list = storage.list_conversations(10).unwrap();
        assert_eq!(list[0].title, "Conversation 4");
        assert_eq!(list[1].title, "Conversation 3");
    }
}
