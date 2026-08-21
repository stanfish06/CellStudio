use std::io::Write;
use std::net::IpAddr;
use std::process::ExitCode;

use cellstudio_server::{Config, bind, shutdown_signal};

const USAGE: &str = "\
cellstudio-server --token <hex> [options]

  --token <hex>                 session bearer token (required; from the Electron supervisor)
  --host <ip>                   bind address (default 127.0.0.1; loopback only)
  --port <n>                    bind port (default 0 = ephemeral)
  --brick-cache-bytes <n>       decoded-brick LRU budget
  --decode-permits <n>          concurrent assembled reads (minimum 2)
  --volume-budget-bytes <n>     per-timepoint 3D budget the proxy level is chosen for
  --no-proxy                    do not schedule the volume proxy build on open
  --help                        print this and exit

Prints {\"port\":N} on stdout once bound; logs to stderr (CELLSTUDIO_LOG=debug for more).";

fn main() -> ExitCode {
    let config = match parse_args(std::env::args().skip(1)) {
        Ok(Some(config)) => config,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("cellstudio-server: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("CELLSTUDIO_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("cellstudio-server: cannot start the tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(async move {
        let bound = match bind(config).await {
            Ok(bound) => bound,
            Err(e) => {
                eprintln!("cellstudio-server: cannot bind: {e}");
                return ExitCode::FAILURE;
            }
        };
        // the supervisor blocks on this line, so it is written and flushed before serving
        println!("{{\"port\":{}}}", bound.port());
        if let Err(e) = std::io::stdout().flush() {
            eprintln!("cellstudio-server: cannot flush the port handshake: {e}");
            return ExitCode::FAILURE;
        }
        tracing::info!(addr = %bound.addr, "listening");
        match bound.serve(shutdown_signal()).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                tracing::error!("server stopped: {e}");
                ExitCode::FAILURE
            }
        }
    })
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Config>, String> {
    let mut args = args.peekable();
    let mut token: Option<String> = None;
    let mut host: Option<IpAddr> = None;
    let mut port: Option<u16> = None;
    let mut brick_cache_bytes: Option<usize> = None;
    let mut decode_permits: Option<usize> = None;
    let mut volume_budget_bytes: Option<u64> = None;
    let mut build_proxy = true;

    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--help" | "-h" => return Ok(None),
            "--no-proxy" => build_proxy = false,
            "--token" => token = Some(value()?),
            "--host" => {
                let raw = value()?;
                host = Some(
                    raw.parse()
                        .map_err(|_| format!("{raw} is not an IP address"))?,
                );
            }
            "--port" => {
                let raw = value()?;
                port = Some(raw.parse().map_err(|_| format!("{raw} is not a port"))?);
            }
            "--brick-cache-bytes" => brick_cache_bytes = Some(number(&value()?)?),
            "--decode-permits" => decode_permits = Some(number(&value()?)?),
            "--volume-budget-bytes" => volume_budget_bytes = Some(number(&value()?)?),
            other => return Err(format!("unknown argument {other}")),
        }
    }

    let token = token.ok_or("--token is required")?;
    if token.trim().is_empty() {
        return Err("--token must not be empty".to_owned());
    }
    let mut config = Config::new(token);
    if let Some(host) = host {
        if !host.is_loopback() {
            return Err(format!("{host} is not a loopback address"));
        }
        config.host = host;
    }
    if let Some(port) = port {
        config.port = port;
    }
    if let Some(bytes) = brick_cache_bytes {
        config.brick_cache_bytes = bytes;
    }
    if let Some(permits) = decode_permits {
        config.decode_permits = permits.max(2);
    }
    if let Some(bytes) = volume_budget_bytes {
        config.volume_budget_bytes = bytes;
    }
    config.build_proxy = build_proxy;
    Ok(Some(config))
}

fn number<T: std::str::FromStr>(raw: &str) -> Result<T, String> {
    raw.parse().map_err(|_| format!("{raw} is not a number"))
}
