#![allow(dead_code, unused_imports)]

use clap::{Parser};
use env_logger::Builder;
use eyre::{Result, eyre};
use log::{debug, info, error};
use std::path::PathBuf;
use std::io::Write;
use std::fs;
use std::fs::OpenOptions;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

mod message;
mod message_filter;
mod address_filter;
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
    debug!("Loading configuration from {:?}", cli.config);

    let content = fs::read_to_string(&cli.config)
        .map_err(|e| {
            error!("Failed to read config file {}: {}", cli.config.display(), e);
            eyre!("Failed to read config file {}: {}", cli.config.display(), e)
        })?;

    let config: Config = serde_yaml::from_str(&content)
        .map_err(|e| {
            error!("Failed to parse YAML: {}", e);
            eyre!("Failed to parse YAML: {}", e)
        })?;

    debug!("Successfully loaded configuration.");
    debug!("Parsed config: {:?}", config);

    Ok(config)
}

fn setup_logging() {
    let log_file = "imap-filter.log";
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .expect("Failed to open log file");

    let log_writer = Box::new(file);

    Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            writeln!(
                buf,
                "{} [{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.args()
            )
        })
        .target(env_logger::Target::Pipe(log_writer))
        .init();
}

fn main() -> Result<()> {
    setup_logging();
    info!("=====================================================================================================================");
    info!("Starting IMAP Filter");

    let cli = Cli::parse();
    debug!("Parsed CLI arguments: {:?}", cli);

    let config = load_config(&cli)?;

    let imap_domain = cli.imap_domain.or(config.imap_domain)
        .ok_or_else(|| {
            error!("IMAP domain is required but missing.");
            eyre!("IMAP domain is required")
        })?;

    let imap_username = cli.imap_username.or(config.imap_username)
        .ok_or_else(|| {
            error!("IMAP username is required but missing.");
            eyre!("IMAP username is required")
        })?;

    let imap_password = cli.imap_password.or(config.imap_password)
        .ok_or_else(|| {
            error!("IMAP password is required but missing.");
            eyre!("IMAP password is required")
        })?;

    debug!("IMAP connection parameters retrieved successfully.");

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

    debug!("Loaded {} filters.", filters.len());
    debug!("Filters: {:?}", filters);

    let mut imap_filter = IMAPFilter::new(imap_domain, imap_username, imap_password, filters)?;
    imap_filter.execute()?;

    info!("IMAP Filter execution completed successfully.");
    Ok(())
}
