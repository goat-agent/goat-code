use std::io::{IsTerminal, Write};
use std::time::Duration;

use color_eyre::eyre::{Result, eyre};
use crossterm::{
    cursor,
    event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, ClearType},
};
use goat_auth::{
    Credential, CredentialKey, CredentialKind, CredentialService, CredentialStore, SecretString,
};
use goat_provider::{AuthMethod, ProviderId, ProviderMetadata};
use goat_providers::{DEFAULT_ACCOUNT, Registry};
use tokio::sync::mpsc;

use crate::{
    cli::ProviderCommand,
    style::{ColorMode, Palette, print_row, truncate_to_width},
};

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
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(eyre!(
            "provider is required when stdin/stdout is not a terminal"
        ));
    }
    let registry = Registry::new(store);
    let stored = store.entries();
    let choices = registry
        .all()
        .iter()
        .filter(|provider| !matches!(provider.capabilities().auth, AuthMethod::None))
        .map(|provider| {
            let id = provider.id().to_string();
            let caps = provider.capabilities();
            let metadata = provider.metadata();
            let accounts = provider_accounts(&stored, &id);
            let status = connection_status(caps.auth, metadata.env_var, &accounts);
            ProviderChoice { id, status }
        })
        .collect::<Vec<_>>();
    if choices.is_empty() {
        return Err(eyre!("no login-capable providers available"));
    }
    ProviderPicker::new(choices).pick()
}

struct ProviderChoice {
    id: String,
    status: ConnectionStatus,
}

struct ProviderPicker {
    choices: Vec<ProviderChoice>,
    selected: usize,
}

impl ProviderPicker {
    fn new(choices: Vec<ProviderChoice>) -> Self {
        Self {
            choices,
            selected: 0,
        }
    }

    fn pick(mut self) -> Result<String> {
        terminal::enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(
            stdout,
            terminal::EnterAlternateScreen,
            cursor::Hide,
            terminal::Clear(ClearType::All)
        )?;
        let result = self.pick_raw(&mut stdout);
        execute!(stdout, terminal::LeaveAlternateScreen, cursor::Show)?;
        terminal::disable_raw_mode()?;
        result
    }

    fn pick_raw(&mut self, stdout: &mut std::io::Stdout) -> Result<String> {
        loop {
            self.render(stdout)?;
            if let TermEvent::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Up => self.move_up(),
                    KeyCode::Down => self.move_down(),
                    KeyCode::Char('k') if key.modifiers.is_empty() => self.move_up(),
                    KeyCode::Char('j') if key.modifiers.is_empty() => self.move_down(),
                    KeyCode::Enter => {
                        return Ok(self.choices[self.selected].id.clone());
                    }
                    KeyCode::Esc => return Err(eyre!("provider login cancelled")),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Err(eyre!("provider login cancelled"));
                    }
                    _ => {}
                }
            }
        }
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        self.selected = (self.selected + 1).min(self.choices.len().saturating_sub(1));
    }

    fn render(&self, stdout: &mut std::io::Stdout) -> Result<()> {
        execute!(
            stdout,
            cursor::MoveTo(0, 0),
            terminal::Clear(ClearType::All)
        )?;
        let color = ColorMode::detect();
        writeln!(stdout)?;
        writeln!(
            stdout,
            "  {}",
            color.paint("select provider", Palette::Muted)
        )?;
        writeln!(stdout)?;
        writeln!(
            stdout,
            "  {} {}",
            color.cell("provider", Palette::Muted, PROVIDER_WIDTH),
            color.cell("status", Palette::Muted, STATUS_WIDTH)
        )?;
        for (index, choice) in self.choices.iter().enumerate() {
            let marker = if index == self.selected { "›" } else { " " };
            writeln!(
                stdout,
                "  {}",
                format_provider_row(color, marker, &choice.id, &choice.status, Palette::Provider,)
            )?;
        }
        writeln!(stdout)?;
        writeln!(
            stdout,
            "  {}",
            color.paint("↑↓ choose   ↵ continue   esc cancel", Palette::Muted)
        )?;
        stdout.flush()?;
        Ok(())
    }
}

fn format_provider_row(
    color: ColorMode,
    marker: &str,
    id: &str,
    status: &ConnectionStatus,
    id_palette: Palette,
) -> String {
    format!(
        "{} {} {}",
        color.paint(marker, id_palette),
        color.cell(id, id_palette, PROVIDER_WIDTH),
        color.cell(status.compact_label(), status.palette(), STATUS_WIDTH)
    )
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
        return Err(eyre!("--endpoint is not supported for provider {provider}"));
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
        let credential = api_key_credential(secret, endpoint, metadata)?;
        store
            .store(&credential_key, credential)
            .map_err(|err| eyre!(err.to_string()))?;
    }

    println!("stored credential for {provider} ({account})");
    verify(store, provider, account).await;
    Ok(())
}

fn api_key_credential(
    secret: String,
    endpoint: Option<String>,
    metadata: ProviderMetadata,
) -> color_eyre::Result<Credential> {
    let Some(endpoint_metadata) = metadata.login_endpoint else {
        return Ok(Credential::ApiKey(SecretString::from(secret)));
    };
    let endpoint = endpoint
        .or_else(|| {
            endpoint_metadata
                .env_var
                .and_then(|env_var| std::env::var(env_var).ok())
        })
        .or_else(|| endpoint_metadata.default.map(str::to_owned))
        .ok_or_else(|| eyre!("endpoint is required for this provider"))?;
    let endpoint = if let Some(validate) = endpoint_metadata.validate {
        validate(&endpoint).map_err(|err| eyre!(err))?
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
        println!("verified: {count} models available");
    } else if provider.verifies_credentials() {
        println!("warning: could not verify credential (no models returned)");
    } else {
        println!(
            "stored but not verified: this provider uses a catalog-only model list; validation will happen on first request"
        );
    }
}

const PROVIDER_WIDTH: usize = 15;
const STATUS_WIDTH: usize = 10;
const ACCOUNT_WIDTH: usize = 22;

#[cfg(test)]
fn list_provider_lines(store: &CredentialStore) -> Vec<String> {
    let color = ColorMode::detect();
    list_provider_lines_with_color(store, color)
}

fn list_provider_lines_with_color(store: &CredentialStore, color: ColorMode) -> Vec<String> {
    let registry = Registry::new(store);
    let stored = store.entries();
    let mut lines = vec![format!(
        "  {} {} {}",
        color.cell("provider", Palette::Muted, PROVIDER_WIDTH),
        color.cell("status", Palette::Muted, STATUS_WIDTH),
        color.paint("account", Palette::Muted)
    )];
    for provider in registry.all() {
        let id = provider.id().to_string();
        let caps = provider.capabilities();
        let metadata = provider.metadata();
        let accounts = provider_accounts(&stored, &id);
        let status = connection_status(caps.auth, metadata.env_var, &accounts);
        let account = account_preview(&accounts);
        let line = if account.is_empty() {
            format!(
                "  {}",
                format_provider_row(color, status.icon(), &id, &status, Palette::Provider)
            )
        } else {
            format!(
                "  {} {}",
                format_provider_row(color, status.icon(), &id, &status, Palette::Provider),
                color.paint(account, Palette::Value)
            )
        };
        lines.push(line);
    }
    lines
}

fn list_providers(store: &CredentialStore) {
    let color = ColorMode::detect();
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
        println!("run `goat provider list` to see stored provider accounts");
    } else {
        println!("no credential found for {provider} ({account})");
    }
    Ok(())
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
    fn provider_row_columns_align() {
        let status = super::ConnectionStatus {
            kind: super::ConnectionKind::Disconnected,
            label: "not connected".to_owned(),
        };
        let row = super::format_provider_row(
            crate::style::ColorMode::Plain,
            "›",
            "openrouter",
            &status,
            crate::style::Palette::Provider,
        );
        assert!(row.starts_with("› "));
        assert!(row.contains("openrouter"));
        assert!(row.contains("missing"));
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
