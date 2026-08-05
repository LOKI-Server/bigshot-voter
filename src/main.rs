//! Big-Shot Voting — self-hosted voting panels.

// Release builds: no console on double-click / Start Menu (tray-only start).
// Debug builds keep the console subsystem so `cargo run` / --help work normally.
// Parent terminals are still attached in tray::prepare_stdio for release + CLI.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use bigshot_voter::app::{AppState, build_router};
use bigshot_voter::config::{self, Cli, Config};
use bigshot_voter::db;
use bigshot_voter::tray::{self, ShutdownFlag, TrayConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tray::prepare_stdio();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let foreground = cli.foreground;
    let no_tray = cli.no_tray;

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

    // Tray + console management (Windows). Config-only modes above never reach here.
    let shutdown_flag = ShutdownFlag::new();
    let _tray = tray::start(
        TrayConfig {
            bind: bind.clone(),
            foreground,
            no_tray,
        },
        shutdown_flag.clone(),
    );

    // Graceful-ish shutdown: stop accepting on ctrl-c, tray Exit, or console close.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_flag))
        .await?;
    Ok(())
}

async fn shutdown_signal(tray_shutdown: ShutdownFlag) {
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

    let from_tray = tray::wait_for_shutdown(tray_shutdown);

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
        _ = from_tray => {},
    }
    tracing::info!("shutdown signal received");
}
