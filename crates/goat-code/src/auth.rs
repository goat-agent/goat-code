use std::io::Write;
use std::time::Duration;

use color_eyre::eyre::eyre;
use goat_auth::{CredentialKey, CredentialKind, CredentialStore, ResolvedCredential, SecretString};
use goat_provider::{AuthMethod, ProviderId};
use goat_providers::{DEFAULT_ACCOUNT, Registry};
use tokio::sync::mpsc;

use crate::cli::AuthCommand;

pub async fn run(command: AuthCommand) -> color_eyre::Result<()> {
    let path = goat_config::auth_path().ok_or_else(|| eyre!("could not resolve ~/.goat-code"))?;
    let store = CredentialStore::new(path);
    match command {
        AuthCommand::Login {
            provider,
            account,
            key,
        } => {
            let account = account.as_deref().unwrap_or(DEFAULT_ACCOUNT);
            login(&store, &provider, account, key).await
        }
        AuthCommand::List => {
            list(&store);
            Ok(())
        }
        AuthCommand::Logout { provider, account } => {
            let account = account.as_deref().unwrap_or(DEFAULT_ACCOUNT);
            logout(&store, &provider, account)
        }
    }
}

async fn login(
    store: &CredentialStore,
    provider: &str,
    account: &str,
    key: Option<String>,
) -> color_eyre::Result<()> {
    let method = Registry::builtin(store)
        .login_providers()
        .into_iter()
        .find(|(id, _)| id == provider)
        .map(|(_, method)| method)
        .ok_or_else(|| eyre!("unknown provider: {provider}"))?;

    let credential_key = CredentialKey {
        provider: provider.to_owned(),
        account: account.to_owned(),
    };

    match method {
        AuthMethod::None => {
            println!("{provider} requires no login");
            return Ok(());
        }
        AuthMethod::ApiKey => {
            let secret = match key {
                Some(key) => key,
                None => prompt(&format!("enter API key for {provider}: "))?,
            };
            let secret = secret.trim().to_owned();
            if secret.is_empty() {
                return Err(eyre!("no API key provided"));
            }
            store
                .store(
                    &credential_key,
                    ResolvedCredential::ApiKey(SecretString::from(secret)),
                )
                .map_err(|err| eyre!(err.to_string()))?;
        }
        AuthMethod::OAuth => {
            let (status, mut lines) = mpsc::channel::<String>(16);
            let printer = tokio::spawn(async move {
                while let Some(line) = lines.recv().await {
                    println!("{line}");
                }
            });
            let tokens = goat_providers::oauth_login(provider, &status)
                .await
                .map_err(|err| eyre!(err))?;
            drop(status);
            let _ = printer.await;
            store
                .store(&credential_key, ResolvedCredential::OAuth(tokens))
                .map_err(|err| eyre!(err.to_string()))?;
        }
    }

    println!("stored credential for {provider} ({account})");
    verify(store, provider, account).await;
    Ok(())
}

async fn verify(store: &CredentialStore, provider: &str, account: &str) {
    let registry = Registry::for_account(store, account);
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
    } else {
        println!("warning: could not verify credential (no models returned)");
    }
}

fn list(store: &CredentialStore) {
    let entries = store.entries();
    if entries.is_empty() {
        println!("no credentials stored");
        return;
    }
    for (key, kind) in entries {
        let kind = match kind {
            CredentialKind::ApiKey => "api_key",
            CredentialKind::OAuth => "oauth",
        };
        println!("{}/{}  {kind}", key.provider, key.account);
    }
}

fn logout(store: &CredentialStore, provider: &str, account: &str) -> color_eyre::Result<()> {
    let key = CredentialKey {
        provider: provider.to_owned(),
        account: account.to_owned(),
    };
    if store.remove(&key).map_err(|err| eyre!(err.to_string()))? {
        println!("removed credential for {provider} ({account})");
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
