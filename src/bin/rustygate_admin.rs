use std::str::FromStr;

use clap::{Parser, Subcommand};
use rustygate::{
    auth::keys::{KeyLimits, KeyRole, SqliteKeyStore},
    config::AppConfig,
};

#[derive(Debug, Parser)]
#[command(name = "rustygate_admin")]
#[command(about = "Manage RustyGate API keys")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Keys {
        #[command(subcommand)]
        command: KeysCommand,
    },
}

#[derive(Debug, Subcommand)]
enum KeysCommand {
    Create {
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "inference")]
        role: String,
        #[arg(long)]
        rpm: Option<u32>,
        #[arg(long)]
        daily_tokens: Option<u64>,
        #[arg(long)]
        daily_cost_usd: Option<f64>,
        #[arg(long, default_value_t = true)]
        cache_enabled: bool,
    },
    List,
    Revoke {
        id: String,
    },
    Rotate {
        id: String,
        #[arg(long)]
        label: String,
        #[arg(long)]
        role: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let config = AppConfig::from_env()?;
    let store = SqliteKeyStore::connect(&config.storage.database_url).await?;

    match cli.command {
        Command::Keys { command } => match command {
            KeysCommand::Create {
                label,
                role,
                rpm,
                daily_tokens,
                daily_cost_usd,
                cache_enabled,
            } => {
                let generated = store
                    .create_key(
                        &label,
                        KeyRole::from_str(&role)?,
                        KeyLimits {
                            requests_per_minute: rpm,
                            daily_token_quota: daily_tokens,
                            daily_cost_quota_usd: daily_cost_usd,
                        },
                        cache_enabled,
                    )
                    .await?;
                println!("id: {}", generated.id);
                println!("prefix: {}", generated.prefix);
                println!("key: {}", generated.raw_key);
            }
            KeysCommand::List => {
                for key in store.list_keys().await? {
                    println!(
                        "{}\t{}\t{}\t{}",
                        key.id,
                        key.prefix,
                        key.label,
                        key.role.as_str()
                    );
                }
            }
            KeysCommand::Revoke { id } => {
                store.revoke_key(&id).await?;
                println!("revoked: {id}");
            }
            KeysCommand::Rotate { id, label, role } => {
                let existing = store
                    .get_key(&id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("key `{id}` was not found or is revoked"))?;
                store.revoke_key(&id).await?;
                let role = role
                    .as_deref()
                    .map(KeyRole::from_str)
                    .transpose()?
                    .unwrap_or(existing.role);
                let generated = store
                    .create_key(&label, role, existing.limits, existing.cache_enabled)
                    .await?;
                println!("revoked: {id}");
                println!("id: {}", generated.id);
                println!("prefix: {}", generated.prefix);
                println!("key: {}", generated.raw_key);
            }
        },
    }

    Ok(())
}
