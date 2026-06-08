use std::sync::Arc;

use goat_auth::CredentialStore;
use goat_provider::{AuthMethod, ModelProvider, ProviderId};

pub const DEFAULT_ACCOUNT: &str = "default";

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("provider {0} does not support OAuth")]
    Unsupported(String),
    #[error(transparent)]
    Provider(#[from] goat_provider_openai_codex::CodexError),
    #[error(transparent)]
    Anthropic(#[from] goat_provider_anthropic::AnthropicAuthError),
}

pub async fn oauth_login(
    provider: &str,
    status: &tokio::sync::mpsc::Sender<String>,
) -> Result<goat_auth::OAuthTokenSet, OAuthError> {
    match provider {
        goat_provider_openai_codex::PROVIDER_ID => goat_provider_openai_codex::login(status)
            .await
            .map_err(OAuthError::Provider),
        goat_provider_anthropic::PROVIDER_ID => goat_provider_anthropic::login(status)
            .await
            .map_err(OAuthError::Anthropic),
        _ => Err(OAuthError::Unsupported(provider.to_owned())),
    }
}

pub struct Registry {
    providers: Vec<Arc<dyn ModelProvider>>,
}

impl Registry {
    pub fn builtin(store: &CredentialStore) -> Self {
        Self::for_account(store, DEFAULT_ACCOUNT)
    }

    pub fn for_account(store: &CredentialStore, account: &str) -> Self {
        let providers: Vec<Arc<dyn ModelProvider>> = vec![
            Arc::new(goat_provider_openai::build(store, account)),
            Arc::new(goat_provider_openai_codex::build(store, account)),
            Arc::new(goat_provider_anthropic::build(store, account)),
            Arc::new(goat_provider_local::ollama()),
            Arc::new(goat_provider_local::lmstudio()),
            Arc::new(goat_provider_local::llama_cpp()),
        ];
        Self { providers }
    }

    pub fn from_providers(providers: Vec<Arc<dyn ModelProvider>>) -> Self {
        Self { providers }
    }

    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn ModelProvider>> {
        self.providers
            .iter()
            .find(|provider| &provider.id() == id)
            .cloned()
    }

    pub fn login_providers(&self) -> Vec<(String, AuthMethod)> {
        self.providers
            .iter()
            .map(|provider| (provider.id().to_string(), provider.capabilities().auth))
            .collect()
    }

    pub fn all(&self) -> &[Arc<dyn ModelProvider>] {
        &self.providers
    }
}

pub fn build_provider(
    store: &CredentialStore,
    provider: &str,
    account: &str,
) -> Option<Arc<dyn ModelProvider>> {
    Registry::for_account(store, account)
        .all()
        .iter()
        .find(|candidate| candidate.id().to_string() == provider)
        .cloned()
}

#[cfg(test)]
mod tests {
    use goat_provider::ProviderId;

    use super::Registry;

    #[test]
    fn builtin_registers_known_providers() {
        let store = goat_auth::CredentialStore::new(
            std::env::temp_dir().join("goat-providers-registry-test.json"),
        );
        let registry = Registry::builtin(&store);
        assert_eq!(registry.all().len(), 6);
        assert!(registry.get(&ProviderId::from("anthropic")).is_some());
        assert!(registry.get(&ProviderId::from("ollama")).is_some());
        assert!(registry.get(&ProviderId::from("does-not-exist")).is_none());
    }
}
