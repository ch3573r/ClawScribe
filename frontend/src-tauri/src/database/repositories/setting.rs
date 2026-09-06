use crate::database::models::{Setting, TranscriptSetting};
use crate::summary::CustomOpenAIConfig;
use sqlx::SqlitePool;

#[derive(serde::Deserialize, Debug)]
pub struct SaveModelConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(rename = "whisperModel")]
    pub whisper_model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "ollamaEndpoint")]
    pub ollama_endpoint: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
pub struct SaveTranscriptConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "baseUrl")]
    pub base_url: Option<String>,
    pub endpoint: Option<String>,
    pub region: Option<String>,
}

pub struct SettingsRepository;

// Transcript providers: localWhisper, deepgram, elevenLabs, groq, openai
// Summary providers: openai, claude, ollama, groq, added openrouter
// NOTE: Handle data exclusion in the higher layer as this is database abstraction layer(using SELECT *)

impl SettingsRepository {
    pub async fn get_model_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<Setting>, sqlx::Error> {
        let setting = sqlx::query_as::<_, Setting>("SELECT * FROM settings LIMIT 1")
            .fetch_optional(pool)
            .await?;
        Ok(setting)
    }

    pub async fn save_model_config(
        pool: &SqlitePool,
        provider: &str,
        model: &str,
        whisper_model: &str,
        ollama_endpoint: Option<&str>,
    ) -> std::result::Result<(), sqlx::Error> {
        // Using id '1' for backward compatibility
        sqlx::query(
            r#"
            INSERT INTO settings (id, provider, model, whisperModel, ollamaEndpoint)
            VALUES ('1', $1, $2, $3, $4)
            ON CONFLICT(id) DO UPDATE SET
                provider = excluded.provider,
                model = excluded.model,
                whisperModel = excluded.whisperModel,
                ollamaEndpoint = excluded.ollamaEndpoint
            "#,
        )
        .bind(provider)
        .bind(model)
        .bind(whisper_model)
        .bind(ollama_endpoint)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn save_api_key(
        pool: &SqlitePool,
        provider: &str,
        api_key: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        // Operator-managed providers use their own config instead of a separate API key column.
        if provider == "custom-openai" || provider == "openclaw" || provider == "codex" {
            return Err(sqlx::Error::Protocol(
                format!("{provider} provider should not use save_api_key()").into(),
            ));
        }

        let api_key_column = match provider {
            "openai" => "openaiApiKey",
            "claude" => "anthropicApiKey",
            "ollama" => "ollamaApiKey",
            "groq" => "groqApiKey",
            "openrouter" => "openRouterApiKey",
            "builtin-ai" => return Ok(()), // No API key needed
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            r#"
            INSERT INTO settings (id, provider, model, whisperModel, "{}")
            VALUES ('1', 'openai', 'gpt-4o-2024-11-20', 'large-v3', $1)
            ON CONFLICT(id) DO UPDATE SET
                "{}" = $1
            "#,
            api_key_column, api_key_column
        );
        let protected = crate::credentials::seal_async(
            format!("settings/{api_key_column}"),
            api_key.to_string(),
        )
        .await?;
        sqlx::query(&query).bind(protected).execute(pool).await?;

        Ok(())
    }

    pub async fn get_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        // Custom OpenAI uses JSON config - extract API key from there
        if provider == "custom-openai" {
            let config = Self::get_custom_openai_config(pool).await?;
            return Ok(config.and_then(|c| c.api_key));
        }
        if provider == "openclaw" || provider == "codex" {
            return Ok(None);
        }

        let api_key_column = match provider {
            "openai" => "openaiApiKey",
            "ollama" => "ollamaApiKey",
            "groq" => "groqApiKey",
            "claude" => "anthropicApiKey",
            "openrouter" => "openRouterApiKey",
            "builtin-ai" => return Ok(None), // No API key needed
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        read_protected_setting(pool, "settings", api_key_column).await
    }

    pub async fn get_transcript_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<TranscriptSetting>, sqlx::Error> {
        let setting =
            sqlx::query_as::<_, TranscriptSetting>("SELECT * FROM transcript_settings LIMIT 1")
                .fetch_optional(pool)
                .await?;
        Ok(setting)
    }

    pub async fn save_transcript_config(
        pool: &SqlitePool,
        provider: &str,
        model: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO transcript_settings (id, provider, model)
            VALUES ('1', $1, $2)
            ON CONFLICT(id) DO UPDATE SET
                provider = excluded.provider,
                model = excluded.model
            "#,
        )
        .bind(provider)
        .bind(model)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn save_transcript_provider_config(
        pool: &SqlitePool,
        provider: &str,
        base_url: Option<&str>,
        endpoint: Option<&str>,
        region: Option<&str>,
    ) -> std::result::Result<(), sqlx::Error> {
        match provider {
            "cloud-whisper" => {
                sqlx::query(
                    r#"
                    INSERT INTO transcript_settings (id, provider, model, cloudWhisperBaseUrl)
                    VALUES ('1', 'cloud-whisper', 'whisper-1', $1)
                    ON CONFLICT(id) DO UPDATE SET
                        cloudWhisperBaseUrl = $1
                    "#,
                )
                .bind(base_url)
                .execute(pool)
                .await?;
            }
            "mai-transcribe" => {
                sqlx::query(
                    r#"
                    INSERT INTO transcript_settings (id, provider, model, maiTranscribeEndpoint, maiTranscribeRegion)
                    VALUES ('1', 'mai-transcribe', 'mai-transcribe-1.5', $1, $2)
                    ON CONFLICT(id) DO UPDATE SET
                        maiTranscribeEndpoint = $1,
                        maiTranscribeRegion = $2
                    "#,
                )
                .bind(endpoint)
                .bind(region)
                .execute(pool)
                .await?;
            }
            _ => {}
        }

        Ok(())
    }

    pub async fn save_transcript_api_key(
        pool: &SqlitePool,
        provider: &str,
        api_key: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        let api_key_column = match provider {
            "localWhisper" => "whisperApiKey",
            "parakeet" => return Ok(()), // Parakeet doesn't need an API key, return early
            "deepgram" => "deepgramApiKey",
            "elevenLabs" => "elevenLabsApiKey",
            "groq" => "groqApiKey",
            "openai" => "openaiApiKey",
            "cloud-whisper" => "cloudWhisperApiKey",
            "mai-transcribe" => "maiTranscribeApiKey",
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            r#"
            INSERT INTO transcript_settings (id, provider, model, "{}")
            VALUES ('1', 'parakeet', '{}', $1)
            ON CONFLICT(id) DO UPDATE SET
                "{}" = $1
            "#,
            api_key_column,
            crate::config::DEFAULT_PARAKEET_MODEL,
            api_key_column
        );
        let protected = crate::credentials::seal_async(
            format!("transcript_settings/{api_key_column}"),
            api_key.to_string(),
        )
        .await?;
        sqlx::query(&query).bind(protected).execute(pool).await?;

        Ok(())
    }

    pub async fn get_transcript_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        let api_key_column = match provider {
            "localWhisper" => "whisperApiKey",
            "parakeet" => return Ok(None), // Parakeet doesn't need an API key
            "deepgram" => "deepgramApiKey",
            "elevenLabs" => "elevenLabsApiKey",
            "groq" => "groqApiKey",
            "openai" => "openaiApiKey",
            "cloud-whisper" => "cloudWhisperApiKey",
            "mai-transcribe" => "maiTranscribeApiKey",
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        read_protected_setting(pool, "transcript_settings", api_key_column).await
    }

    pub async fn delete_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        // Custom OpenAI uses JSON config - clear the entire config
        if provider == "custom-openai" {
            sqlx::query("UPDATE settings SET customOpenAIConfig = NULL WHERE id = '1'")
                .execute(pool)
                .await?;
            return Ok(());
        }

        let api_key_column = match provider {
            "openai" => "openaiApiKey",
            "ollama" => "ollamaApiKey",
            "groq" => "groqApiKey",
            "claude" => "anthropicApiKey",
            "openrouter" => "openRouterApiKey",
            "builtin-ai" => return Ok(()), // No API key needed
            "codex" => return Ok(()),      // Codex auth is managed by Codex CLI
            _ => {
                return Err(sqlx::Error::Protocol(
                    format!("Invalid provider: {}", provider).into(),
                ))
            }
        };

        let query = format!(
            "UPDATE settings SET {} = NULL WHERE id = '1'",
            api_key_column
        );
        sqlx::query(&query).execute(pool).await?;

        Ok(())
    }

    // ===== CUSTOM OPENAI CONFIG METHODS =====

    /// Gets the custom OpenAI configuration from JSON
    ///
    /// # Returns
    /// * `Ok(Some(CustomOpenAIConfig))` - Config exists and is valid JSON
    /// * `Ok(None)` - No config stored
    /// * `Err(sqlx::Error)` - Database error
    pub async fn get_custom_openai_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<CustomOpenAIConfig>, sqlx::Error> {
        use sqlx::Row;

        let row = sqlx::query(
            r#"
            SELECT customOpenAIConfig
            FROM settings
            WHERE id = '1'
            LIMIT 1
            "#,
        )
        .fetch_optional(pool)
        .await?;

        match row {
            Some(record) => {
                let config_json: Option<String> = record.get("customOpenAIConfig");

                if let Some(json) = config_json {
                    // Parse JSON into CustomOpenAIConfig
                    let mut config: CustomOpenAIConfig =
                        serde_json::from_str(&json).map_err(|e| {
                            sqlx::Error::Protocol(
                                format!("Invalid JSON in customOpenAIConfig: {}", e).into(),
                            )
                        })?;

                    if let Some(stored) = config.api_key.as_ref().filter(|key| !key.is_empty()) {
                        let protected = if crate::credentials::is_protected(stored) {
                            stored.clone()
                        } else {
                            let protected = crate::credentials::seal_async(
                                "summary/custom-openai".into(),
                                stored.clone(),
                            )
                            .await?;
                            let mut persisted = config.clone();
                            persisted.api_key = Some(protected.clone());
                            let serialized = serde_json::to_string(&persisted).map_err(|_| {
                                sqlx::Error::Protocol("Invalid provider configuration".into())
                            })?;
                            let changed = sqlx::query("UPDATE settings SET customOpenAIConfig = ? WHERE id = '1' AND customOpenAIConfig = ?")
                                .bind(serialized).bind(&json).execute(pool).await?;
                            if changed.rows_affected() != 1 {
                                return Err(sqlx::Error::Protocol(
                                    "Provider settings changed; retry the operation".into(),
                                ));
                            }
                            protected
                        };
                        config.api_key = Some(
                            crate::credentials::open_async(
                                "summary/custom-openai".into(),
                                protected,
                            )
                            .await?,
                        );
                    }
                    Ok(Some(config))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Saves the custom OpenAI configuration as JSON
    ///
    /// # Arguments
    /// * `pool` - Database connection pool
    /// * `config` - CustomOpenAIConfig to save (includes endpoint, apiKey, model, maxTokens, temperature, topP)
    ///
    /// # Returns
    /// * `Ok(())` - Config saved successfully
    /// * `Err(sqlx::Error)` - Database or JSON serialization error
    pub async fn save_custom_openai_config(
        pool: &SqlitePool,
        config: &CustomOpenAIConfig,
    ) -> std::result::Result<(), sqlx::Error> {
        let mut persisted = config.clone();
        if let Some(value) = &config.api_key {
            persisted.api_key = Some(
                crate::credentials::seal_async("summary/custom-openai".into(), value.clone())
                    .await?,
            );
        }
        let config_json = serde_json::to_string(&persisted).map_err(|e| {
            sqlx::Error::Protocol(format!("Failed to serialize config to JSON: {}", e).into())
        })?;

        // Upsert into settings table
        sqlx::query(
            r#"
            INSERT INTO settings (id, provider, model, whisperModel, customOpenAIConfig)
            VALUES ('1', 'custom-openai', $1, 'large-v3', $2)
            ON CONFLICT(id) DO UPDATE SET
                customOpenAIConfig = excluded.customOpenAIConfig
            "#,
        )
        .bind(&config.model)
        .bind(config_json)
        .execute(pool)
        .await?;

        Ok(())
    }

    // ===== OPENAI AUTH CONFIG METHODS =====

    /// Gets OpenAI auth-mode metadata JSON. This intentionally does not return API keys.
    pub async fn get_openai_auth_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        let config_json = sqlx::query_scalar(
            r#"
            SELECT openAIAuthConfig
            FROM settings
            WHERE id = '1'
            LIMIT 1
            "#,
        )
        .fetch_optional(pool)
        .await?;

        Ok(config_json)
    }

    /// Saves OpenAI auth-mode metadata JSON. Keys use protected references in compatibility columns.
    pub async fn save_openai_auth_config(
        pool: &SqlitePool,
        config_json: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO settings (id, provider, model, whisperModel, openAIAuthConfig)
            VALUES ('1', 'openai', 'gpt-4o-2024-11-20', 'large-v3', $1)
            ON CONFLICT(id) DO UPDATE SET
                openAIAuthConfig = excluded.openAIAuthConfig
            "#,
        )
        .bind(config_json)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Clears OpenAI auth-mode metadata without deleting any legacy OpenAI API key.
    pub async fn clear_openai_auth_config(
        pool: &SqlitePool,
    ) -> std::result::Result<(), sqlx::Error> {
        sqlx::query("UPDATE settings SET openAIAuthConfig = NULL WHERE id = '1'")
            .execute(pool)
            .await?;

        Ok(())
    }
}

// Table and column identifiers are supplied only by the closed provider maps above.
async fn read_protected_setting(
    pool: &SqlitePool,
    table: &str,
    column: &str,
) -> Result<Option<String>, sqlx::Error> {
    let scope = format!("{table}/{column}");
    for _ in 0..3 {
        let stored = sqlx::query_scalar::<_, Option<String>>(&format!(
            "SELECT {column} FROM {table} WHERE id = '1'"
        ))
        .fetch_optional(pool)
        .await?
        .flatten();
        let Some(stored) = stored.filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        if crate::credentials::is_protected(&stored) {
            return crate::credentials::open_async(scope, stored)
                .await
                .map(Some);
        }
        let protected = crate::credentials::seal_async(scope.clone(), stored.clone()).await?;
        let updated = sqlx::query(&format!(
            "UPDATE {table} SET {column} = ? WHERE id = '1' AND {column} = ?"
        ))
        .bind(&protected)
        .bind(&stored)
        .execute(pool)
        .await?;
        if updated.rows_affected() == 1 {
            return crate::credentials::open_async(scope, protected)
                .await
                .map(Some);
        }
    }
    Err(sqlx::Error::Protocol(
        "Provider settings changed; retry the operation".into(),
    ))
}

/// Migrate independent providers without making local recording depend on any
/// credential service. A provider whose migration fails stays unusable until
/// its protected store is available; plaintext is never used as a fallback.
pub(crate) async fn migrate_provider_credentials(pool: &SqlitePool) {
    for (table, columns) in [
        (
            "settings",
            &[
                "openaiApiKey",
                "anthropicApiKey",
                "ollamaApiKey",
                "groqApiKey",
                "openRouterApiKey",
            ][..],
        ),
        (
            "transcript_settings",
            &[
                "whisperApiKey",
                "deepgramApiKey",
                "elevenLabsApiKey",
                "groqApiKey",
                "openaiApiKey",
                "cloudWhisperApiKey",
                "maiTranscribeApiKey",
            ][..],
        ),
    ] {
        for column in columns {
            if read_protected_setting(pool, table, column).await.is_err() {
                log::warn!(
                    "A provider credential needs protected-store access before it can be used"
                );
            }
        }
    }
    if SettingsRepository::get_custom_openai_config(pool)
        .await
        .is_err()
    {
        log::warn!("Custom provider credentials need protected-store access before use");
    }
}
