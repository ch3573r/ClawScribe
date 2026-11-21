use super::llm_client::{canonical_provider, fixed_summary_endpoint, LLMProvider};
use super::openai_provider::{
    config_from_custom_openai, config_from_openai_api_key, OpenAICompatibleProcessingProvider,
    OpenAICompatibleProviderConfig,
};
use crate::database::repositories::setting::SettingsRepository;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::sync::Arc;
use tauri::{AppHandle, Runtime};
use url::{Host, Url};

pub(crate) type DispatchCheck = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct AuthorizedRequest {
    pub request: AutomaticRequest,
    pub check: DispatchCheck,
}

// The executable request owns all provider inputs, including its credential.
// It is deliberately neither serializable nor printable.
#[derive(Clone)]
pub(crate) struct AutomaticRequest {
    pub provider: LLMProvider,
    pub model: String,
    pub execution: Execution,
}

#[derive(Clone)]
pub(crate) enum Execution {
    BuiltIn,
    Ollama {
        base: Url,
        client: reqwest::Client,
    },
    Standard {
        endpoint: Url,
        api_key: String,
        client: reqwest::Client,
    },
    Compatible(OpenAICompatibleProcessingProvider),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DestinationReview {
    pub provider: &'static str,
    pub model: String,
    pub endpoint: Option<String>,
    pub external: bool,
    pub unencrypted: bool,
    pub fingerprint: String,
    pub approved: bool,
}

pub(super) fn canonical_endpoint(raw: &str) -> Result<Url, String> {
    let endpoint = Url::parse(raw.trim()).map_err(|_| "Enter a valid summary endpoint URL.")?;
    if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host().is_none() {
        return Err("The summary endpoint must use HTTP or HTTPS.".into());
    }
    if !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(
            "Remove credentials, query parameters, and fragments from the summary endpoint URL."
                .into(),
        );
    }
    Ok(endpoint)
}

fn is_loopback(endpoint: &Url) -> bool {
    match endpoint.host() {
        Some(Host::Domain("localhost")) => true,
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

pub(super) fn direct_client() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
}

async fn credential(pool: &SqlitePool, provider: &'static str) -> Result<String, String> {
    SettingsRepository::get_api_key(pool, provider)
        .await
        .map_err(|_| "Could not read the summary provider credential")?
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| "Set the summary provider credential in Settings.".into())
}

impl AutomaticRequest {
    pub async fn resolve<R: Runtime>(
        app: &AppHandle<R>,
        pool: &SqlitePool,
    ) -> Result<Self, String> {
        let settings = SettingsRepository::get_model_config(pool)
            .await
            .map_err(|_| "Could not read the summary model settings")?
            .ok_or("Choose a summary model in Settings.")?;
        let provider = LLMProvider::from_str(&settings.provider)
            .map_err(|_| "Automatic notes cannot identify this provider's destination.")?;
        let mut model = settings.model.trim().to_owned();
        let execution = match provider {
            LLMProvider::BuiltInAI => {
                model = super::summary_engine::models::get_model_by_name(&model)
                    .ok_or("Choose a Built-in AI summary model in Settings.")?
                    .name;
                Execution::BuiltIn
            }
            LLMProvider::Ollama => {
                let endpoint = settings
                    .ollama_endpoint
                    .as_deref()
                    .map(str::trim)
                    .filter(|url| !url.is_empty())
                    .unwrap_or("http://localhost:11434");
                let base = canonical_endpoint(endpoint)?;
                let mut client = direct_client();
                if base.host() == Some(Host::Domain("localhost")) {
                    // Keep the local exception local even if the OS resolver is changed.
                    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
                    let port = base
                        .port_or_known_default()
                        .ok_or("The Ollama endpoint has no port")?;
                    client = client.resolve_to_addrs(
                        "localhost",
                        &[
                            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
                            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
                        ],
                    );
                }
                let client = client
                    .build()
                    .map_err(|_| "Could not initialize the Ollama connection")?;
                Execution::Ollama { base, client }
            }
            LLMProvider::OpenAI => Self::compatible(config_from_openai_api_key(
                Some(credential(pool, "openai").await?),
                model.clone(),
            ))?,
            LLMProvider::OpenAICompatible | LLMProvider::CustomOpenAI => {
                let configured = SettingsRepository::get_custom_openai_config(pool)
                    .await
                    .map_err(|_| "Could not read the compatible summary provider settings")?;
                let config = match configured {
                    Some(mut config) => {
                        config.endpoint = canonical_endpoint(&config.endpoint)?.into();
                        config_from_custom_openai(config)
                    }
                    None if provider == LLMProvider::OpenAICompatible => {
                        config_from_openai_api_key(
                            Some(credential(pool, "openai").await?),
                            model.clone(),
                        )
                    }
                    None => return Err("Set the compatible summary endpoint in Settings.".into()),
                };
                Self::compatible(config)?
            }
            LLMProvider::OpenClaw => {
                let config = crate::openclaw::load_config(app)
                    .map_err(|_| "Could not read the OpenClaw summary provider settings")?;
                if !config.enabled || config.bearer_token.trim().is_empty() {
                    return Err("Enable OpenClaw and set its credential in Settings.".into());
                }
                Self::compatible(OpenAICompatibleProviderConfig {
                    base_url: canonical_endpoint(&config.model_endpoint)?.into(),
                    api_key: Some(config.bearer_token),
                    allow_unencrypted: config.allow_unencrypted,
                    context_window: super::context_budget::resolve(&provider, &model, None, None)
                        .context_tokens,
                    model: model.clone(),
                    temperature: None,
                    ..OpenAICompatibleProviderConfig::default()
                })?
            }
            LLMProvider::Claude | LLMProvider::Groq | LLMProvider::OpenRouter => {
                let endpoint = canonical_endpoint(
                    fixed_summary_endpoint(&provider)
                        .ok_or("Automatic notes cannot identify this provider's destination.")?,
                )?;
                let api_key = credential(pool, canonical_provider(&provider)).await?;
                let client = direct_client()
                    .build()
                    .map_err(|_| "Could not initialize the summary provider connection")?;
                Execution::Standard {
                    endpoint,
                    api_key,
                    client,
                }
            }
            LLMProvider::Codex => {
                return Err("Automatic notes are unavailable with Codex because ClawScribe cannot verify which ChatGPT workspace will receive the transcript. Generate notes from the saved meeting.".into());
            }
            LLMProvider::ClaudeCode => {
                return Err("Automatic notes are unavailable with Claude Code because ClawScribe cannot verify which account will receive the transcript. Generate notes from the saved meeting.".into());
            }
        };
        if let Execution::Compatible(compatible) = &execution {
            model = compatible.text_settings().0.to_owned();
        }
        if model.is_empty() {
            return Err("Choose a summary model in Settings.".into());
        }
        Ok(Self {
            provider,
            model,
            execution,
        })
    }

    fn compatible(config: OpenAICompatibleProviderConfig) -> Result<Execution, String> {
        let endpoint = canonical_endpoint(&config.base_url)?;
        crate::openai::secret_destination::validate_secret_destination(
            endpoint.as_str(),
            config.allow_unencrypted,
        )?;
        OpenAICompatibleProcessingProvider::new_direct(config).map(Execution::Compatible)
    }

    pub fn review(&self) -> DestinationReview {
        let (endpoint, external, policy, organization, project) = match &self.execution {
            Execution::BuiltIn => (None, false, "builtin", None, None),
            Execution::Ollama { base, .. } => (
                Some(base.clone()),
                !is_loopback(base),
                "direct-ollama",
                None,
                None,
            ),
            Execution::Standard { endpoint, .. } => {
                (Some(endpoint.clone()), true, "direct-https", None, None)
            }
            Execution::Compatible(provider) => {
                let (url, allow_unencrypted, organization, project) = provider.destination();
                let policy = if allow_unencrypted {
                    "direct-lan-http"
                } else {
                    "direct-secure"
                };
                (
                    Some(Url::parse(&url).expect("validated compatible endpoint")),
                    true,
                    policy,
                    organization,
                    project,
                )
            }
        };
        let provider = canonical_provider(&self.provider);
        // A broader outbound payload requires a new approval for the same route.
        // Keep existing local imports and interrupted local jobs eligible.
        let version = if external {
            "automatic-summary-v3"
        } else {
            "automatic-summary-v2"
        };
        let identity = serde_json::to_vec(&(
            version,
            provider,
            &self.model,
            endpoint
                .as_ref()
                .map(|url| url.as_str().trim_end_matches('/')),
            policy,
            organization,
            project,
        ))
        .expect("serializable request identity");
        DestinationReview {
            provider,
            model: self.model.clone(),
            endpoint: endpoint
                .as_ref()
                .map(|url| url.as_str().trim_end_matches('/').to_owned()),
            external,
            unencrypted: endpoint.as_ref().is_some_and(|url| url.scheme() == "http"),
            fingerprint: format!("{:x}", Sha256::digest(identity)),
            approved: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ollama(url: &str) -> DestinationReview {
        AutomaticRequest {
            provider: LLMProvider::Ollama,
            model: "synthetic-model".into(),
            execution: Execution::Ollama {
                base: canonical_endpoint(url).unwrap(),
                client: direct_client().build().unwrap(),
            },
        }
        .review()
    }

    #[test]
    fn only_loopback_ollama_is_local_and_the_entire_route_is_bound_to_consent() {
        assert!(!ollama("http://LOCALHOST:11434").external);
        assert!(!ollama("http://127.2.3.4:11434").external);
        assert!(!ollama("http://[::1]:11434").external);
        assert!(ollama("http://localhost.example.com:11434").external);
        assert!(ollama("http://localhost.:11434").external);
        let first = ollama("https://models.example.com/path");
        assert_eq!(
            first.fingerprint,
            ollama("https://MODELS.example.com:443/path/").fingerprint
        );
        assert_ne!(
            first.fingerprint,
            ollama("https://models.example.com/Path").fingerprint
        );
        assert_ne!(
            first.fingerprint,
            ollama("https://models.example.org/path").fingerprint
        );
        assert_eq!(
            first.endpoint.as_deref(),
            Some("https://models.example.com/path")
        );
    }

    #[test]
    fn automatic_urls_refuse_credentials_queries_and_fragments() {
        for url in [
            "https://user@models.example.com",
            "https://models.example.com?token=example",
            "http://localhost/#route",
        ] {
            assert!(canonical_endpoint(url).is_err());
        }
    }
}
