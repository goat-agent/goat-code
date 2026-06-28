use std::io::Write;
use std::time::Duration;

use color_eyre::eyre::eyre;
use goat_auth::{
    Credential, CredentialKey, CredentialKind, CredentialService, CredentialStore, SecretString,
};
use goat_provider::{AuthMethod, ProviderId};
use goat_providers::{DEFAULT_ACCOUNT, Registry};
use tokio::sync::mpsc;

use crate::cli::{AuthCommand, ProviderCommand};

pub async fn run(command: AuthCommand) -> color_eyre::Result<()> {
    let path = goat_config::auth_path().ok_or_else(|| eyre!(goat_config::HOME_NOT_FOUND))?;
    let store = CredentialStore::new(path);
    match command {
        AuthCommand::Login {
            provider,
            account,
            key,
            service,
        } => {
            let account = account.as_deref().unwrap_or(DEFAULT_ACCOUNT);
            login(&store, &provider, account, key, None, service.into()).await
        }
        AuthCommand::List => {
            list(&store, None);
            Ok(())
        }
        AuthCommand::Logout {
            provider,
            account,
            service,
        } => {
            let account = account.as_deref().unwrap_or(DEFAULT_ACCOUNT);
            logout(&store, &provider, account, service.into())
        }
    }
}

pub async fn run_provider(command: ProviderCommand) -> color_eyre::Result<()> {
    let path = goat_config::auth_path().ok_or_else(|| eyre!(goat_config::HOME_NOT_FOUND))?;
    let store = CredentialStore::new(path);
    match command {
        ProviderCommand::Login {
            provider,
            account,
            key,
            endpoint,
        } => {
            let account = account.as_deref().unwrap_or(DEFAULT_ACCOUNT);
            login(
                &store,
                &provider,
                account,
                key,
                endpoint,
                CredentialService::Model,
            )
            .await
        }
        ProviderCommand::List { accounts } => {
            if accounts {
                list_provider_accounts(&store);
            } else {
                list_providers(&store);
            }
            Ok(())
        }
        ProviderCommand::Accounts => {
            list_provider_accounts(&store);
            Ok(())
        }
        ProviderCommand::Info { provider } => provider_info(&store, &provider),
        ProviderCommand::Logout { provider, account } => {
            let account = account.as_deref().unwrap_or(DEFAULT_ACCOUNT);
            logout(&store, &provider, account, CredentialService::Model)
        }
    }
}

async fn login(
    store: &CredentialStore,
    provider: &str,
    account: &str,
    key: Option<String>,
    endpoint: Option<String>,
    service: CredentialService,
) -> color_eyre::Result<()> {
    if service == CredentialService::Search {
        return login_search(store, provider, account, key);
    }
    let registry = Registry::new(store);
    let method = registry
        .all()
        .iter()
        .find(|p| p.id().to_string() == provider)
        .map(|p| p.capabilities().auth)
        .ok_or_else(|| unknown_provider_error(provider, &registry))?;

    if endpoint.is_some() && provider != goat_provider_hosted::QWEN {
        return Err(eyre!("--endpoint is only supported for qwen"));
    }
    if key.is_some() && matches!(method, AuthMethod::OAuth) {
        return Err(eyre!(
            "--key is not supported for OAuth-only provider {provider}"
        ));
    }
    if endpoint.is_some() && matches!(method, AuthMethod::OAuth) {
        return Err(eyre!(
            "--endpoint is not supported for OAuth-only provider {provider}"
        ));
    }

    let credential_key = CredentialKey::model(provider, account);

    let use_oauth = match method {
        AuthMethod::None => {
            println!("{provider} requires no login");
            return Ok(());
        }
        AuthMethod::ApiKey => false,
        AuthMethod::OAuth => true,
        AuthMethod::ApiKeyOrOAuth => key.is_none(),
    };

    if use_oauth {
        let (status, mut lines) = mpsc::channel::<String>(16);
        let printer = tokio::spawn(async move {
            while let Some(line) = lines.recv().await {
                println!("{line}");
            }
        });
        let tokens = registry
            .login(provider, status)
            .await
            .map_err(|err| eyre!(err))?;
        let _ = printer.await;
        store
            .store(&credential_key, Credential::OAuth(tokens))
            .map_err(|err| eyre!(err.to_string()))?;
    } else {
        let secret = match key {
            Some(key) => key,
            None => prompt(&format!("enter API key for {provider}: "))?,
        };
        let secret = secret.trim().to_owned();
        if secret.is_empty() {
            return Err(eyre!("no API key provided"));
        }
        let credential = if provider == goat_provider_hosted::QWEN {
            let endpoint = endpoint
                .or_else(|| std::env::var("QWEN_BASE_URL").ok())
                .unwrap_or_else(|| {
                    "https://dashscope-us.aliyuncs.com/compatible-mode/v1".to_owned()
                });
            let endpoint = goat_provider_hosted::validate_qwen_endpoint(&endpoint)
                .map_err(|err| eyre!(err))?;
            Credential::ApiKeyWithEndpoint {
                secret: SecretString::from(secret),
                endpoint,
            }
        } else {
            Credential::ApiKey(SecretString::from(secret))
        };
        store
            .store(&credential_key, credential)
            .map_err(|err| eyre!(err.to_string()))?;
    }

    println!("stored credential for {provider} ({account})");
    verify(store, provider, account).await;
    Ok(())
}

fn login_search(
    store: &CredentialStore,
    provider: &str,
    account: &str,
    key: Option<String>,
) -> color_eyre::Result<()> {
    match provider {
        "brave" | "tavily" => {}
        "duckduckgo" | "browser" | "searxng" => {
            println!("{provider} search accounts do not require secret credentials");
            return Ok(());
        }
        other => return Err(eyre!("unknown search provider: {other}")),
    }
    let secret = match key {
        Some(key) => key,
        None => prompt(&format!("enter API key for search {provider}: "))?,
    };
    let secret = secret.trim().to_owned();
    if secret.is_empty() {
        return Err(eyre!("no API key provided"));
    }
    let credential_key = CredentialKey::search(provider, account);
    store
        .store(
            &credential_key,
            Credential::ApiKey(SecretString::from(secret)),
        )
        .map_err(|err| eyre!(err.to_string()))?;
    println!("stored search credential for {provider} ({account})");
    Ok(())
}

async fn verify(store: &CredentialStore, provider: &str, account: &str) {
    let registry = Registry::load(store, account);
    let Some(provider) = registry.get(&ProviderId::from(provider)) else {
        return;
    };
    let (tx, mut rx) = mpsc::channel(32);
    let handle = provider.discover(tx);
    let mut count = 0usize;
    let collect = async {
        while rx.recv().await.is_some() {
            count += 1;
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(5), collect).await;
    handle.abort();
    if count > 0 {
        println!("verified: {count} models available");
    } else if provider.verifies_credentials() {
        println!("warning: could not verify credential (no models returned)");
    } else {
        println!(
            "stored but not verified: this provider uses a catalog-only model list; validation will happen on first request"
        );
    }
}

fn list_provider_lines(store: &CredentialStore) -> Vec<String> {
    let registry = Registry::new(store);
    let stored = store.entries();
    let mut lines = vec![format!(
        "{:<16}  {:<22}  {:<18}  {:<42}  {}",
        "provider", "auth", "status", "setup", "models"
    )];
    for provider in registry.all() {
        let id = provider.id().to_string();
        let caps = provider.capabilities();
        let metadata = provider.metadata();
        let accounts = provider_accounts(&stored, &id);
        lines.push(format!(
            "{:<16}  {:<22}  {:<18}  {:<42}  {}",
            id,
            auth_label(caps.auth),
            connection_status(caps.auth, metadata.env_var, &accounts),
            setup_hint(&id, caps.auth, metadata.env_var),
            model_preview(provider.catalog()),
        ));
    }
    lines
}

fn list_providers(store: &CredentialStore) {
    for line in list_provider_lines(store) {
        println!("{line}");
    }
    println!();
    println!("Use `goat provider info <provider>` for endpoint, OAuth, and validation details.");
    println!("Use `goat provider accounts` to show stored accounts only.");
}

fn provider_info(store: &CredentialStore, provider: &str) -> color_eyre::Result<()> {
    let registry = Registry::new(store);
    let target = registry
        .all()
        .iter()
        .find(|candidate| candidate.id().to_string() == provider)
        .ok_or_else(|| unknown_provider_error(provider, &registry))?;
    let stored = store.entries();
    let id = target.id().to_string();
    let caps = target.capabilities();
    let metadata = target.metadata();
    let accounts = provider_accounts(&stored, &id);
    println!("{id}");
    println!("  auth        {}", auth_label(caps.auth));
    println!(
        "  status      {}",
        connection_status(caps.auth, metadata.env_var, &accounts)
    );
    println!(
        "  accounts    {}",
        if accounts.is_empty() {
            "none".to_owned()
        } else {
            accounts
                .iter()
                .map(|(account, kind)| format!("{account} ({})", credential_kind_label(*kind)))
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    println!(
        "  env         {}",
        metadata.env_var.map_or("-".to_owned(), str::to_owned)
    );
    println!(
        "  endpoint    {}",
        metadata.endpoint.map_or("fixed".to_owned(), str::to_owned)
    );
    println!("  validation  {}", metadata.validation);
    println!(
        "  oauth       {}",
        metadata.oauth.map_or_else(
            || match caps.auth {
                AuthMethod::OAuth | AuthMethod::ApiKeyOrOAuth => "device code".to_owned(),
                AuthMethod::ApiKey | AuthMethod::None => "-".to_owned(),
            },
            str::to_owned,
        )
    );
    println!("  models      {}", model_preview(target.catalog()));
    println!();
    println!("setup");
    for line in setup_lines(&id, caps.auth, metadata.env_var) {
        println!("  {line}");
    }
    Ok(())
}

fn list_provider_accounts(store: &CredentialStore) {
    let entries: Vec<_> = store
        .entries()
        .into_iter()
        .filter(|(key, _)| key.service == CredentialService::Model)
        .collect();
    if entries.is_empty() {
        println!("no model provider accounts stored");
        println!("run `goat provider list` to see available providers");
        return;
    }
    println!("{:<16}  {:<16}  method", "provider", "account");
    for (key, kind) in entries {
        println!(
            "{:<16}  {:<16}  {}",
            key.provider,
            key.account,
            credential_kind_label(kind)
        );
    }
}

fn provider_accounts(
    stored: &[(CredentialKey, CredentialKind)],
    provider: &str,
) -> Vec<(String, CredentialKind)> {
    stored
        .iter()
        .filter(|(key, _)| key.service == CredentialService::Model && key.provider == provider)
        .map(|(key, kind)| (key.account.clone(), *kind))
        .collect()
}

fn connection_status(
    auth: AuthMethod,
    env_var: Option<&str>,
    accounts: &[(String, CredentialKind)],
) -> String {
    if !accounts.is_empty() {
        return if accounts.len() == 1 {
            format!("connected: {}", accounts[0].0)
        } else {
            format!("connected: {}", accounts.len())
        };
    }
    if let Some(var) = env_var
        && std::env::var(var).is_ok_and(|value| !value.is_empty())
    {
        return format!("env: {var}");
    }
    if matches!(auth, AuthMethod::None) {
        "local".to_owned()
    } else {
        "not connected".to_owned()
    }
}

fn setup_hint(id: &str, auth: AuthMethod, env_var: Option<&str>) -> String {
    match id {
        goat_provider_hosted::KIMI => "OAuth users: goat provider login kimi-code".to_owned(),
        goat_provider_hosted::KIMI_CODE => "goat provider login kimi-code".to_owned(),
        goat_provider_hosted::QWEN => {
            "goat provider login qwen --endpoint ... --key ...".to_owned()
        }
        goat_provider_hosted::ZAI_CODING => "goat provider login zai-coding --key ...".to_owned(),
        _ => match auth {
            AuthMethod::None => "no login needed".to_owned(),
            AuthMethod::OAuth => format!("goat provider login {id}"),
            AuthMethod::ApiKey | AuthMethod::ApiKeyOrOAuth => env_var.map_or_else(
                || format!("goat provider login {id} --key ..."),
                |var| format!("set {var} or login --key ..."),
            ),
        },
    }
}

fn setup_lines(id: &str, auth: AuthMethod, env_var: Option<&str>) -> Vec<String> {
    match id {
        goat_provider_hosted::KIMI => vec![
            "Kimi Platform API key provider.".to_owned(),
            "For Kimi Code OAuth, use `goat provider login kimi-code`.".to_owned(),
            "API-key setup: `goat provider login kimi --key sk-...`.".to_owned(),
        ],
        goat_provider_hosted::KIMI_CODE => vec![
            "Kimi Code OAuth device-code login.".to_owned(),
            "Run `goat provider login kimi-code`, open the URL, and enter the code.".to_owned(),
        ],
        goat_provider_hosted::QWEN => vec![
            "Qwen DashScope API-key provider.".to_owned(),
            "Default endpoint: https://dashscope-us.aliyuncs.com/compatible-mode/v1".to_owned(),
            "Non-US workspaces: `goat provider login qwen --endpoint <url> --key sk-...`."
                .to_owned(),
            "Qwen OAuth enrollment is discontinued upstream.".to_owned(),
        ],
        goat_provider_hosted::ZAI_CODING => vec![
            "Z.AI Coding Plan API-key provider.".to_owned(),
            "Use `ZAI_CODING_API_KEY` or `goat provider login zai-coding --key sk-...`.".to_owned(),
            "This is not OAuth and does not reuse the standard `zai` credential.".to_owned(),
        ],
        _ => match auth {
            AuthMethod::None => {
                vec!["No login required. Make sure the local server is running.".to_owned()]
            }
            AuthMethod::OAuth => vec![format!(
                "Run `goat provider login {id}` for device-code login."
            )],
            AuthMethod::ApiKey => vec![env_var.map_or_else(
                || format!("Run `goat provider login {id} --key sk-...`."),
                |var| format!("Set `{var}` or run `goat provider login {id} --key sk-...`."),
            )],
            AuthMethod::ApiKeyOrOAuth => vec![
                format!("Run `goat provider login {id}` for OAuth device-code login."),
                format!("Run `goat provider login {id} --key sk-...` to store an API key."),
            ],
        },
    }
}

fn model_preview(catalog: &[&str]) -> String {
    if catalog.is_empty() {
        return "discovered live".to_owned();
    }
    let shown = catalog
        .iter()
        .take(3)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if catalog.len() > 3 {
        format!("{shown}, …")
    } else {
        shown
    }
}

fn credential_kind_label(kind: CredentialKind) -> &'static str {
    match kind {
        CredentialKind::ApiKey => "api key",
        CredentialKind::OAuth => "oauth",
    }
}

fn auth_label(auth: AuthMethod) -> &'static str {
    match auth {
        AuthMethod::None => "none",
        AuthMethod::ApiKey => "api key",
        AuthMethod::OAuth => "device code",
        AuthMethod::ApiKeyOrOAuth => "api key or device code",
    }
}

fn unknown_provider_error(provider: &str, registry: &Registry) -> color_eyre::Report {
    let mut ids: Vec<String> = registry.all().iter().map(|p| p.id().to_string()).collect();
    ids.sort();
    let suggestions = closest_provider_ids(provider, &ids);
    if suggestions.is_empty() {
        eyre!("unknown provider: {provider}. run `goat provider list` to see available providers")
    } else {
        eyre!(
            "unknown provider: {provider}. did you mean {}? run `goat provider list` to see available providers",
            suggestions.join(", ")
        )
    }
}

fn closest_provider_ids(provider: &str, ids: &[String]) -> Vec<String> {
    ids.iter()
        .filter(|id| {
            id.contains(provider)
                || provider.contains(id.as_str())
                || id.chars().next() == provider.chars().next()
        })
        .take(3)
        .cloned()
        .collect()
}

fn list(store: &CredentialStore, service: Option<CredentialService>) {
    let entries: Vec<_> = store
        .entries()
        .into_iter()
        .filter(|(key, _)| service.is_none_or(|service| key.service == service))
        .collect();
    if entries.is_empty() {
        println!("no credentials stored");
        return;
    }
    for (key, kind) in entries {
        let kind = credential_kind_label(kind);
        println!(
            "{}/{}  {}/{}",
            service_name(key.service),
            key.provider,
            key.account,
            kind
        );
    }
}

fn logout(
    store: &CredentialStore,
    provider: &str,
    account: &str,
    service: CredentialService,
) -> color_eyre::Result<()> {
    let key = match service {
        CredentialService::Model => CredentialKey::model(provider, account),
        CredentialService::Search => CredentialKey::search(provider, account),
    };
    if store.remove(&key).map_err(|err| eyre!(err.to_string()))? {
        println!("disconnected {provider} ({account})");
    } else if service == CredentialService::Model {
        println!("no stored account found for {provider} ({account})");
        println!("run `goat provider accounts` to see stored provider accounts");
    } else {
        println!("no credential found for {provider} ({account})");
    }
    Ok(())
}

fn service_name(service: CredentialService) -> &'static str {
    match service {
        CredentialService::Model => "model",
        CredentialService::Search => "search",
    }
}

fn prompt(message: &str) -> color_eyre::Result<String> {
    print!("{message}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    #[test]
    fn provider_list_output_discovers_providers() {
        let store = goat_auth::CredentialStore::new(
            std::env::temp_dir().join("goat-code-provider-list-test.json"),
        );
        let lines = super::list_provider_lines(&store);
        assert!(lines[0].contains("provider"));
        assert!(lines.iter().any(|line| line.contains("openrouter")));
        assert!(lines.iter().any(|line| line.contains("kimi-code")));
        assert!(lines.iter().any(|line| line.contains("zai-coding")));
        assert!(!lines.iter().any(|line| line.contains("validation:")));
        assert!(!lines.iter().any(|line| line.contains("endpoint:")));
    }

    #[test]
    fn provider_accounts_output_data_is_grouped() {
        let rows = super::provider_accounts(
            &[(
                goat_auth::CredentialKey::model("kimi-code", "default"),
                goat_auth::CredentialKind::OAuth,
            )],
            "kimi-code",
        );
        assert_eq!(
            rows,
            vec![("default".to_owned(), goat_auth::CredentialKind::OAuth)]
        );
    }

    #[test]
    fn provider_info_unknown_suggests_list() {
        let store = goat_auth::CredentialStore::new(
            std::env::temp_dir().join("goat-code-provider-info-test.json"),
        );
        let error = super::provider_info(&store, "kim-code")
            .unwrap_err()
            .to_string();
        assert!(error.contains("goat provider list"));
    }

    #[test]
    fn unknown_provider_suggests_list() {
        let store = goat_auth::CredentialStore::new(
            std::env::temp_dir().join("goat-code-provider-unknown-test.json"),
        );
        let registry = goat_providers::Registry::new(&store);
        let error = super::unknown_provider_error("openruter", &registry).to_string();
        assert!(error.contains("goat provider list"));
    }
}
