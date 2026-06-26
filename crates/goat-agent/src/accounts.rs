use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use goat_auth::{
    Credential, CredentialKey, CredentialKind, CredentialStore, SecretString, TokenSet,
};
use goat_protocol::{
    AccountChoice, AccountEntry, AccountInfo, AuthMethod, Effort, Event, LoginCredential,
    LoginProvider, ModelEntry, ModelTarget, NotifyKind,
};
use goat_provider::Provider;
use goat_providers::{DEFAULT_ACCOUNT, Registry};
use goat_store::Store;
use tokio::sync::mpsc;

use crate::Ctx;

pub(crate) async fn restore_target(
    store: &Store,
    credentials: &CredentialStore,
    cwd: &std::path::Path,
) -> Option<ModelTarget> {
    let cwd = cwd.display().to_string();
    let thread = store.latest_thread_in(cwd).await.ok().flatten()?;
    let provider = Registry::load(credentials, &thread.account)
        .get(&goat_provider::ProviderId::from(thread.provider.as_str()))?;
    if !provider.authenticated() {
        return None;
    }
    Some(ModelTarget {
        provider: thread.provider,
        model: thread.model,
        account: thread.account,
        effort: thread.effort.as_deref().and_then(Effort::parse),
    })
}

pub(crate) async fn emit_accounts_changed(
    events: &mpsc::Sender<Event>,
    registry: &Registry,
    credentials: &CredentialStore,
) {
    let _ = events
        .send(Event::AccountsChanged {
            providers: build_account_entries(registry, credentials),
        })
        .await;
}

pub(crate) async fn handle_remove_account(
    provider: String,
    name: String,
    credentials: &CredentialStore,
    registry: &mut Registry,
    events: &mpsc::Sender<Event>,
) {
    let key = CredentialKey {
        provider: provider.clone(),
        account: name.clone(),
    };
    if let Err(err) = credentials.remove(&key) {
        tracing::warn!(%err, "failed to remove account");
    }
    *registry = Registry::new(credentials);
    let entries = discover_ready(registry, credentials).await;
    let _ = events.send(Event::ModelListChanged { entries }).await;
    emit_accounts_changed(events, registry, credentials).await;
}

pub(crate) fn build_account_entries(
    registry: &Registry,
    credentials: &CredentialStore,
) -> Vec<AccountEntry> {
    let stored = credentials.entries();
    registry
        .all()
        .iter()
        .map(|p| {
            let provider_id = p.id().to_string();
            let auth_method = p.capabilities().auth;
            let is_local = matches!(auth_method, AuthMethod::None);
            let accounts = stored
                .iter()
                .filter(|(key, _)| key.provider == provider_id)
                .map(|(key, kind)| AccountInfo {
                    name: key.account.clone(),
                    method: match kind {
                        CredentialKind::ApiKey => AuthMethod::ApiKey,
                        CredentialKind::OAuth => AuthMethod::OAuth,
                    },
                })
                .collect();
            AccountEntry {
                display_name: provider_id.clone(),
                provider: provider_id,
                accounts,
                local: is_local,
                login: auth_method,
            }
        })
        .collect()
}

pub(crate) async fn announce_startup(
    events: &mpsc::Sender<Event>,
    registry: &Registry,
    credentials: &CredentialStore,
    target: Option<&ModelTarget>,
) {
    let _ = events
        .send(Event::LoginProviders {
            providers: registry
                .all()
                .iter()
                .map(|p| LoginProvider {
                    id: p.id().to_string(),
                    method: p.capabilities().auth,
                })
                .collect(),
        })
        .await;
    let _ = events
        .send(Event::AccountsChanged {
            providers: build_account_entries(registry, credentials),
        })
        .await;
    let _ = events
        .send(Event::ModelListChanged {
            entries: catalog_only(registry, credentials),
        })
        .await;
    if let Some(selected) = target {
        let _ = events
            .send(Event::ModelSelected {
                target: selected.clone(),
            })
            .await;
    }
    let providers = provider_accounts(registry, credentials);
    let bg_events = events.clone();
    tokio::spawn(async move {
        let entries = discover_entries(providers).await;
        let _ = bg_events.send(Event::ModelListChanged { entries }).await;
    });
}

pub(crate) struct LoginCtx<'a> {
    pub(crate) credentials: &'a CredentialStore,
    pub(crate) registry: &'a mut Registry,
    pub(crate) events: &'a mpsc::Sender<Event>,
}

async fn login_succeeded(provider: &str, events: &mpsc::Sender<Event>) {
    let _ = events
        .send(Event::Notify {
            kind: NotifyKind::Success,
            message: format!("{provider} connected"),
        })
        .await;
    let _ = events
        .send(Event::LoginStatus {
            provider: provider.to_owned(),
            message: String::new(),
            done: true,
            ok: true,
        })
        .await;
}

async fn login_failed(provider: &str, events: &mpsc::Sender<Event>, message: String) {
    let _ = events
        .send(Event::LoginStatus {
            provider: provider.to_owned(),
            message,
            done: true,
            ok: false,
        })
        .await;
}

async fn run_self_oauth(
    provider: &str,
    events: &mpsc::Sender<Event>,
    registry: &Registry,
) -> Result<TokenSet, String> {
    let (status_tx, mut status_rx) = mpsc::channel::<String>(8);
    let status_provider = provider.to_owned();
    let status_events = events.clone();
    let forwarder = tokio::spawn(async move {
        while let Some(message) = status_rx.recv().await {
            let _ = status_events
                .send(Event::LoginStatus {
                    provider: status_provider.clone(),
                    message,
                    done: false,
                    ok: false,
                })
                .await;
        }
    });
    let result = registry.login(provider, status_tx).await;
    let _ = forwarder.await;
    result
}

async fn finalize_login(
    ctx: LoginCtx<'_>,
    provider: String,
    name: String,
    key: CredentialKey,
    resolved: Credential,
) {
    if let Err(message) = ctx
        .credentials
        .store(&key, resolved)
        .map_err(|err| err.to_string())
    {
        login_failed(&provider, ctx.events, message).await;
        emit_accounts_changed(ctx.events, ctx.registry, ctx.credentials).await;
        return;
    }
    *ctx.registry = Registry::new(ctx.credentials);
    if let Err(message) = validate_stored(ctx.credentials, &provider, &name).await {
        let _ = ctx.credentials.remove(&key);
        *ctx.registry = Registry::new(ctx.credentials);
        login_failed(&provider, ctx.events, message).await;
        emit_accounts_changed(ctx.events, ctx.registry, ctx.credentials).await;
        return;
    }
    let entries = discover_ready(ctx.registry, ctx.credentials).await;
    let _ = ctx.events.send(Event::ModelListChanged { entries }).await;
    login_succeeded(&provider, ctx.events).await;
    emit_accounts_changed(ctx.events, ctx.registry, ctx.credentials).await;
}

async fn validate_stored(
    credentials: &CredentialStore,
    provider: &str,
    name: &str,
) -> Result<(), String> {
    match Registry::load(credentials, name).get(&goat_provider::ProviderId::from(provider)) {
        Some(target) => target
            .validate()
            .await
            .unwrap_or_else(|err| Err(err.to_string())),
        None => Err("unknown provider".to_owned()),
    }
}

pub(crate) async fn handle_login(
    ctx: LoginCtx<'_>,
    provider: String,
    name: String,
    credential: LoginCredential,
    dedup: bool,
) {
    let key = CredentialKey {
        provider: provider.clone(),
        account: name.clone(),
    };
    if dedup
        && ctx
            .credentials
            .entries()
            .iter()
            .any(|(stored, _)| stored == &key)
    {
        login_failed(
            &provider,
            ctx.events,
            format!("account '{name}' already exists"),
        )
        .await;
        return;
    }
    let resolved = match credential {
        LoginCredential::ApiKey { key: secret } => Credential::ApiKey(SecretString::from(secret)),
        LoginCredential::OAuth {} => {
            match run_self_oauth(&provider, ctx.events, ctx.registry).await {
                Ok(tokens) => Credential::OAuth(tokens),
                Err(message) => {
                    login_failed(&provider, ctx.events, message).await;
                    emit_accounts_changed(ctx.events, ctx.registry, ctx.credentials).await;
                    return;
                }
            }
        }
    };
    finalize_login(ctx, provider, name, key, resolved).await;
}

fn account_names_for(credentials: &CredentialStore, provider_id: &str) -> Vec<String> {
    credentials
        .entries()
        .into_iter()
        .filter(|(key, _)| key.provider == provider_id)
        .map(|(key, _)| key.account)
        .collect()
}

fn accounts_for_provider(
    credentials: &CredentialStore,
    provider: &dyn goat_provider::Provider,
) -> Option<Vec<String>> {
    let is_local = matches!(provider.capabilities().auth, AuthMethod::None);
    let stored = account_names_for(credentials, &provider.id().to_string());
    if is_local {
        if stored.is_empty() {
            return Some(vec![DEFAULT_ACCOUNT.to_owned()]);
        }
        return Some(stored);
    }
    if !stored.is_empty() {
        return Some(stored);
    }
    if provider.authenticated() {
        return Some(vec![DEFAULT_ACCOUNT.to_owned()]);
    }
    None
}

fn model_entry(
    provider_id: &str,
    model: &str,
    accounts: &[String],
    efforts: Vec<Effort>,
    context_window: Option<u32>,
    supports_images: bool,
) -> ModelEntry {
    ModelEntry {
        provider: provider_id.to_owned(),
        model: model.to_owned(),
        context_window,
        supports_images,
        efforts,
        accounts: accounts
            .iter()
            .map(|account| AccountChoice {
                id: account.clone(),
                display: account.clone(),
                target: ModelTarget {
                    provider: provider_id.to_owned(),
                    model: model.to_owned(),
                    account: account.clone(),
                    effort: None,
                },
            })
            .collect(),
    }
}

fn catalog_only(registry: &Registry, credentials: &CredentialStore) -> Vec<ModelEntry> {
    let mut entries = Vec::new();
    for provider in registry.all() {
        let Some(accounts) = accounts_for_provider(credentials, provider.as_ref()) else {
            continue;
        };
        let provider_id = provider.id().to_string();
        for &id in provider.catalog() {
            entries.push(model_entry(
                &provider_id,
                id,
                &accounts,
                provider.efforts(id),
                provider.context_window(id),
                provider.supports_images(id),
            ));
        }
    }
    entries
}

fn provider_accounts(
    registry: &Registry,
    credentials: &CredentialStore,
) -> Vec<(Arc<dyn Provider>, Vec<String>)> {
    registry
        .all()
        .iter()
        .filter_map(|provider| {
            accounts_for_provider(credentials, provider.as_ref())
                .map(|accounts| (Arc::clone(provider), accounts))
        })
        .collect()
}

async fn discover_provider(provider: Arc<dyn Provider>, accounts: Vec<String>) -> Vec<ModelEntry> {
    let provider_id = provider.id().to_string();
    let (tx, mut rx) = mpsc::channel(32);
    let handle = provider.discover(tx);
    let mut discovered = Vec::new();
    let collect = async {
        while let Some(info) = rx.recv().await {
            discovered.push(info);
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(3), collect).await;
    handle.abort();

    let catalog = provider.catalog();
    let catalog_ids: HashSet<&str> = catalog.iter().copied().collect();

    let mut entries = Vec::new();
    for &id in catalog {
        entries.push(model_entry(
            &provider_id,
            id,
            &accounts,
            provider.efforts(id),
            provider.context_window(id),
            provider.supports_images(id),
        ));
    }
    for info in discovered {
        if catalog_ids.contains(info.id.as_str()) {
            continue;
        }
        let efforts = provider.efforts(&info.id);
        let ctx_win = provider.context_window(&info.id);
        entries.push(model_entry(
            &provider_id,
            &info.id,
            &accounts,
            efforts,
            ctx_win,
            info.supports_images || provider.supports_images(&info.id),
        ));
    }
    entries
}

async fn discover_entries(providers: Vec<(Arc<dyn Provider>, Vec<String>)>) -> Vec<ModelEntry> {
    futures::future::join_all(
        providers
            .into_iter()
            .map(|(provider, accounts)| discover_provider(provider, accounts)),
    )
    .await
    .into_iter()
    .flatten()
    .collect()
}

pub(crate) async fn discover_ready(
    registry: &Registry,
    credentials: &CredentialStore,
) -> Vec<ModelEntry> {
    discover_entries(provider_accounts(registry, credentials)).await
}

pub(crate) fn clear_account_registries(cache: &std::sync::Mutex<HashMap<String, Arc<Registry>>>) {
    cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

pub(crate) fn provider_for(
    ctx: &Ctx<'_>,
    account: &str,
    id: &goat_provider::ProviderId,
) -> Option<Arc<dyn Provider>> {
    if account == DEFAULT_ACCOUNT {
        return ctx.registry.get(id);
    }
    let mut cache = ctx
        .account_registries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry(account.to_owned())
        .or_insert_with(|| Arc::new(Registry::load(ctx.credentials, account)))
        .get(id)
}
