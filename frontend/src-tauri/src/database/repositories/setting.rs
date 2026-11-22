use crate::credentials::{
    forget_in, new_secret_problem, platform_store, Store, KEY_REQUIRED_FOR_NEW_ADDRESS,
};
use crate::database::models::{Setting, TranscriptSetting};
use crate::openai::secret_destination::keeps_destination;
use crate::summary::CustomOpenAIConfig;
use sqlx::SqlitePool;

const DEFAULT_CLOUD_WHISPER_BASE_URL: &str = "https://api.openai.com/v1";
const CHANGED: &str = "Provider settings changed; retry the operation";

#[derive(serde::Deserialize)]
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

#[derive(serde::Deserialize)]
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
        Ok(setting.filter(|setting| {
            // Treat retired provider selections like an unconfigured summary model.
            !matches!(
                setting.provider.trim().to_ascii_lowercase().as_str(),
                "openclaw" | "openclaw-managed"
            )
        }))
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
        Self::save_api_key_with(&platform_store(), pool, provider, api_key).await
    }

    pub(crate) async fn save_api_key_with(
        store: &Store,
        pool: &SqlitePool,
        provider: &str,
        api_key: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        // An empty key means the field was not touched: keep the stored one.
        if api_key.trim().is_empty() {
            return Ok(());
        }
        // Operator-managed providers use their own config instead of a separate API key column.
        if provider == "custom-openai" || provider == "codex" {
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
        if let Some(problem) = new_secret_problem(api_key) {
            return Err(sqlx::Error::Protocol(problem.into()));
        }
        let insert = format!(
            r#"INSERT INTO settings (id, provider, model, whisperModel, "{api_key_column}")
               VALUES ('1', 'openai', 'gpt-4o-2024-11-20', 'large-v3', ?)"#
        );
        replace_protected_setting(
            store,
            pool,
            "settings",
            api_key_column,
            &insert,
            api_key.trim(),
        )
        .await
    }

    /// What the webview may know about a summary provider's stored key.
    pub async fn api_key_status(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<crate::credentials::SecretStatus, sqlx::Error> {
        Self::api_key_status_with(&platform_store(), pool, provider).await
    }

    pub(crate) async fn api_key_status_with(
        store: &Store,
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<crate::credentials::SecretStatus, sqlx::Error> {
        let key = Self::get_api_key_with(store, pool, provider).await?;
        Ok(crate::credentials::SecretStatus::of(key.as_deref()))
    }

    /// The key to use for a request: the one the caller supplied for this
    /// request, else the stored one. The stored key never leaves the core.
    pub async fn effective_api_key(
        pool: &SqlitePool,
        provider: &str,
        supplied: Option<String>,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        match supplied
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
        {
            Some(key) => Ok(Some(key)),
            None => Self::get_api_key(pool, provider).await,
        }
    }

    pub async fn get_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        Self::get_api_key_with(&platform_store(), pool, provider).await
    }

    pub(crate) async fn get_api_key_with(
        store: &Store,
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        // Custom OpenAI uses JSON config - extract API key from there
        if provider == "custom-openai" {
            let config = Self::get_custom_openai_config_with(store, pool).await?;
            return Ok(config.and_then(|c| c.api_key));
        }
        if provider == "codex" || provider == "claude-code" {
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

        read_protected_setting(store, pool, "settings", api_key_column).await
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
        allow_unencrypted: bool,
        key_supplied: bool,
    ) -> std::result::Result<(), sqlx::Error> {
        // A stored key goes only to the address it was saved for. A new address
        // needs the key entered again (`key_supplied`), never silently reused.
        Self::require_key_for_new_destination(pool, provider, base_url, endpoint, key_supplied)
            .await?;
        match provider {
            "cloud-whisper" => {
                crate::openai::secret_destination::validate_secret_destination(
                    base_url.unwrap_or("https://api.openai.com/v1"),
                    allow_unencrypted,
                )
                .map_err(sqlx::Error::Protocol)?;
                sqlx::query(
                    r#"
                    INSERT INTO transcript_settings (id, provider, model, cloudWhisperBaseUrl, cloudWhisperAllowUnencrypted)
                    VALUES ('1', 'cloud-whisper', 'whisper-1', $1, $2)
                    ON CONFLICT(id) DO UPDATE SET
                        cloudWhisperBaseUrl = $1, cloudWhisperAllowUnencrypted = $2
                    "#,
                )
                .bind(base_url)
                .bind(allow_unencrypted)
                .execute(pool)
                .await?;
            }
            "mai-transcribe" => {
                crate::openai::secret_destination::validate_secret_destination(
                    endpoint.unwrap_or(""),
                    allow_unencrypted,
                )
                .map_err(sqlx::Error::Protocol)?;
                sqlx::query(
                    r#"
                    INSERT INTO transcript_settings (id, provider, model, maiTranscribeEndpoint, maiTranscribeRegion, maiTranscribeAllowUnencrypted)
                    VALUES ('1', 'mai-transcribe', 'mai-transcribe-1.5', $1, $2, $3)
                    ON CONFLICT(id) DO UPDATE SET
                        maiTranscribeEndpoint = $1,
                        maiTranscribeRegion = $2, maiTranscribeAllowUnencrypted = $3
                    "#,
                )
                .bind(endpoint)
                .bind(region)
                .bind(allow_unencrypted)
                .execute(pool)
                .await?;
            }
            _ => {}
        }

        Ok(())
    }

    /// Refuses to point a stored hosted-transcription key at a different
    /// address unless a new key comes with it.
    pub(crate) async fn require_key_for_new_destination(
        pool: &SqlitePool,
        provider: &str,
        base_url: Option<&str>,
        endpoint: Option<&str>,
        key_supplied: bool,
    ) -> std::result::Result<(), sqlx::Error> {
        let (key_column, address_column, requested, default) = match provider {
            "cloud-whisper" => (
                "cloudWhisperApiKey",
                "cloudWhisperBaseUrl",
                base_url,
                Some(DEFAULT_CLOUD_WHISPER_BASE_URL),
            ),
            "mai-transcribe" => (
                "maiTranscribeApiKey",
                "maiTranscribeEndpoint",
                endpoint,
                None,
            ),
            _ => return Ok(()),
        };
        if key_supplied {
            return Ok(());
        }
        let stored: Option<(Option<String>, Option<String>)> = sqlx::query_as(&format!(
            "SELECT {key_column}, {address_column} FROM transcript_settings WHERE id = '1'"
        ))
        .fetch_optional(pool)
        .await?;
        let Some((key, address)) = stored else {
            return Ok(());
        };
        if key.as_deref().is_none_or(str::is_empty) {
            return Ok(());
        }
        let saved = address.filter(|value| !value.trim().is_empty());
        let saved = saved.as_deref().or(default);
        let requested = requested
            .filter(|value| !value.trim().is_empty())
            .or(default);
        match (saved, requested) {
            (Some(saved), Some(requested)) if !keeps_destination(saved, requested) => {
                Err(sqlx::Error::Protocol(KEY_REQUIRED_FOR_NEW_ADDRESS.into()))
            }
            // A key with no address it was saved for is bound to none: a new
            // address needs the key entered again.
            (None, Some(_)) => Err(sqlx::Error::Protocol(KEY_REQUIRED_FOR_NEW_ADDRESS.into())),
            _ => Ok(()),
        }
    }

    pub async fn save_transcript_api_key(
        pool: &SqlitePool,
        provider: &str,
        api_key: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        Self::save_transcript_api_key_with(&platform_store(), pool, provider, api_key).await
    }

    pub(crate) async fn save_transcript_api_key_with(
        store: &Store,
        pool: &SqlitePool,
        provider: &str,
        api_key: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        // An empty key means the field was not touched: keep the stored one.
        if api_key.trim().is_empty() {
            return Ok(());
        }
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
        if let Some(problem) = new_secret_problem(api_key) {
            return Err(sqlx::Error::Protocol(problem.into()));
        }
        let insert = format!(
            r#"INSERT INTO transcript_settings (id, provider, model, "{api_key_column}")
               VALUES ('1', 'parakeet', '{}', ?)"#,
            crate::config::DEFAULT_PARAKEET_MODEL
        );
        replace_protected_setting(
            store,
            pool,
            "transcript_settings",
            api_key_column,
            &insert,
            api_key.trim(),
        )
        .await
    }

    /// What the webview may know about a hosted transcription provider's key.
    pub async fn transcript_api_key_status(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<crate::credentials::SecretStatus, sqlx::Error> {
        Self::transcript_api_key_status_with(&platform_store(), pool, provider).await
    }

    pub(crate) async fn transcript_api_key_status_with(
        store: &Store,
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<crate::credentials::SecretStatus, sqlx::Error> {
        let key = Self::get_transcript_api_key_with(store, pool, provider).await?;
        Ok(crate::credentials::SecretStatus::of(key.as_deref()))
    }

    /// Removes a stored hosted-transcription key and its credential-store entry.
    pub async fn clear_transcript_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        Self::clear_transcript_api_key_with(&platform_store(), pool, provider).await
    }

    pub(crate) async fn clear_transcript_api_key_with(
        store: &Store,
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        let column = match provider {
            "localWhisper" => "whisperApiKey",
            "parakeet" => return Ok(()),
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
        clear_protected_setting(store, pool, "transcript_settings", column).await
    }

    pub async fn get_transcript_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        Self::get_transcript_api_key_with(&platform_store(), pool, provider).await
    }

    pub(crate) async fn get_transcript_api_key_with(
        store: &Store,
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

        read_protected_setting(store, pool, "transcript_settings", api_key_column).await
    }

    /// Removes a stored summary-provider key and its credential-store entry.
    pub async fn delete_api_key(
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        Self::delete_api_key_with(&platform_store(), pool, provider).await
    }

    pub(crate) async fn delete_api_key_with(
        store: &Store,
        pool: &SqlitePool,
        provider: &str,
    ) -> std::result::Result<(), sqlx::Error> {
        // Custom OpenAI keeps its other settings; only the key goes.
        if provider == "custom-openai" {
            return Self::clear_custom_openai_api_key_with(store, pool).await;
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

        clear_protected_setting(store, pool, "settings", api_key_column).await
    }

    pub async fn get_custom_openai_config(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<CustomOpenAIConfig>, sqlx::Error> {
        Self::get_custom_openai_config_with(&platform_store(), pool).await
    }

    /// Gets the custom OpenAI configuration from JSON
    ///
    /// # Returns
    /// * `Ok(Some(CustomOpenAIConfig))` - Config exists and is valid JSON
    /// * `Ok(None)` - No config stored
    /// * `Err(sqlx::Error)` - Database error
    pub(crate) async fn get_custom_openai_config_with(
        store: &Store,
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

                if let Some(mut json) = config_json {
                    let mut value: serde_json::Value =
                        serde_json::from_str(&json).map_err(|_| {
                            sqlx::Error::Protocol("Invalid provider configuration".into())
                        })?;
                    let migrated = crate::openai::secret_destination::migrate_http_opt_in(
                        &mut value,
                        &["endpoint"],
                    )
                    .map_err(sqlx::Error::Protocol)?;
                    if migrated {
                        let updated = serde_json::to_string(&value).map_err(|_| {
                            sqlx::Error::Protocol("Invalid provider configuration".into())
                        })?;
                        let changed = sqlx::query("UPDATE settings SET customOpenAIConfig = ? WHERE id = '1' AND customOpenAIConfig = ?")
                            .bind(&updated).bind(&json).execute(pool).await?;
                        if changed.rows_affected() != 1 {
                            return Err(sqlx::Error::Protocol(
                                "Provider settings changed; retry".into(),
                            ));
                        }
                        json = updated;
                    }
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
                            let protected = seal_in(store, "summary/custom-openai", stored).await?;
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
                        config.api_key =
                            Some(open_in(store, "summary/custom-openai", &protected).await?);
                    }
                    config.destination_problem =
                        crate::openai::secret_destination::validate_secret_destination(
                            &config.endpoint,
                            config.allow_unencrypted,
                        )
                        .err();
                    Ok(Some(config))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Saves the custom OpenAI configuration as JSON.
    ///
    /// A save without a new key keeps the stored one, but only for the address
    /// it was saved with: a changed address needs the key entered again. A new
    /// key replaces the old one in the order that cannot lose it (see
    /// `commit_replacement`).
    pub async fn save_custom_openai_config(
        pool: &SqlitePool,
        config: &CustomOpenAIConfig,
    ) -> std::result::Result<(), sqlx::Error> {
        Self::save_custom_openai_config_with(&platform_store(), pool, config).await
    }

    pub(crate) async fn save_custom_openai_config_with(
        store: &Store,
        pool: &SqlitePool,
        config: &CustomOpenAIConfig,
    ) -> std::result::Result<(), sqlx::Error> {
        let mut persisted = config.clone();
        persisted.destination_problem = None;
        // Reading migrates an old plaintext key to a protected reference first.
        Self::get_custom_openai_config_with(store, pool).await?;
        let old_json = Self::stored_custom_openai_json(pool).await?;
        let old_value = old_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok());
        let old_reference = old_value
            .as_ref()
            .and_then(|value| value.get("apiKey"))
            .and_then(|key| key.as_str())
            .filter(|reference| !reference.is_empty())
            .map(str::to_string);
        let old_endpoint = old_value
            .as_ref()
            .and_then(|value| value.get("endpoint"))
            .and_then(|endpoint| endpoint.as_str())
            .map(str::to_string);
        let new_key = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty());
        if let Some(key) = new_key {
            if let Some(problem) = new_secret_problem(key) {
                return Err(sqlx::Error::Protocol(problem.into()));
            }
        } else if old_reference.is_some() {
            // Keeping the stored key: it must not follow a changed address.
            if let Some(old_endpoint) = old_endpoint.as_deref() {
                if !keeps_destination(old_endpoint, &config.endpoint) {
                    return Err(sqlx::Error::Protocol(KEY_REQUIRED_FOR_NEW_ADDRESS.into()));
                }
            }
        }
        let model = config.model.clone();
        let json_for = |persisted: &CustomOpenAIConfig| {
            serde_json::to_string(persisted).map_err(|e| {
                sqlx::Error::Protocol(format!("Failed to serialize config to JSON: {}", e).into())
            })
        };
        match new_key {
            Some(key) => {
                let pool_ref = pool;
                let base = persisted;
                let old_json = old_json.clone();
                crate::credentials::commit_replacement(
                    store,
                    "summary/custom-openai",
                    key,
                    old_reference,
                    |new_reference| async move {
                        let mut with_key = base;
                        with_key.api_key = Some(new_reference);
                        let json = json_for(&with_key).map_err(|e| e.to_string())?;
                        set_custom_openai_json(pool_ref, &model, &json, old_json.as_deref())
                            .await
                            .map_err(|e| e.to_string())
                    },
                )
                .await
                .map(|_| ())
                .map_err(sqlx::Error::Protocol)
            }
            None => {
                persisted.api_key = old_reference;
                let json = json_for(&persisted)?;
                if set_custom_openai_json(pool, &model, &json, old_json.as_deref()).await? {
                    Ok(())
                } else {
                    Err(sqlx::Error::Protocol(CHANGED.into()))
                }
            }
        }
    }

    async fn stored_custom_openai_json(
        pool: &SqlitePool,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        Ok(sqlx::query_scalar::<_, Option<String>>(
            "SELECT customOpenAIConfig FROM settings WHERE id = '1'",
        )
        .fetch_optional(pool)
        .await?
        .flatten())
    }

    /// What the webview may know about the custom provider's stored key.
    pub async fn custom_openai_key_status(
        pool: &SqlitePool,
    ) -> std::result::Result<crate::credentials::SecretStatus, sqlx::Error> {
        let config = Self::get_custom_openai_config(pool).await?;
        Ok(crate::credentials::SecretStatus::of(
            config.as_ref().and_then(|config| config.api_key.as_deref()),
        ))
    }

    /// Removes the custom provider's key and its credential-store entry; the
    /// rest of the configuration stays.
    pub async fn clear_custom_openai_api_key(
        pool: &SqlitePool,
    ) -> std::result::Result<(), sqlx::Error> {
        Self::clear_custom_openai_api_key_with(&platform_store(), pool).await
    }

    pub(crate) async fn clear_custom_openai_api_key_with(
        store: &Store,
        pool: &SqlitePool,
    ) -> std::result::Result<(), sqlx::Error> {
        Self::get_custom_openai_config_with(store, pool).await?;
        let Some(json) = Self::stored_custom_openai_json(pool).await? else {
            return Ok(());
        };
        let mut value: serde_json::Value = serde_json::from_str(&json)
            .map_err(|_| sqlx::Error::Protocol("Invalid provider configuration".into()))?;
        let Some(reference) = value
            .get("apiKey")
            .and_then(|key| key.as_str())
            .filter(|reference| !reference.is_empty())
            .map(str::to_string)
        else {
            return Ok(());
        };
        if let Some(object) = value.as_object_mut() {
            object.remove("apiKey");
        }
        let updated = serde_json::to_string(&value)
            .map_err(|_| sqlx::Error::Protocol("Invalid provider configuration".into()))?;
        let changed = sqlx::query(
            "UPDATE settings SET customOpenAIConfig = ? WHERE id = '1' AND customOpenAIConfig = ?",
        )
        .bind(updated)
        .bind(&json)
        .execute(pool)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(sqlx::Error::Protocol(CHANGED.into()));
        }
        forget_in(store, "summary/custom-openai", reference).await;
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

async fn seal_in(store: &Store, scope: &str, value: &str) -> Result<String, sqlx::Error> {
    let (store, scope, value) = (store.clone(), scope.to_string(), value.to_string());
    tokio::task::spawn_blocking(move || store.seal(&scope, &value))
        .await
        .map_err(|_| sqlx::Error::Protocol("Protected credential storage is unavailable.".into()))?
        .map_err(sqlx::Error::Protocol)
}

async fn open_in(store: &Store, scope: &str, stored: &str) -> Result<String, sqlx::Error> {
    let (store, scope, stored) = (store.clone(), scope.to_string(), stored.to_string());
    tokio::task::spawn_blocking(move || store.open(&scope, &stored))
        .await
        .map_err(|_| sqlx::Error::Protocol("Protected credential storage is unavailable.".into()))?
        .map_err(sqlx::Error::Protocol)
}

/// The stored value of a credential column as it is in the database: a
/// protected reference (or an old plaintext value), never an opened secret.
async fn read_stored_reference(
    pool: &SqlitePool,
    table: &str,
    column: &str,
) -> Result<Option<String>, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, Option<String>>(&format!(
        "SELECT {column} FROM {table} WHERE id = '1'"
    ))
    .fetch_optional(pool)
    .await?
    .flatten()
    .filter(|value| !value.is_empty()))
}

/// Points a credential column at `new_reference` only if it still holds
/// `old_reference` (compare-and-set). Inserts the first row when there is none.
/// Returns false when another save changed the column first.
async fn compare_and_set_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    insert_sql: &str,
    new_reference: &str,
    old_reference: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(&format!(
        "UPDATE {table} SET {column} = ? WHERE id = '1' AND COALESCE({column}, '') = ?"
    ))
    .bind(new_reference)
    .bind(old_reference.unwrap_or(""))
    .execute(pool)
    .await?;
    if updated.rows_affected() == 1 {
        return Ok(true);
    }
    let exists: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE id = '1'"))
        .fetch_one(pool)
        .await?;
    if exists > 0 {
        return Ok(false);
    }
    sqlx::query(insert_sql)
        .bind(new_reference)
        .execute(pool)
        .await?;
    Ok(true)
}

/// Replaces a stored key: new entry, compare-and-set update, then the old entry
/// is removed (see `commit_replacement`).
async fn replace_protected_setting(
    store: &Store,
    pool: &SqlitePool,
    table: &str,
    column: &str,
    insert_sql: &str,
    new_key: &str,
) -> Result<(), sqlx::Error> {
    let scope = format!("{table}/{column}");
    let old = read_stored_reference(pool, table, column).await?;
    let old_for_update = old.clone();
    crate::credentials::commit_replacement(
        store,
        &scope,
        new_key,
        old,
        |new_reference| async move {
            compare_and_set_column(
                pool,
                table,
                column,
                insert_sql,
                &new_reference,
                old_for_update.as_deref(),
            )
            .await
            .map_err(|e| e.to_string())
        },
    )
    .await
    .map(|_| ())
    .map_err(sqlx::Error::Protocol)
}

/// Writes the custom provider JSON only if it still is `old_json` (compare-and-set),
/// inserting the first row when there is none. False when another save changed it first.
async fn set_custom_openai_json(
    pool: &SqlitePool,
    model: &str,
    json: &str,
    old_json: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "UPDATE settings SET customOpenAIConfig = ? WHERE id = '1' AND customOpenAIConfig IS ?",
    )
    .bind(json)
    .bind(old_json)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 1 {
        return Ok(true);
    }
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM settings WHERE id = '1'")
        .fetch_one(pool)
        .await?;
    if exists > 0 {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO settings (id, provider, model, whisperModel, customOpenAIConfig) VALUES ('1', 'custom-openai', ?, 'large-v3', ?)",
    )
    .bind(model)
    .bind(json)
    .execute(pool)
    .await?;
    Ok(true)
}

/// Clears a credential column and removes its credential-store entry. The
/// column is cleared first; a store entry that cannot be removed is logged
/// without detail and does not bring the key back.
async fn clear_protected_setting(
    store: &Store,
    pool: &SqlitePool,
    table: &str,
    column: &str,
) -> Result<(), sqlx::Error> {
    let old = read_stored_reference(pool, table, column).await?;
    sqlx::query(&format!(
        "UPDATE {table} SET {column} = NULL WHERE id = '1'"
    ))
    .execute(pool)
    .await?;
    if let Some(old) = old {
        forget_in(store, &format!("{table}/{column}"), old).await;
    }
    Ok(())
}

async fn read_protected_setting(
    store: &Store,
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
            return open_in(store, &scope, &stored).await.map(Some);
        }
        let protected = seal_in(store, &scope, &stored).await?;
        let updated = sqlx::query(&format!(
            "UPDATE {table} SET {column} = ? WHERE id = '1' AND {column} = ?"
        ))
        .bind(&protected)
        .bind(&stored)
        .execute(pool)
        .await?;
        if updated.rows_affected() == 1 {
            return open_in(store, &scope, &protected).await.map(Some);
        }
    }
    Err(sqlx::Error::Protocol(CHANGED.into()))
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
            if read_protected_setting(&platform_store(), pool, table, column)
                .await
                .is_err()
            {
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

#[cfg(test)]
mod secret_tests {
    use super::*;
    use crate::credentials::test_support::MemoryStore;
    use crate::credentials::SecretStore;
    use std::sync::Arc;

    const KEY_ONE: &str = "fake-key-one-0000000000";
    const KEY_TWO: &str = "fake-key-two-1111111111";
    const SHORT: &str = "abcd";

    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    fn fixture() -> (Arc<MemoryStore>, Store) {
        let memory = MemoryStore::new();
        let store: Store = memory.clone();
        (memory, store)
    }

    async fn column(pool: &SqlitePool, table: &str, column: &str) -> Option<String> {
        sqlx::query_scalar::<_, Option<String>>(&format!(
            "SELECT {column} FROM {table} WHERE id = '1'"
        ))
        .fetch_optional(pool)
        .await
        .unwrap()
        .flatten()
    }

    async fn fail_updates(pool: &SqlitePool, table: &str) {
        sqlx::query(&format!(
            "CREATE TRIGGER block_updates BEFORE UPDATE ON {table} BEGIN SELECT RAISE(ABORT, 'blocked'); END"
        ))
        .execute(pool)
        .await
        .unwrap();
    }

    fn custom(endpoint: &str, key: Option<&str>) -> CustomOpenAIConfig {
        CustomOpenAIConfig {
            destination_problem: None,
            allow_unencrypted: false,
            endpoint: endpoint.into(),
            api_key: key.map(str::to_string),
            model: "model-a".into(),
            timeout_seconds: Some(60),
            organization: None,
            project: None,
            max_tokens: None,
            context_window: None,
            temperature: None,
            top_p: None,
        }
    }

    #[tokio::test]
    async fn a_summary_key_is_stored_as_a_reference_and_replaced_without_leaving_entries() {
        let pool = pool().await;
        let (memory, store) = fixture();
        SettingsRepository::save_api_key_with(&store, &pool, "openai", KEY_ONE)
            .await
            .unwrap();
        let reference = column(&pool, "settings", "openaiApiKey").await.unwrap();
        assert!(crate::credentials::is_protected(&reference));
        assert!(!reference.contains(KEY_ONE));
        assert_eq!(memory.entries(), 1);
        SettingsRepository::save_api_key_with(&store, &pool, "openai", KEY_TWO)
            .await
            .unwrap();
        assert_eq!(
            memory.entries(),
            1,
            "the old entry is removed after the update"
        );
        assert_eq!(
            SettingsRepository::get_api_key_with(&store, &pool, "openai")
                .await
                .unwrap()
                .as_deref(),
            Some(KEY_TWO)
        );
        // An empty key keeps the stored one.
        SettingsRepository::save_api_key_with(&store, &pool, "openai", "  ")
            .await
            .unwrap();
        assert_eq!(
            SettingsRepository::get_api_key_with(&store, &pool, "openai")
                .await
                .unwrap()
                .as_deref(),
            Some(KEY_TWO)
        );
    }

    #[tokio::test]
    async fn a_new_key_shorter_than_eight_characters_is_refused_and_changes_nothing() {
        let pool = pool().await;
        let (memory, store) = fixture();
        SettingsRepository::save_api_key_with(&store, &pool, "openai", KEY_ONE)
            .await
            .unwrap();
        let before = column(&pool, "settings", "openaiApiKey").await;
        for short in [SHORT, "1234567", "   abcdefg   "] {
            let error = SettingsRepository::save_api_key_with(&store, &pool, "openai", short)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("too short"));
            let error =
                SettingsRepository::save_transcript_api_key_with(&store, &pool, "groq", short)
                    .await
                    .unwrap_err();
            assert!(error.to_string().contains("too short"));
        }
        assert_eq!(column(&pool, "settings", "openaiApiKey").await, before);
        assert_eq!(memory.entries(), 1);
        // Exactly eight characters is accepted.
        SettingsRepository::save_api_key_with(&store, &pool, "groq", "12345678")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_failing_update_keeps_the_old_key_and_removes_the_new_entry() {
        let pool = pool().await;
        let (memory, store) = fixture();
        SettingsRepository::save_api_key_with(&store, &pool, "openai", KEY_ONE)
            .await
            .unwrap();
        let before = column(&pool, "settings", "openaiApiKey").await;
        fail_updates(&pool, "settings").await;
        let error = SettingsRepository::save_api_key_with(&store, &pool, "openai", KEY_TWO)
            .await
            .unwrap_err();
        assert!(!error.to_string().contains(KEY_TWO));
        assert_eq!(column(&pool, "settings", "openaiApiKey").await, before);
        assert_eq!(memory.entries(), 1, "the new entry was removed again");
        assert!(!memory.contains_value(KEY_TWO));
        assert_eq!(
            SettingsRepository::get_api_key_with(&store, &pool, "openai")
                .await
                .unwrap()
                .as_deref(),
            Some(KEY_ONE)
        );
    }

    #[tokio::test]
    async fn a_lost_race_keeps_the_column_and_removes_the_new_entry() {
        let pool = pool().await;
        let (memory, store) = fixture();
        SettingsRepository::save_api_key_with(&store, &pool, "openai", KEY_ONE)
            .await
            .unwrap();
        // The save read a reference that another save has since replaced.
        let stale = memory
            .seal("settings/openaiApiKey", "fake-key-stale-22222222")
            .unwrap();
        let current = column(&pool, "settings", "openaiApiKey").await.unwrap();
        let replaced = crate::credentials::commit_replacement(
            &store,
            "settings/openaiApiKey",
            KEY_TWO,
            Some(stale.clone()),
            |new_reference| {
                let pool = pool.clone();
                async move {
                    compare_and_set_column(
                        &pool,
                        "settings",
                        "openaiApiKey",
                        "SELECT 1",
                        &new_reference,
                        Some(&stale),
                    )
                    .await
                    .map_err(|e| e.to_string())
                }
            },
        )
        .await;
        assert!(replaced.is_err());
        assert_eq!(
            column(&pool, "settings", "openaiApiKey").await.unwrap(),
            current
        );
        assert!(
            !memory.contains_value(KEY_TWO),
            "the new entry was removed again"
        );
        assert_eq!(
            SettingsRepository::get_api_key_with(&store, &pool, "openai")
                .await
                .unwrap()
                .as_deref(),
            Some(KEY_ONE)
        );
    }

    #[tokio::test]
    async fn clearing_a_summary_key_removes_the_column_value_and_the_entry() {
        let pool = pool().await;
        let (memory, store) = fixture();
        SettingsRepository::save_api_key_with(&store, &pool, "groq", KEY_ONE)
            .await
            .unwrap();
        SettingsRepository::delete_api_key_with(&store, &pool, "groq")
            .await
            .unwrap();
        assert_eq!(column(&pool, "settings", "groqApiKey").await, None);
        assert_eq!(memory.entries(), 0);
        assert!(SettingsRepository::get_api_key_with(&store, &pool, "groq")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn the_custom_key_follows_its_address_and_never_a_different_one() {
        let pool = pool().await;
        let (memory, store) = fixture();
        let a = "https://gateway.example.com/v1";
        SettingsRepository::save_custom_openai_config_with(
            &store,
            &pool,
            &custom(a, Some(KEY_ONE)),
        )
        .await
        .unwrap();
        let before = column(&pool, "settings", "customOpenAIConfig").await;

        // Endpoint B without a new key is refused; key and endpoint stay.
        let error = SettingsRepository::save_custom_openai_config_with(
            &store,
            &pool,
            &custom("https://other.example.com/v1", None),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Enter the key again"));
        assert_eq!(
            column(&pool, "settings", "customOpenAIConfig").await,
            before
        );
        assert_eq!(memory.entries(), 1);
        let kept = SettingsRepository::get_custom_openai_config_with(&store, &pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(kept.endpoint, a);
        assert_eq!(kept.api_key.as_deref(), Some(KEY_ONE));

        // Same endpoint written with other case and a trailing slash passes.
        SettingsRepository::save_custom_openai_config_with(
            &store,
            &pool,
            &custom(" HTTPS://Gateway.Example.com/V1/ ", None),
        )
        .await
        .unwrap();
        let kept = SettingsRepository::get_custom_openai_config_with(&store, &pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(kept.api_key.as_deref(), Some(KEY_ONE));

        // A different scheme or port is a different destination.
        for other in [
            "http://gateway.example.com/v1",
            "https://gateway.example.com:8443/v1",
        ] {
            assert!(SettingsRepository::save_custom_openai_config_with(
                &store,
                &pool,
                &custom(other, None)
            )
            .await
            .is_err());
        }

        // Endpoint B with a newly entered key is accepted and the old entry goes.
        SettingsRepository::save_custom_openai_config_with(
            &store,
            &pool,
            &custom("https://other.example.com/v1", Some(KEY_TWO)),
        )
        .await
        .unwrap();
        let now = SettingsRepository::get_custom_openai_config_with(&store, &pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(now.endpoint, "https://other.example.com/v1");
        assert_eq!(now.api_key.as_deref(), Some(KEY_TWO));
        assert_eq!(memory.entries(), 1);
        assert!(!memory.contains_value(KEY_ONE));
    }

    #[tokio::test]
    async fn without_a_stored_key_the_custom_address_can_change_freely() {
        let pool = pool().await;
        let (_memory, store) = fixture();
        SettingsRepository::save_custom_openai_config_with(
            &store,
            &pool,
            &custom("https://a.example.com/v1", None),
        )
        .await
        .unwrap();
        SettingsRepository::save_custom_openai_config_with(
            &store,
            &pool,
            &custom("https://b.example.com/v1", None),
        )
        .await
        .unwrap();
        let now = SettingsRepository::get_custom_openai_config_with(&store, &pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(now.endpoint, "https://b.example.com/v1");
        assert!(now.api_key.is_none());
    }

    #[tokio::test]
    async fn a_short_custom_key_is_refused() {
        let pool = pool().await;
        let (memory, store) = fixture();
        let error = SettingsRepository::save_custom_openai_config_with(
            &store,
            &pool,
            &custom("https://a.example.com/v1", Some(SHORT)),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("too short"));
        assert_eq!(memory.entries(), 0);
        assert!(column(&pool, "settings", "customOpenAIConfig")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn a_failing_custom_update_keeps_the_old_key_and_removes_the_new_entry() {
        let pool = pool().await;
        let (memory, store) = fixture();
        let a = "https://gateway.example.com/v1";
        SettingsRepository::save_custom_openai_config_with(
            &store,
            &pool,
            &custom(a, Some(KEY_ONE)),
        )
        .await
        .unwrap();
        let before = column(&pool, "settings", "customOpenAIConfig").await;
        fail_updates(&pool, "settings").await;
        assert!(SettingsRepository::save_custom_openai_config_with(
            &store,
            &pool,
            &custom(a, Some(KEY_TWO))
        )
        .await
        .is_err());
        assert_eq!(
            column(&pool, "settings", "customOpenAIConfig").await,
            before
        );
        assert_eq!(memory.entries(), 1);
        assert!(!memory.contains_value(KEY_TWO));
    }

    #[tokio::test]
    async fn clearing_the_custom_key_keeps_the_other_settings_and_removes_the_entry() {
        let pool = pool().await;
        let (memory, store) = fixture();
        let a = "https://gateway.example.com/v1";
        SettingsRepository::save_custom_openai_config_with(
            &store,
            &pool,
            &custom(a, Some(KEY_ONE)),
        )
        .await
        .unwrap();
        SettingsRepository::clear_custom_openai_api_key_with(&store, &pool)
            .await
            .unwrap();
        assert_eq!(memory.entries(), 0);
        let now = SettingsRepository::get_custom_openai_config_with(&store, &pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(now.endpoint, a);
        assert_eq!(now.model, "model-a");
        assert!(now.api_key.is_none());
    }

    #[tokio::test]
    async fn the_hosted_transcription_key_stays_with_its_address() {
        let pool = pool().await;
        let (_memory, store) = fixture();
        let a = Some("https://api.example.com/v1");
        SettingsRepository::save_transcript_provider_config(
            &pool,
            "cloud-whisper",
            a,
            None,
            None,
            false,
            false,
        )
        .await
        .unwrap();
        SettingsRepository::save_transcript_api_key_with(&store, &pool, "cloud-whisper", KEY_ONE)
            .await
            .unwrap();
        let before = column(&pool, "transcript_settings", "cloudWhisperBaseUrl").await;

        let error = SettingsRepository::save_transcript_provider_config(
            &pool,
            "cloud-whisper",
            Some("https://other.example.com/v1"),
            None,
            None,
            false,
            false,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Enter the key again"));
        assert_eq!(
            column(&pool, "transcript_settings", "cloudWhisperBaseUrl").await,
            before
        );

        // Other case and a trailing slash are the same address.
        SettingsRepository::save_transcript_provider_config(
            &pool,
            "cloud-whisper",
            Some("HTTPS://API.example.com/V1/"),
            None,
            None,
            false,
            false,
        )
        .await
        .unwrap();

        // A new address with a newly entered key is accepted.
        SettingsRepository::save_transcript_provider_config(
            &pool,
            "cloud-whisper",
            Some("https://other.example.com/v1"),
            None,
            None,
            false,
            true,
        )
        .await
        .unwrap();
        assert_eq!(
            column(&pool, "transcript_settings", "cloudWhisperBaseUrl")
                .await
                .as_deref(),
            Some("https://other.example.com/v1")
        );
    }

    #[tokio::test]
    async fn the_mai_key_stays_with_its_endpoint() {
        let pool = pool().await;
        let (_memory, store) = fixture();
        let a = Some("https://mai.example.com/transcribe");
        SettingsRepository::save_transcript_provider_config(
            &pool,
            "mai-transcribe",
            None,
            a,
            Some("west"),
            false,
            false,
        )
        .await
        .unwrap();
        SettingsRepository::save_transcript_api_key_with(&store, &pool, "mai-transcribe", KEY_ONE)
            .await
            .unwrap();
        let error = SettingsRepository::save_transcript_provider_config(
            &pool,
            "mai-transcribe",
            None,
            Some("https://elsewhere.example.com/transcribe"),
            Some("west"),
            false,
            false,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Enter the key again"));
        assert_eq!(
            column(&pool, "transcript_settings", "maiTranscribeEndpoint")
                .await
                .as_deref(),
            a
        );
    }

    #[tokio::test]
    async fn a_stored_key_with_no_saved_address_needs_the_key_again_for_a_new_address() {
        let pool = pool().await;
        let (_memory, store) = fixture();
        SettingsRepository::save_transcript_api_key_with(&store, &pool, "mai-transcribe", KEY_ONE)
            .await
            .unwrap();
        let address = Some("https://mai.example.com/transcribe");
        let error = SettingsRepository::save_transcript_provider_config(
            &pool,
            "mai-transcribe",
            None,
            address,
            Some("west"),
            false,
            false,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Enter the key again"));
        assert_eq!(
            column(&pool, "transcript_settings", "maiTranscribeEndpoint").await,
            None
        );
        // With the key entered again for the new address it is accepted.
        SettingsRepository::save_transcript_provider_config(
            &pool,
            "mai-transcribe",
            None,
            address,
            Some("west"),
            false,
            true,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_failing_transcript_update_keeps_the_old_key() {
        let pool = pool().await;
        let (memory, store) = fixture();
        SettingsRepository::save_transcript_api_key_with(&store, &pool, "groq", KEY_ONE)
            .await
            .unwrap();
        fail_updates(&pool, "transcript_settings").await;
        assert!(
            SettingsRepository::save_transcript_api_key_with(&store, &pool, "groq", KEY_TWO)
                .await
                .is_err()
        );
        assert_eq!(memory.entries(), 1);
        assert_eq!(
            SettingsRepository::get_transcript_api_key_with(&store, &pool, "groq")
                .await
                .unwrap()
                .as_deref(),
            Some(KEY_ONE)
        );
    }
}
