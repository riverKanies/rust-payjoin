use std::path::PathBuf;

use anyhow::Result;
use clap::ArgMatches;
use config::builder::DefaultState;
use config::{ConfigError, File, FileFormat};
use payjoin::bitcoin::FeeRate;
use serde::Deserialize;
use url::Url;

use crate::db;

type Builder = config::builder::ConfigBuilder<DefaultState>;

#[derive(Debug, Clone, Deserialize)]
pub struct BitcoindConfig {
    pub rpchost: Url,
    pub cookie: Option<PathBuf>,
    pub rpcuser: String,
    pub rpcpassword: String,
}

#[cfg(feature = "v1")]
#[derive(Debug, Clone, Deserialize)]
pub struct V1Config {
    pub port: u16,
    pub pj_endpoint: Url,
}

#[cfg(feature = "v2")]
#[derive(Debug, Clone, Deserialize)]
pub struct V2Config {
    #[serde(deserialize_with = "deserialize_ohttp_keys_from_path")]
    pub ohttp_keys: Option<payjoin::OhttpKeys>,
    pub ohttp_relay: Url,
    pub pj_directory: Url,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub db_path: PathBuf,
    pub max_fee_rate: Option<FeeRate>,
    pub bitcoind: BitcoindConfig,
    #[cfg(feature = "v1")]
    pub v1: V1Config,
    #[cfg(feature = "v2")]
    pub v2: V2Config,
}

impl Config {
    pub(crate) fn new(matches: &ArgMatches) -> Result<Self, ConfigError> {
        let mut builder = config::Config::builder();
        builder = add_bitcoind_defaults(builder, matches)?;
        builder = add_common_defaults(builder, matches)?;

        #[cfg(feature = "v1")]
        {
            builder = add_v1_defaults(builder)?;
        }

        #[cfg(feature = "v2")]
        {
            builder = add_v2_defaults(builder, matches)?;
        }

        builder = handle_subcommands(builder, matches)?;
        builder = builder.add_source(File::new("config.toml", FileFormat::Toml).required(false));

        let config = builder.build()?;
        let app_config: Config = config.try_deserialize()?;
        log::debug!("App config: {:?}", app_config);
        Ok(app_config)
    }
}

/// Set up default values and CLI overrides for Bitcoin RPC connection settings
fn add_bitcoind_defaults(builder: Builder, matches: &ArgMatches) -> Result<Builder, ConfigError> {
    builder
        .set_default("bitcoind.rpchost", "http://localhost:18443")?
        .set_override_option(
            "bitcoind.rpchost",
            matches.get_one::<Url>("rpchost").map(|s| s.as_str()),
        )?
        .set_default("bitcoind.cookie", None::<String>)?
        .set_override_option(
            "bitcoind.cookie",
            matches.get_one::<String>("cookie_file").map(|s| s.as_str()),
        )?
        .set_default("bitcoind.rpcuser", "bitcoin")?
        .set_override_option(
            "bitcoind.rpcuser",
            matches.get_one::<String>("rpcuser").map(|s| s.as_str()),
        )?
        .set_default("bitcoind.rpcpassword", "")?
        .set_override_option(
            "bitcoind.rpcpassword",
            matches.get_one::<String>("rpcpassword").map(|s| s.as_str()),
        )
}

/// Set up default values and CLI overrides for common settings shared between v1 and v2
fn add_common_defaults(builder: Builder, matches: &ArgMatches) -> Result<Builder, ConfigError> {
    builder
        .set_default("db_path", db::DB_PATH)?
        .set_override_option("db_path", matches.get_one::<String>("db_path").map(|s| s.as_str()))
}

/// Set up default values for v1-specific settings when v2 is not enabled
#[cfg(feature = "v1")]
fn add_v1_defaults(builder: Builder) -> Result<Builder, ConfigError> {
    builder
        .set_default("v1.port", 3000_u16)?
        .set_default("v1.pj_endpoint", "https://localhost:3000")
}

/// Set up default values and CLI overrides for v2-specific settings
#[cfg(feature = "v2")]
fn add_v2_defaults(builder: Builder, matches: &ArgMatches) -> Result<Builder, ConfigError> {
    builder
        .set_override_option(
            "v2.ohttp_relay",
            matches.get_one::<Url>("ohttp_relay").map(|s| s.as_str()),
        )?
        .set_default("v2.pj_directory", "https://payjo.in")?
        .set_default("v2.ohttp_keys", None::<String>)
}

/// Handles configuration overrides based on CLI subcommands
fn handle_subcommands(builder: Builder, matches: &ArgMatches) -> Result<Builder, ConfigError> {
    match matches.subcommand() {
        Some(("send", _)) => Ok(builder),
        Some(("receive", matches)) => {
            let builder = handle_receive_command(builder, matches)?;
            let max_fee_rate = matches.get_one::<FeeRate>("max_fee_rate");
            builder.set_override_option("max_fee_rate", max_fee_rate.map(|f| f.to_string()))
        }
        #[cfg(feature = "v2")]
        Some(("resume", _)) => Ok(builder),
        _ => unreachable!(), // If all subcommands are defined above, anything else is unreachabe!()
    }
}

/// Handle configuration overrides specific to the receive command
fn handle_receive_command(builder: Builder, matches: &ArgMatches) -> Result<Builder, ConfigError> {
    #[cfg(not(feature = "v2"))]
    let builder = {
        let port = matches
            .get_one::<String>("port")
            .map(|port| port.parse::<u16>())
            .transpose()
            .map_err(|_| ConfigError::Message("\"port\" must be a valid number".to_string()))?;
        builder.set_override_option("v1.port", port)?.set_override_option(
            "v1.pj_endpoint",
            matches.get_one::<Url>("pj_endpoint").map(|s| s.as_str()),
        )?
    };

    #[cfg(feature = "v2")]
    let builder = {
        builder
            .set_override_option(
                "v2.pj_directory",
                matches.get_one::<Url>("pj_directory").map(|s| s.as_str()),
            )?
            .set_override_option(
                "v2.ohttp_keys",
                matches.get_one::<String>("ohttp_keys").map(|s| s.as_str()),
            )?
    };

    Ok(builder)
}

#[cfg(feature = "v2")]
fn deserialize_ohttp_keys_from_path<'de, D>(
    deserializer: D,
) -> Result<Option<payjoin::OhttpKeys>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let path_str: Option<String> = Option::deserialize(deserializer)?;

    match path_str {
        None => Ok(None),
        Some(path) => std::fs::read(path)
            .map_err(|e| serde::de::Error::custom(format!("Failed to read ohttp_keys file: {}", e)))
            .and_then(|bytes| {
                payjoin::OhttpKeys::decode(&bytes).map_err(|e| {
                    serde::de::Error::custom(format!("Failed to decode ohttp keys: {}", e))
                })
            })
            .map(Some),
    }
}
