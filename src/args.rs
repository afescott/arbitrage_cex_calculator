use std::collections::HashMap;

/// Command-line arguments for configuring exchange credentials.
///
/// Note: passing secrets via CLI arguments is not ideal because command-line
/// args can be visible in process listings. Prefer environment variables or a
/// secrets manager when possible.
#[derive(Debug, Default, Clone)]
pub struct Args {
    pub pair: Option<String>,

    // Hyperliquid
    pub hyperliquid_private_key: Option<String>,

    // Kraken
    pub kraken_api_key: Option<String>,
    pub kraken_api_secret: Option<String>,

    // Binance
    pub binance_api_key: Option<String>,
    pub binance_api_secret: Option<String>,
}

impl Args {
    /// Parse args from `std::env::args()`.
    ///
    /// Supported flags (all optional):
    /// - `--pair <SYMBOL>` (example: `BTC/USDT`)
    /// - `--hyperliquid-private-key <KEY>`
    /// - `--kraken-api-key <KEY>`
    /// - `--kraken-api-secret <SECRET>`
    /// - `--binance-api-key <KEY>`
    /// - `--binance-api-secret <SECRET>`
    ///
    /// Unknown flags are ignored to keep this lightweight.
    pub fn from_env() -> Result<Self, String> {
        let raw_args: Vec<String> = std::env::args().collect();
        let mut map: HashMap<String, String> = HashMap::new();

        let mut i = 1; // skip argv[0]
        while i < raw_args.len() {
            let key = raw_args[i].as_str();
            if key.starts_with("--") {
                if i + 1 >= raw_args.len() {
                    return Err(format!("Missing value for flag `{}`", key));
                }
                let val = raw_args[i + 1].clone();
                map.insert(key.to_string(), val);
                i += 2;
            } else {
                i += 1;
            }
        }

        Ok(Self {
            pair: map.get("--pair").cloned(),
            hyperliquid_private_key: map.get("--hyperliquid-private-key").cloned(),
            kraken_api_key: map.get("--kraken-api-key").cloned(),
            kraken_api_secret: map.get("--kraken-api-secret").cloned(),
            binance_api_key: map.get("--binance-api-key").cloned(),
            binance_api_secret: map.get("--binance-api-secret").cloned(),
        })
    }
}

