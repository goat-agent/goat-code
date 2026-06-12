use axoupdater::{AxoUpdater, AxoupdateError};

pub async fn run() -> color_eyre::Result<()> {
    let mut updater = AxoUpdater::new_for("goat");

    match updater.load_receipt() {
        Ok(_) => {}
        Err(AxoupdateError::NoReceipt { .. } | AxoupdateError::ReceiptLoadFailed { .. }) => {
            println!(
                "No install receipt found. To use self-update, install goat via the official installer:"
            );
            println!(
                "  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/goat-agent/goat-code/releases/latest/download/goat-code-installer.sh | sh"
            );
            return Ok(());
        }
        Err(e) => return Err(color_eyre::eyre::eyre!("{e}")),
    }

    match updater.run().await {
        Ok(Some(result)) => {
            println!("Updated to {}.", result.new_version);
        }
        Ok(None) => {
            println!("Already up to date.");
        }
        Err(e) => return Err(color_eyre::eyre::eyre!("{e}")),
    }

    Ok(())
}
