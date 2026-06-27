use color_eyre::eyre::eyre;
use goat_config::{Config, SearchAccountConfig};

use crate::cli::SearchCommand;

pub fn run(command: SearchCommand) -> color_eyre::Result<()> {
    match command {
        SearchCommand::Add {
            provider,
            account,
            endpoint,
            engine,
            default,
        } => add(
            &provider,
            &account,
            endpoint.as_deref(),
            engine.as_deref(),
            default,
        ),
        SearchCommand::Default { target } => set_default(&target),
        SearchCommand::List => {
            list();
            Ok(())
        }
        SearchCommand::Remove { target } => remove(&target),
    }
}

fn add(
    provider: &str,
    account: &str,
    endpoint: Option<&str>,
    engine: Option<&str>,
    make_default: bool,
) -> color_eyre::Result<()> {
    let mut config = Config::load();
    let entry = match provider {
        "duckduckgo" => SearchAccountConfig::Duckduckgo {
            account: account.to_owned(),
        },
        "browser" => {
            let engine = engine.unwrap_or("duckduckgo");
            if engine != "duckduckgo" {
                return Err(eyre!("unsupported browser search engine: {engine}"));
            }
            SearchAccountConfig::Browser {
                account: account.to_owned(),
                engine: engine.to_owned(),
            }
        }
        "searxng" => SearchAccountConfig::Searxng {
            account: account.to_owned(),
            endpoint: endpoint
                .ok_or_else(|| eyre!("searxng requires --endpoint"))?
                .to_owned(),
        },
        "brave" => SearchAccountConfig::Brave {
            account: account.to_owned(),
        },
        "tavily" => SearchAccountConfig::Tavily {
            account: account.to_owned(),
        },
        other => return Err(eyre!("unknown search provider: {other}")),
    };
    let target = entry.target();
    config
        .search
        .accounts
        .retain(|existing| existing.target() != target);
    config.search.accounts.push(entry);
    if make_default || config.search.default_target.is_none() {
        config.search.default_target = Some(target.clone());
    }
    config.save()?;
    println!("registered search target {target}");
    if make_default {
        println!("default search target set to {target}");
    }
    Ok(())
}

fn set_default(target: &str) -> color_eyre::Result<()> {
    let mut config = Config::load();
    if !has_builtin_target(target)
        && !config
            .search
            .accounts
            .iter()
            .any(|account| account.target() == target)
    {
        return Err(eyre!("unknown search target: {target}"));
    }
    config.search.default_target = Some(target.to_owned());
    config.save()?;
    println!("default search target set to {target}");
    Ok(())
}

fn list() {
    let config = Config::load();
    let default = config
        .search
        .default_target
        .as_deref()
        .unwrap_or("browser/duckduckgo");
    println!("default: {default}");
    println!("targets:");
    println!("- browser/duckduckgo built_in ephemeral_no_cookie");
    println!("- duckduckgo/html built_in no_cookie_http");
    for account in config.search.accounts {
        println!("- {} configured", account.target());
    }
}

fn remove(target: &str) -> color_eyre::Result<()> {
    if has_builtin_target(target) {
        return Err(eyre!("cannot remove built-in search target: {target}"));
    }
    let mut config = Config::load();
    let before = config.search.accounts.len();
    config
        .search
        .accounts
        .retain(|account| account.target() != target);
    if before == config.search.accounts.len() {
        return Err(eyre!("unknown search target: {target}"));
    }
    if config.search.default_target.as_deref() == Some(target) {
        config.search.default_target = Some("browser/duckduckgo".to_owned());
    }
    config.save()?;
    println!("removed search target {target}");
    Ok(())
}

fn has_builtin_target(target: &str) -> bool {
    matches!(target, "browser/duckduckgo" | "duckduckgo/html")
}
