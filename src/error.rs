use std::path::PathBuf;

/// Errors that carry enough context for actionable CLI messages.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("invalid input: {0}")]
    Validation(String),

    #[error(
        "missing Alpaca credentials: set ALPACA_API_KEY and ALPACA_SECRET_KEY in .env or the environment"
    )]
    MissingCredentials,

    #[error("Alpaca API {status} on {path}: {message}")]
    Api {
        status: u16,
        path: String,
        message: String,
    },

    #[error("Alpaca rate limit reached on {path}; retry after {retry_after_secs}s")]
    RateLimited { path: String, retry_after_secs: u64 },

    #[error("network error calling {path}: {source}")]
    Network {
        path: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("insufficient market data for {symbol}: need {needed} bars, have {have}")]
    InsufficientData {
        symbol: String,
        needed: usize,
        have: usize,
    },

    #[error("no market data returned for: {0}")]
    NoData(String),

    #[error("live trading refused: {0}")]
    LiveTradingRefused(String),

    #[error("risk limit exceeded: {0}")]
    RiskLimit(String),

    #[error("operation cancelled")]
    Cancelled,

    #[error("cache error: {0}")]
    Cache(String),

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
}

impl Error {
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            path: PathBuf::from("<unbound>"),
            source,
        }
    }
}

pub trait IoContext<T> {
    fn at_path(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at_path(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.into(),
            source,
        })
    }
}
