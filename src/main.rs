//! Big-Shot Voting — self-hosted voting panels.

use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use bigshot_voter::app::{AppState, build_router};
use bigshot_voter::config::{self, Cli, Config};
use bigshot_voter::db;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // Config-only modes: update bigshot.json and exit without starting the server.
    if cli.clear_data_dir {
        let result = config::clear_persisted_data_dir()?;
        println!(
            "Cleared data directory override (default: data/ next to the executable)."
        );
        println!("Config written to {}", result.config_file.display());
        return Ok(());
    }
    if let Some(path) = cli.set_data_dir {
        let result = config::set_persisted_data_dir(path)?;
        let stored = result
            .data_dir
            .as_deref()
            .expect("set_persisted_data_dir always stores a path");
        println!("Data directory set to {stored}");
        println!("Config written to {}", result.config_file.display());
        println!("This does not move existing files — relocate data yourself if needed.");
        return Ok(());
    }

    let config = Config::from_cli(cli)?;

    tracing::info!(config = %config.config_file.display(), "config file");
    tracing::info!(data_dir = %config.data_dir.display(), "data directory");
    tracing::info!(db = %config.db_path.display(), "database");

    let pool = db::connect(&config.db_path).await?;
    let bind = config.bind.clone();
    let state = AppState {
        pool,
        config: Arc::new(config),
    };

    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("Big-Shot Voting listening on http://{bind}");

    // Graceful-ish shutdown: stop accepting on ctrl-c; in-flight requests finish.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to install SIGTERM handler");
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
