use goat_provider::{AuthMethod, ProviderId};
use goat_provider_openai_compat::OpenAiCompatProvider;

pub const PROVIDER_ID: &str = "lmstudio";
const BASE_URL: &str = "http://localhost:1234/v1";

pub fn build() -> OpenAiCompatProvider {
    OpenAiCompatProvider::new(
        ProviderId::from(PROVIDER_ID),
        BASE_URL,
        None,
        AuthMethod::None,
    )
}

#[cfg(test)]
mod tests {
    use goat_provider::{ModelProvider, ProviderId};

    use super::{PROVIDER_ID, build};

    #[test]
    fn builds_with_provider_id() {
        assert_eq!(build().id(), ProviderId::from(PROVIDER_ID));
    }
}
