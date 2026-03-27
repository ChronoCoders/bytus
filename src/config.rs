use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub jwt_secret: String,
    pub fee_bps: u32,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url =
            env::var("DATABASE_URL").map_err(|_| ConfigError::Missing("DATABASE_URL"))?;
        let jwt_secret = env::var("JWT_SECRET").map_err(|_| ConfigError::Missing("JWT_SECRET"))?;
        let fee_bps_str = env::var("FEE_BPS").map_err(|_| ConfigError::Missing("FEE_BPS"))?;
        let fee_bps = fee_bps_str
            .parse::<u32>()
            .map_err(|_| ConfigError::ParseU32("FEE_BPS"))?;

        Ok(Config {
            database_url,
            jwt_secret,
            fee_bps,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable: {0}")]
    Missing(&'static str),
    #[error("failed to parse environment variable as u32: {0}")]
    ParseU32(&'static str),
}
