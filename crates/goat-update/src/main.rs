use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use clap::{Parser, Subcommand};
use color_eyre::eyre::{Context, eyre};

#[derive(Parser)]
#[command(name = "goat-update", version, about = "goat update helper")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Apply(ApplyArgs),
}

#[derive(Parser)]
struct ApplyArgs {
    #[arg(long)]
    staged_dir: PathBuf,
    #[arg(long)]
    install_dir: PathBuf,
    #[arg(long)]
    bin_path: PathBuf,
    #[arg(long)]
    helper_path: PathBuf,
    #[arg(long)]
    version: String,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    match Cli::parse().command {
        Command::Apply(args) => apply(&args),
    }
}

fn apply(args: &ApplyArgs) -> color_eyre::Result<()> {
    println!("Installing goat {}...", args.version);
    fs::create_dir_all(&args.install_dir).with_context(|| {
        format!(
            "failed to create install directory {}",
            args.install_dir.display()
        )
    })?;

    let staged_bin = args.staged_dir.join(exe_name("goat-code"));
    let staged_helper = args.staged_dir.join(exe_name("goat-update"));
    require_file(&staged_bin)?;
    require_file(&staged_helper)?;

    replace_pair(
        &staged_bin,
        &args.bin_path,
        &staged_helper,
        &args.helper_path,
    )?;
    println!("goat {} installed.", args.version);
    Ok(())
}

fn replace_pair(
    staged_bin: &Path,
    bin_path: &Path,
    staged_helper: &Path,
    helper_path: &Path,
) -> color_eyre::Result<()> {
    let bin_backup = backup_path(bin_path);
    let helper_backup = backup_path(helper_path);
    cleanup_optional(&bin_backup)?;
    cleanup_optional(&helper_backup)?;

    let result = (|| {
        backup_existing(bin_path, &bin_backup)?;
        backup_existing(helper_path, &helper_backup)?;
        copy_executable(staged_bin, bin_path)?;
        copy_executable(staged_helper, helper_path)?;
        Ok(())
    })();

    if let Err(err) = result {
        restore_backup(&bin_backup, bin_path)?;
        restore_backup(&helper_backup, helper_path)?;
        return Err(err);
    }

    cleanup_optional(&bin_backup)?;
    cleanup_optional(&helper_backup)?;
    Ok(())
}

fn backup_existing(path: &Path, backup: &Path) -> color_eyre::Result<()> {
    if path.exists() {
        retry(|| fs::rename(path, backup)).with_context(|| {
            format!("failed to move {} to {}", path.display(), backup.display())
        })?;
    }
    Ok(())
}

fn restore_backup(backup: &Path, path: &Path) -> color_eyre::Result<()> {
    if backup.exists() {
        cleanup_optional(path)?;
        retry(|| fs::rename(backup, path)).with_context(|| {
            format!(
                "failed to restore {} from {}",
                path.display(),
                backup.display()
            )
        })?;
    }
    Ok(())
}

fn copy_executable(src: &Path, dest: &Path) -> color_eyre::Result<()> {
    retry(|| fs::copy(src, dest).map(|_| ()))
        .with_context(|| format!("failed to install {}", dest.display()))?;
    set_executable(dest)?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> color_eyre::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> color_eyre::Result<()> {
    Ok(())
}

fn retry(mut action: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let mut delay = Duration::from_millis(50);
    let mut last = None;
    for _ in 0..40 {
        match action() {
            Ok(()) => return Ok(()),
            Err(err) => last = Some(err),
        }
        thread::sleep(delay);
        delay = delay.saturating_mul(2).min(Duration::from_millis(500));
    }
    Err(last.expect("retry loop records at least one error"))
}

fn cleanup_optional(path: &Path) -> color_eyre::Result<()> {
    if path.exists() {
        retry(|| fs::remove_file(path))
            .with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

fn require_file(path: &Path) -> color_eyre::Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(eyre!("required file is missing: {}", path.display()))
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_os_string();
    backup.push(".old");
    PathBuf::from(backup)
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
    use std::fs;

    use super::{backup_path, replace_pair};

    #[test]
    fn replaces_pair() {
        let dir = tempfile::tempdir().unwrap();
        let staged_bin = dir.path().join("staged-goat");
        let staged_helper = dir.path().join("staged-helper");
        let bin = dir.path().join("goat");
        let helper = dir.path().join("goat-update");
        fs::write(&staged_bin, "new goat").unwrap();
        fs::write(&staged_helper, "new helper").unwrap();
        fs::write(&bin, "old goat").unwrap();
        fs::write(&helper, "old helper").unwrap();

        replace_pair(&staged_bin, &bin, &staged_helper, &helper).unwrap();

        assert_eq!(fs::read_to_string(&bin).unwrap(), "new goat");
        assert_eq!(fs::read_to_string(&helper).unwrap(), "new helper");
        assert!(!backup_path(&bin).exists());
        assert!(!backup_path(&helper).exists());
    }
}
