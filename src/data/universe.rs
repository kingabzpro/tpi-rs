//! Ticker and universe resolution: explicit lists, Top-N discovery and
//! deterministic fallback when discovery is unavailable.

use crate::config::DEFAULT_UNIVERSE;
use crate::data::alpaca::AlpacaClient;
use crate::error::{Error, Result};

const DISCOVERY_ALIASES: &[&str] = &["TOP", "TOP100", "AUTO", "SCAN"];

pub fn parse_tickers(values: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        for part in value.split([',', ' ', ';']) {
            let symbol = part.trim().to_ascii_uppercase();
            if symbol.is_empty() {
                continue;
            }
            if validate_symbol(&symbol) && !out.contains(&symbol) {
                out.push(symbol);
            }
        }
    }
    out
}

pub fn validate_symbol(symbol: &str) -> bool {
    let len = symbol.len();
    (1..=10).contains(&len)
        && symbol
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '.' || c == '-')
}

pub fn wants_discovery(values: &[String]) -> bool {
    values.iter().any(|v| {
        let v = v.trim().to_ascii_uppercase();
        DISCOVERY_ALIASES.contains(&v.as_str())
    })
}

/// Resolve the requested symbols. Top-N discovery failures degrade to the
/// configured historical universe with a warning rather than aborting.
pub async fn resolve_symbols(
    client: Option<&AlpacaClient>,
    requested: &[String],
    top: Option<usize>,
    offline: bool,
    fallback: &[String],
) -> Result<(Vec<String>, Vec<String>)> {
    let mut warnings = Vec::new();
    let explicit = parse_tickers(requested);
    let discovery_requested = top.is_some() || wants_discovery(requested) || explicit.is_empty();

    if !explicit.is_empty() && !wants_discovery(requested) && top.is_none() {
        return Ok((explicit, warnings));
    }

    let limit = top.unwrap_or(100).clamp(1, 100);
    if offline {
        let mut symbols: Vec<String> = fallback.iter().take(limit).cloned().collect();
        if symbols.is_empty() {
            symbols = DEFAULT_UNIVERSE.iter().map(|s| s.to_string()).collect();
        }
        warnings.push(format!(
            "Offline/synthetic mode: Top-{limit} replaced by {} configured symbols",
            symbols.len()
        ));
        return Ok((symbols, warnings));
    }

    if !discovery_requested {
        return Ok((explicit, warnings));
    }

    if let Some(client) = client {
        match client.most_active(limit).await {
            Ok(symbols) if !symbols.is_empty() => {
                let mut clean = Vec::new();
                for s in symbols {
                    if validate_symbol(&s) && !clean.contains(&s) {
                        clean.push(s);
                    }
                }
                clean.truncate(limit);
                if !clean.is_empty() {
                    return Ok((clean, warnings));
                }
                warnings.push("Top-N discovery returned no valid symbols; using fallback".into());
            }
            Ok(_) => warnings.push("Top-N discovery returned no symbols; using fallback".into()),
            Err(e) => warnings.push(format!(
                "Top-N discovery unavailable ({e}); using fallback universe"
            )),
        }
    } else {
        warnings.push("No Alpaca client for discovery; using fallback universe".into());
    }

    let symbols: Vec<String> = fallback.iter().take(limit).cloned().collect();
    if symbols.is_empty() {
        return Err(Error::NoData(
            "no symbols could be resolved for discovery".to_string(),
        ));
    }
    Ok((symbols, warnings))
}

/// Resolve a backtest universe from CLI text or configuration.
pub fn resolve_universe(cli: &[String], configured: &[String]) -> Result<Vec<String>> {
    let parsed = parse_tickers(cli);
    let list = if parsed.is_empty() {
        configured.to_vec()
    } else {
        parsed
    };
    let mut clean = Vec::new();
    for symbol in list {
        let s = symbol.trim().to_ascii_uppercase();
        if validate_symbol(&s) && !clean.contains(&s) {
            clean.push(s);
        }
    }
    if clean.is_empty() {
        return Err(Error::validation("universe is empty or invalid"));
    }
    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_dedups() {
        let out = parse_tickers(&["aapl, MSFT".into(), "AAPL".into(), "BAD!".into()]);
        assert_eq!(out, vec!["AAPL", "MSFT"]);
    }

    #[test]
    fn validates_symbols() {
        assert!(validate_symbol("BRK.B"));
        assert!(validate_symbol("BF-B"));
        assert!(!validate_symbol(""));
        assert!(!validate_symbol("TOOLONGSYMBOL"));
        assert!(!validate_symbol("AAPL;"));
    }

    #[test]
    fn offline_top_uses_fallback() {
        let fallback = vec!["A".to_string(), "B".to_string()];
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (symbols, warnings) = runtime
            .block_on(resolve_symbols(None, &[], Some(100), true, &fallback))
            .unwrap();
        assert_eq!(symbols, fallback);
        assert_eq!(warnings.len(), 1);
    }
}
