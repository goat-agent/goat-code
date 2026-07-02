use goat_auth::CredentialStore;
use goat_provider_openai_compat::{OpenAiCompatProvider, api_key, no_efforts};

pub const PROVIDER_ID: &str = "mistral";
const BASE_URL: &str = "https://api.mistral.ai/v1";
const HOST: &str = "api.mistral.ai";
const ENV_VAR: &str = "MISTRAL_API_KEY";

const CATALOG: &[&str] = &[
    "mistral-large-latest",
    "mistral-medium-latest",
    "mistral-small-latest",
    "devstral-medium-latest",
    "codestral-latest",
    "ministral-8b-latest",
];

const CONTEXT_WINDOWS: &[(&str, u32)] = &[
    ("mistral-large", 131_072),
    ("mistral-medium", 131_072),
    ("mistral-small", 131_072),
    ("devstral-medium", 131_072),
    ("codestral", 256_000),
];

fn is_vision_model(id: &str) -> bool {
    id.to_ascii_lowercase().contains("pixtral")
}

pub fn build(store: &CredentialStore, account: &str) -> OpenAiCompatProvider {
    api_key(store, account, PROVIDER_ID, BASE_URL, HOST, ENV_VAR)
        .with_catalog(CATALOG)
        .with_context_windows(CONTEXT_WINDOWS)
        .with_vision_filter(is_vision_model)
        .with_efforts(no_efforts)
        .with_reasoning_effort(false)
}
