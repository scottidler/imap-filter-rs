#![allow(dead_code)]


use clap::{Parser};
use eyre::{Result, eyre};
use std::path::PathBuf;
use std::fs;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

mod leatherman;
mod imap_filter;

use imap_filter::{IMAPFilter, MessageFilter};

#[derive(Parser, Debug)]
#[command(name = "imap-filter", version, about = "IMAP email filtering CLI", long_about = None)]
struct Cli {
    #[arg(short, long, default_value = "imap-filter.yml")]
    config: PathBuf,

    #[arg(short = 'd', long, env = "IMAP_DOMAIN")]
    imap_domain: Option<String>,

    #[arg(short = 'u', long, env = "IMAP_USERNAME")]
    imap_username: Option<String>,

    #[arg(short = 'p', long, env = "IMAP_PASSWORD")]
    imap_password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Config {
    imap_domain: Option<String>,
    imap_username: Option<String>,
    imap_password: Option<String>,
    filters: Vec<HashMap<String, MessageFilter>>,
    folders: Option<HashMap<String, FolderSettings>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FolderSettings {
    read: Option<String>,
    unread: Option<String>,
}

fn load_config(cli: &Cli) -> Result<Config> {
    let content = fs::read_to_string(&cli.config)
        .map_err(|e| eyre!("Failed to read config file {}: {}", cli.config.display(), e))?;
    let config: Config = serde_yaml::from_str(&content)
        .map_err(|e| eyre!("Failed to parse YAML: {}", e))?;

    Ok(config)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = load_config(&cli)?;

    let imap_domain = cli.imap_domain.or(config.imap_domain).ok_or_else(|| eyre!("IMAP domain is required"))?;
    let imap_username = cli.imap_username.or(config.imap_username).ok_or_else(|| eyre!("IMAP username is required"))?;
    let imap_password = cli.imap_password.or(config.imap_password).ok_or_else(|| eyre!("IMAP password is required"))?;

    let filters: Vec<MessageFilter> = config
        .filters
        .into_iter()
        .flat_map(|map| {
            map.into_iter().map(|(name, mut filter)| {
                filter.name = name;
                filter
            })
        })
        .collect();

    let mut imap_filter = IMAPFilter::new(imap_domain, imap_username, imap_password, filters)?;
    imap_filter.execute()?;

    Ok(())
}
