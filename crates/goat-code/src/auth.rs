use std::io::IsTerminal;
use std::time::Duration;

use color_eyre::eyre::Result;
use goat_auth::{
    Credential, CredentialKey, CredentialKind, CredentialService, CredentialStore, SecretString,
};
use goat_provider::{AuthMethod, LoginEndpointMetadata, ProviderId, ProviderMetadata};
use goat_providers::{DEFAULT_ACCOUNT, Registry};
use tokio::sync::mpsc;

use crate::{
    cli::ProviderCommand,
    cli_ui::{self, AuthPick, ProviderPick},
    provider_table,
    style::{ColorMode, Palette, print_row, truncate_to_width},
};

pub async fn run_provider(command: ProviderCommand) -> color_eyre::Result<()> {
    let path =
        goat_config::auth_path().ok_or_else(|| cli_ui::report(goat_config::HOME_NOT_FOUND))?;
    let store = CredentialStore::new(path);
    match command {
        ProviderCommand::Login {
            provider,
            account,
            key,
            endpoint,
        } => {
            let provider = match provider {
                Some(provider) => provider,
                None => pick_login_provider(&store)?,
            };
            let account = account.as_deref().unwrap_or(DEFAULT_ACCOUNT);
            login(&store, &provider, account, key, endpoint).await
        }
        ProviderCommand::List => {
            list_providers(&store);
            Ok(())
        }
        ProviderCommand::Info { provider } => provider_info(&store, &provider),
        ProviderCommand::Logout { provider, account } => {
            logout(&store, &provider, &account, CredentialService::Model)
        }
    }
}

fn pick_login_provider(store: &CredentialStore) -> Result<String> {
    let registry = Registry::new(store);
    let stored = store.entries();
    let choices = login_provider_choices(&registry, &stored);
    if choices.is_empty() {
        return Err(cli_ui::report("no login-capable providers available"));
    }
    let items = choices
        .iter()
        .map(|choice| ProviderPick {
            id: choice.id.clone(),
            status: choice.status.compact_label().to_owned(),
            status_palette: choice.status.palette(),
        })
        .collect::<Vec<_>>();
    let index = cli_ui::pick_provider(&items)?;
    Ok(choices[index].id.clone())
}

fn login_provider_choices(
    registry: &Registry,
    stored: &[(CredentialKey, CredentialKind)],
) -> Vec<LoginProviderChoice> {
    registry
        .all()
        .iter()
        .filter(|provider| !matches!(provider.capabilities().auth, AuthMethod::None))
        .map(|provider| {
            let id = provider.id().to_string();
            let caps = provider.capabilities();
            let metadata = provider.metadata();
            let accounts = provider_accounts(stored, &id);
            let status = connection_status(caps.auth, metadata.env_var, &accounts);
            LoginProviderChoice { id, status }
        })
        .collect()
}

struct LoginProviderChoice {
    id: String,
    status: ConnectionStatus,
}

async fn login(
    store: &CredentialStore,
    provider: &str,
    account: &str,
    key: Option<String>,
    endpoint: Option<String>,
) -> color_eyre::Result<()> {
    let registry = Registry::new(store);
    let provider_handle = registry
        .all()
        .iter()
        .find(|p| p.id().to_string() == provider)
        .cloned()
        .ok_or_else(|| unknown_provider_error(provider, &registry))?;
    let method = provider_handle.capabilities().auth;
    let metadata = provider_handle.metadata();

    if endpoint.is_some() && metadata.login_endpoint.is_none() {
        return cli_ui::fail_hint(
            format!("--endpoint is not supported for provider {provider}"),
            "omit --endpoint for this provider",
        );
    }
    if key.is_some() && matches!(method, AuthMethod::OAuth) {
        return cli_ui::fail_hint(
            format!("--key is not supported for OAuth-only provider {provider}"),
            "run without --key to start device-code login",
        );
    }
    if endpoint.is_some() && matches!(method, AuthMethod::OAuth) {
        return cli_ui::fail_hint(
            format!("--endpoint is not supported for OAuth-only provider {provider}"),
            "omit --endpoint for this provider",
        );
    }

    let credential_key = CredentialKey::model(provider, account);

    if matches!(method, AuthMethod::None) {
        return Ok(());
    }

    let auth_pick = if key.is_some() {
        AuthPick::ApiKey
    } else {
        cli_ui::pick_auth_method(provider, method)?
    };

    match auth_pick {
        AuthPick::OAuth => {
            let (status, mut lines) = mpsc::channel::<String>(16);
            let printer = tokio::spawn(async move {
                while let Some(line) = lines.recv().await {
                    cli_ui::oauth_status(&line);
                }
            });
            let tokens = registry
                .login(provider, status)
                .await
                .map_err(cli_ui::report)?;
            let _ = printer.await;
            store
                .store(&credential_key, Credential::OAuth(tokens))
                .map_err(storage_error)?;
        }
        AuthPick::ApiKey => {
            let secret = match key {
                Some(key) => key,
                None => cli_ui::prompt_api_key(provider)?,
            };
            if secret.is_empty() {
                return cli_ui::fail("no API key provided");
            }
            let endpoint = resolve_login_endpoint(endpoint, metadata.login_endpoint)?;
            let credential = api_key_credential(secret, Some(endpoint), metadata)?;
            store
                .store(&credential_key, credential)
                .map_err(storage_error)?;
        }
    }

    cli_ui::success(&format!("stored credential for {provider} ({account})"));
    verify(store, provider, account).await;
    Ok(())
}

fn resolve_login_endpoint(
    endpoint: Option<String>,
    login_endpoint: Option<LoginEndpointMetadata>,
) -> color_eyre::Result<String> {
    let Some(endpoint_metadata) = login_endpoint else {
        return Ok(String::new());
    };
    let endpoint = endpoint
        .or_else(|| {
            endpoint_metadata
                .env_var
                .and_then(|env_var| std::env::var(env_var).ok())
        })
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let endpoint = match endpoint {
        Some(endpoint) => endpoint,
        None if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() => {
            cli_ui::prompt_endpoint(endpoint_metadata.default)?
        }
        None => endpoint_metadata
            .default
            .map(str::to_owned)
            .ok_or_else(|| {
                cli_ui::report_hint(
                    "endpoint is required for this provider",
                    "pass --endpoint or set the provider env var",
                )
            })?,
    };
    if endpoint.is_empty() {
        return Err(cli_ui::report_hint(
            "endpoint is required for this provider",
            "pass --endpoint or set the provider env var",
        ));
    }
    if let Some(validate) = endpoint_metadata.validate {
        validate(&endpoint).map_err(cli_ui::report)
    } else {
        Ok(endpoint)
    }
}

fn api_key_credential(
    secret: String,
    endpoint: Option<String>,
    metadata: ProviderMetadata,
) -> color_eyre::Result<Credential> {
    let Some(endpoint_metadata) = metadata.login_endpoint else {
        return Ok(Credential::ApiKey(SecretString::from(secret)));
    };
    let endpoint = endpoint.filter(|value| !value.is_empty()).ok_or_else(|| {
        cli_ui::report_hint(
            "endpoint is required for this provider",
            "pass --endpoint or set the provider env var",
        )
    })?;
    let endpoint = if let Some(validate) = endpoint_metadata.validate {
        validate(&endpoint).map_err(cli_ui::report)?
    } else {
        endpoint
    };
    Ok(Credential::ApiKeyWithEndpoint {
        secret: SecretString::from(secret),
        endpoint,
    })
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
        cli_ui::success(&format!("verified: {count} models"));
    } else if provider.verifies_credentials() {
        cli_ui::warning("could not verify credential");
    }
}

const ACCOUNT_WIDTH: usize = 22;

#[cfg(test)]
fn list_provider_lines(store: &CredentialStore) -> Vec<String> {
    let color = ColorMode::detect();
    list_provider_lines_with_color(store, color)
}

fn list_provider_lines_with_color(store: &CredentialStore, color: ColorMode) -> Vec<String> {
    let registry = Registry::new(store);
    let stored = store.entries();
    let mut lines = vec![provider_table::header(color, true)];
    for provider in registry.all() {
        let id = provider.id().to_string();
        let caps = provider.capabilities();
        let metadata = provider.metadata();
        let accounts = provider_accounts(&stored, &id);
        let status = connection_status(caps.auth, metadata.env_var, &accounts);
        let account = account_preview(&accounts);
        let account = (!account.is_empty()).then_some(account.as_str());
        lines.push(provider_table::row(
            color,
            status.icon(),
            status.palette(),
            &id,
            status.compact_label(),
            status.palette(),
            account,
        ));
    }
    lines
}

fn list_providers(store: &CredentialStore) {
    let color = ColorMode::detect();
    println!();
    for line in list_provider_lines_with_color(store, color) {
        println!("{line}");
    }
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
    let status = connection_status(caps.auth, metadata.env_var, &accounts);
    let color = ColorMode::detect();
    println!("{}", color.paint(&id, Palette::Provider));
    print_row(color, "status", status.label(), status.palette());
    print_row(color, "auth", auth_label(caps.auth), Palette::Value);
    print_row(
        color,
        "accounts",
        provider_account_details(&accounts),
        Palette::Value,
    );
    print_row(
        color,
        "env",
        metadata.env_var.map_or("-".to_owned(), str::to_owned),
        Palette::Value,
    );
    print_row(
        color,
        "endpoint",
        metadata.endpoint.map_or("fixed".to_owned(), str::to_owned),
        Palette::Value,
    );
    print_row(color, "validation", metadata.validation, Palette::Value);
    print_row(
        color,
        "oauth",
        metadata.oauth.map_or_else(
            || match caps.auth {
                AuthMethod::OAuth | AuthMethod::ApiKeyOrOAuth => "device code".to_owned(),
                AuthMethod::ApiKey | AuthMethod::None => "-".to_owned(),
            },
            str::to_owned,
        ),
        Palette::Value,
    );
    print_row(
        color,
        "models",
        model_preview(target.catalog()),
        Palette::Value,
    );
    println!();
    println!("{}", color.paint("setup", Palette::Muted));
    for line in provider_setup_lines(&id, caps.auth, metadata) {
        println!("  {}", color.paint(line, Palette::Value));
    }
    Ok(())
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

fn account_preview(accounts: &[(String, CredentialKind)]) -> String {
    match accounts {
        [] => String::new(),
        [(account, _)] => truncate_label(account, ACCOUNT_WIDTH),
        [(first, _), (second, _)] => truncate_label(&format!("{first}, {second}"), ACCOUNT_WIDTH),
        [(first, _), (second, _), rest @ ..] => {
            truncate_label(&format!("{first}, {second} +{}", rest.len()), ACCOUNT_WIDTH)
        }
    }
}

fn provider_account_details(accounts: &[(String, CredentialKind)]) -> String {
    if accounts.is_empty() {
        return "none".to_owned();
    }
    accounts
        .iter()
        .map(|(account, kind)| format!("{account} ({})", credential_kind_label(*kind)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn truncate_label(label: &str, width: usize) -> String {
    truncate_to_width(label, width)
}

struct ConnectionStatus {
    kind: ConnectionKind,
    label: String,
}

impl ConnectionStatus {
    fn icon(&self) -> &'static str {
        match self.kind {
            ConnectionKind::Connected | ConnectionKind::Env => "●",
            ConnectionKind::Local => "◆",
            ConnectionKind::Disconnected => "○",
        }
    }

    fn palette(&self) -> Palette {
        match self.kind {
            ConnectionKind::Connected => Palette::Success,
            ConnectionKind::Env => Palette::Info,
            ConnectionKind::Local => Palette::Local,
            ConnectionKind::Disconnected => Palette::Warning,
        }
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn compact_label(&self) -> &str {
        match self.kind {
            ConnectionKind::Connected => "connected",
            ConnectionKind::Env => "env",
            ConnectionKind::Local => "local",
            ConnectionKind::Disconnected => "missing",
        }
    }
}

enum ConnectionKind {
    Connected,
    Env,
    Local,
    Disconnected,
}

fn connection_status(
    auth: AuthMethod,
    env_var: Option<&str>,
    accounts: &[(String, CredentialKind)],
) -> ConnectionStatus {
    if !accounts.is_empty() {
        let label = if accounts.len() == 1 {
            format!("connected: {}", accounts[0].0)
        } else {
            format!("connected: {} accounts", accounts.len())
        };
        return ConnectionStatus {
            kind: ConnectionKind::Connected,
            label,
        };
    }
    if let Some(var) = env_var
        && std::env::var(var).is_ok_and(|value| !value.is_empty())
    {
        return ConnectionStatus {
            kind: ConnectionKind::Env,
            label: format!("env: {var}"),
        };
    }
    if matches!(auth, AuthMethod::None) {
        ConnectionStatus {
            kind: ConnectionKind::Local,
            label: "local".to_owned(),
        }
    } else {
        ConnectionStatus {
            kind: ConnectionKind::Disconnected,
            label: "not connected".to_owned(),
        }
    }
}

fn provider_setup_lines(id: &str, auth: AuthMethod, metadata: ProviderMetadata) -> Vec<String> {
    if !metadata.setup.is_empty() {
        return metadata.setup.iter().map(ToString::to_string).collect();
    }
    match auth {
        AuthMethod::None => {
            vec!["No login required. Make sure the local server is running.".to_owned()]
        }
        AuthMethod::OAuth => vec![format!(
            "Run `goat provider login {id}` for device-code login."
        )],
        AuthMethod::ApiKey => vec![metadata.env_var.map_or_else(
            || format!("Run `goat provider login {id} --key sk-...`."),
            |var| format!("Set `{var}` or run `goat provider login {id} --key sk-...`."),
        )],
        AuthMethod::ApiKeyOrOAuth => vec![
            format!("Run `goat provider login {id}` for OAuth device-code login."),
            format!("Run `goat provider login {id} --key sk-...` to store an API key."),
        ],
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
    let message = if suggestions.is_empty() {
        format!("unknown provider: {provider}")
    } else {
        format!(
            "unknown provider: {provider} · did you mean {}",
            suggestions.join(", ")
        )
    };
    cli_ui::report_hint(
        message,
        "run `goat provider list` to see available providers",
    )
}

fn storage_error(err: impl std::fmt::Display) -> color_eyre::Report {
    cli_ui::report_hint(
        format!("could not update credential store: {err}"),
        "check permissions on ~/.goat-code",
    )
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
    if store.remove(&key).map_err(storage_error)? {
        cli_ui::success(&format!("disconnected {provider} ({account})"));
    } else if service == CredentialService::Model {
        cli_ui::warning(&format!("no stored account for {provider} ({account})"));
    } else {
        cli_ui::warning(&format!("no credential found for {provider} ({account})"));
    }
    Ok(())
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
