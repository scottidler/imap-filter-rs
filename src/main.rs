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

mod state;
mod message;
mod message_filter;
mod address_filter;
mod imap_filter;
mod uid_tracker;

use state::State;
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
    #[serde(alias = "imap-domain")]
    imap_domain: Option<String>,
    #[serde(alias = "imap-username")]
    imap_username: Option<String>,
    #[serde(alias = "imap-password")]
    imap_password: Option<String>,
    filters: Vec<HashMap<String, MessageFilter>>,
    states: Vec<HashMap<String, State>>,
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

    let states: Vec<State> = config
        .states
        .into_iter()
        .flat_map(|map| map.into_iter().map(|(name, mut state)| {
            state.name = name;
            state
        }))
        .collect();

    debug!("Loaded {} states.", states.len());
    debug!("States: {:?}", states);

    let mut imap_filter = IMAPFilter::new(imap_domain, imap_username, imap_password, filters, states)?;
    imap_filter.execute()?;


    info!("IMAP Filter execution completed successfully.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_config_valid_yaml() {
        let mut tmpfile = NamedTempFile::new().unwrap();
        writeln!(
            tmpfile,
            r#"
imap_domain: imap.test.com
imap_username: test@example.com
imap_password: secret
filters:
  - sample:
      to: "test@example.com"
      action: Star
states:
  - Keepers:
      query: 'X-GM-LABELS "\\Starred"'
      ttl: Keep
"#
        ).unwrap();

        let cli = Cli {
            config: tmpfile.path().to_path_buf(),
            imap_domain: None,
            imap_username: None,
            imap_password: None,
        };

        let config = load_config(&cli).unwrap();
        assert_eq!(config.imap_domain.unwrap(), "imap.test.com");
        assert_eq!(config.filters.len(), 1);
        assert_eq!(config.states.len(), 1);
    }

    #[test]
    fn test_load_config_missing_file_errors() {
        let cli = Cli {
            config: PathBuf::from("nonexistent.yml"),
            imap_domain: None,
            imap_username: None,
            imap_password: None,
        };

        let result = load_config(&cli);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_imap_domain_errors() {
        let cli = Cli {
            config: PathBuf::from("whatever.yml"),
            imap_domain: None,
            imap_username: Some("user".into()),
            imap_password: Some("pass".into()),
        };
        let config = Config {
            imap_domain: None,
            imap_username: Some("user".into()),
            imap_password: Some("pass".into()),
            filters: vec![],
            states: vec![],
        };
        let result = cli.imap_domain.or(config.imap_domain)
            .ok_or_else(|| eyre!("IMAP domain is required"));
        assert!(result.is_err());
    }
}
