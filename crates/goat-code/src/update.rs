use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use color_eyre::eyre::{Context, eyre};
use flate2::read::GzDecoder;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::style::{ColorMode, Palette, print_row};

const REPOSITORY: &str = "goat-agent/goat-code";
const INSTALL_URL: &str = "https://raw.githubusercontent.com/goat-agent/goat-code/main/install.sh";

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

struct InstallPaths {
    install_dir: PathBuf,
    bin_path: PathBuf,
    helper_path: PathBuf,
}

pub async fn run(force: bool) -> color_eyre::Result<()> {
    let target = target_triple()?;
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let client = reqwest::Client::builder()
        .user_agent(format!("goat-code/{current}"))
        .build()?;
    let release = fetch_latest_release(&client).await?;
    let latest = parse_tag(&release.tag_name)?;

    if latest <= current && !force {
        let color = ColorMode::detect();
        println!("{}", color.paint("goat is up to date", Palette::Success));
        print_row(color, "current", current.to_string(), Palette::Value);
        print_row(color, "latest", latest.to_string(), Palette::Value);
        return Ok(());
    }

    drain_daemon(force).await?;

    let archive_name = format!("goat-code-{target}.tar.gz");
    let archive_url = asset_url(&release, &archive_name)?;
    let checksums_url = asset_url(&release, "SHA256SUMS")?;
    let color = ColorMode::detect();
    println!("{}", color.paint("updating goat", Palette::Provider));
    print_row(color, "current", current.to_string(), Palette::Value);
    print_row(color, "latest", latest.to_string(), Palette::Value);
    print_row(color, "target", target, Palette::Value);
    let archive = download(&client, archive_url).await?;
    let checksums = String::from_utf8(download(&client, checksums_url).await?)?;
    verify_checksum(&archive_name, &archive, &checksums)?;
    print_row(color, "checksum", "verified", Palette::Success);

    let staged_dir = stage_dir(&latest, target)?;
    reset_dir(&staged_dir)?;
    extract_archive(&archive, &staged_dir)?;
    require_file(&staged_dir.join(exe_name("goat-code")))?;
    require_file(&staged_dir.join(exe_name("goat-update")))?;

    let paths = install_paths()?;
    print_row(
        color,
        "install",
        paths.bin_path.display().to_string(),
        Palette::Value,
    );
    run_helper(&staged_dir, &paths, &latest)?;
    Ok(())
}

async fn drain_daemon(force: bool) -> color_eyre::Result<()> {
    let Some(socket) = goat_config::socket_path() else {
        return Ok(());
    };
    if !socket.exists() {
        return Ok(());
    }
    let Ok(sessions) = goat_client::status(&socket).await else {
        return Ok(());
    };
    let active = sessions
        .iter()
        .filter(|s| {
            matches!(
                s.state,
                goat_wire::SessionLiveState::Active {}
                    | goat_wire::SessionLiveState::WaitingOnAsk {}
            )
        })
        .count();
    if active > 0 && !force {
        return Err(eyre!(
            "{active} session(s) are still running in the daemon. Finish them or run `goat-code daemon stop`, then retry (or use `goat-code update --force`)."
        ));
    }
    println!("Stopping the running daemon before update...");
    let _ = goat_client::stop(&socket).await;
    Ok(())
}

async fn fetch_latest_release(client: &reqwest::Client) -> color_eyre::Result<Release> {
    let response = client
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/releases/latest"
        ))
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

async fn download(client: &reqwest::Client, url: &str) -> color_eyre::Result<Vec<u8>> {
    let response = client.get(url).send().await?.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}

fn asset_url<'a>(release: &'a Release, name: &str) -> color_eyre::Result<&'a str> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .map(|asset| asset.browser_download_url.as_str())
        .ok_or_else(|| eyre!("release asset not found: {name}"))
}

fn parse_tag(tag: &str) -> color_eyre::Result<Version> {
    Ok(Version::parse(tag.strip_prefix('v').unwrap_or(tag))?)
}

fn target_triple() -> color_eyre::Result<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Ok("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Ok("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok("x86_64-pc-windows-msvc")
    } else {
        Err(eyre!("unsupported update target"))
    }
}

fn verify_checksum(name: &str, bytes: &[u8], checksums: &str) -> color_eyre::Result<()> {
    let expected = parse_checksums(checksums)
        .remove(name)
        .ok_or_else(|| eyre!("checksum not found for {name}"))?;
    let actual = hex_digest(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(eyre!("checksum mismatch for {name}"))
    }
}

fn parse_checksums(raw: &str) -> HashMap<String, String> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?.trim_start_matches('*');
            Some((name.to_string(), hash.to_ascii_lowercase()))
        })
        .collect()
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn stage_dir(version: &Version, target: &str) -> color_eyre::Result<PathBuf> {
    let base = goat_config::update_dir().ok_or_else(|| eyre!(goat_config::HOME_NOT_FOUND))?;
    Ok(base.join(format!("{version}-{target}")))
}

fn reset_dir(path: &Path) -> color_eyre::Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    fs::create_dir_all(path)?;
    Ok(())
}

fn extract_archive(bytes: &[u8], dest: &Path) -> color_eyre::Result<()> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let file_name = path
            .file_name()
            .ok_or_else(|| eyre!("archive entry has no file name"))?;
        let out = dest.join(file_name);
        entry.unpack(out)?;
    }
    Ok(())
}

fn install_paths() -> color_eyre::Result<InstallPaths> {
    let current = std::env::current_exe()?;
    let install_dir = current
        .parent()
        .ok_or_else(|| eyre!("could not resolve current executable directory"))?
        .to_path_buf();
    if let Some(expected) = goat_config::bin_dir()
        && install_dir != expected
    {
        println!("Reinstall goat-code with:");
        println!("  curl -fsSL {INSTALL_URL} | sh");
        return Err(eyre!(
            "goat-code is not installed in {}",
            expected.display()
        ));
    }
    Ok(InstallPaths {
        bin_path: install_dir.join(exe_name("goat-code")),
        helper_path: install_dir.join(exe_name("goat-update")),
        install_dir,
    })
}

fn run_helper(
    staged_dir: &Path,
    paths: &InstallPaths,
    version: &Version,
) -> color_eyre::Result<()> {
    let helper = staged_dir.join(exe_name("goat-update"));
    let args = [
        "apply".to_string(),
        "--staged-dir".to_string(),
        staged_dir.display().to_string(),
        "--install-dir".to_string(),
        paths.install_dir.display().to_string(),
        "--bin-path".to_string(),
        paths.bin_path.display().to_string(),
        "--helper-path".to_string(),
        paths.helper_path.display().to_string(),
        "--version".to_string(),
        version.to_string(),
    ];

    let mut command = Command::new(helper);
    command.args(args);

    if cfg!(windows) {
        command
            .spawn()
            .context("failed to start goat-update helper")?;
        println!("Update helper started. goat will be replaced after this process exits.");
        Ok(())
    } else {
        let status = command
            .status()
            .context("failed to run goat-update helper")?;
        if status.success() {
            Ok(())
        } else {
            Err(eyre!("goat-update failed"))
        }
    }
}

fn require_file(path: &Path) -> color_eyre::Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(eyre!("required file is missing: {}", path.display()))
    }
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{hex_digest, parse_checksums, parse_tag};

    #[test]
    fn parses_v_tag() {
        assert_eq!(parse_tag("v1.2.3").unwrap().to_string(), "1.2.3");
    }

    #[test]
    fn parses_checksums() {
        let parsed = parse_checksums("abc  goat-code.tar.gz\ndef *other.tar.gz\n");
        assert_eq!(parsed["goat-code.tar.gz"], "abc");
        assert_eq!(parsed["other.tar.gz"], "def");
    }

    #[test]
    fn hashes_bytes() {
        assert_eq!(
            hex_digest(b"goat"),
            "5480f08f35968440ebe8135a8bf9e58c8c944bf4e3ba0f45acb141e474bd0c9c"
        );
    }
}
