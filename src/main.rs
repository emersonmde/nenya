//! Nenya distributed rate limiting server

#[cfg(not(feature = "server"))]
compile_error!(
    "The nenya binary requires the 'server' feature.\n\
     Install with: cargo install nenya\n\
     Build with: cargo build --features server"
);

#[cfg(feature = "server")]
use nenya::api;
#[cfg(feature = "server")]
use nenya::config::Config;
#[cfg(feature = "server")]
use nenya::gossip::{gossip_sync_loop, GossipManager};

#[cfg(feature = "server")]
use std::sync::Arc;
#[cfg(feature = "server")]
use tokio::net::TcpListener;
#[cfg(feature = "server")]
use tokio::sync::RwLock;

#[cfg(feature = "server")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    tracing::info!("Starting Nenya v{}", env!("CARGO_PKG_VERSION"));

    let config = Config::from_env()?;
    tracing::info!(
        "Loaded configuration: node_id={}, listen_addr={}, gossip_addr={}",
        config.node_id,
        config.listen_addr,
        config.gossip_addr
    );

    let gossip_enabled =
        !config.seed_nodes.is_empty() || std::env::var("NENYA_ENABLE_GOSSIP").is_ok();

    let mut manager = api::RateLimitManager::new(
        config.default_target_rate,
        config.default_kp,
        config.default_ki,
        config.default_kd,
    );

    if gossip_enabled {
        tracing::info!(
            "Configuring distributed rate limiting (cluster-wide target: {} TPS)",
            config.default_target_rate
        );

        let distributed_pattern = api::ScopePattern {
            pattern: "*".to_string(),
            target_rate: config.default_target_rate,
            min_rate: Some(config.default_min_rate),
            max_rate: Some(config.default_max_rate),
            kp: Some(config.default_kp),
            ki: Some(config.default_ki),
            kd: Some(config.default_kd),
            error_limit_frac: None, // simulator-derived default (0.2)
            distributed: true,
        };

        manager.set_default_pattern(distributed_pattern);
    } else {
        tracing::info!(
            "Configuring standalone rate limiting (per-node target: {} TPS)",
            config.default_target_rate
        );
    }

    let manager = Arc::new(RwLock::new(manager));
    let metrics = api::MetricsCollector::new();

    let gossip_handle = if gossip_enabled {
        tracing::info!("Initializing gossip protocol...");

        let seed_nodes: Vec<String> = config
            .seed_nodes
            .iter()
            .map(|addr| addr.to_string())
            .collect();

        match GossipManager::new(
            config.node_id.clone(),
            config.gossip_addr,
            seed_nodes.clone(),
            "nenya".to_string(),
        )
        .await
        {
            Ok(gossip) => {
                let gossip = Arc::new(gossip);
                tracing::info!(
                    "Gossip protocol started on {}, seeds: {:?}",
                    config.gossip_addr,
                    seed_nodes
                );

                let manager_clone = manager.clone();
                let gossip_clone = gossip.clone();
                let node_id = config.node_id.clone();
                let sync_interval = config.sync_interval;
                let stale_timeout = config.stale_timeout;

                tokio::spawn(async move {
                    tracing::info!("Starting gossip sync loop...");
                    gossip_sync_loop(
                        manager_clone,
                        gossip_clone,
                        node_id,
                        sync_interval,
                        stale_timeout,
                    )
                    .await;
                });

                Some(gossip)
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to initialize gossip: {}. Running in standalone mode.",
                    e
                );
                None
            }
        }
    } else {
        tracing::info!("Running in standalone mode (no gossip)");
        None
    };

    let state = Arc::new(api::AppState {
        manager: manager.clone(),
        metrics,
    });

    let router = api::create_router(state);

    let listener = TcpListener::bind(&config.listen_addr).await?;
    tracing::info!("HTTP server listening on {}", config.listen_addr);

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    if let Some(gossip) = gossip_handle {
        tracing::info!("Shutting down gossip protocol...");
        if let Ok(gossip) = Arc::try_unwrap(gossip) {
            let _ = gossip.shutdown().await;
        }
    }

    tracing::info!("Server shutdown complete");
    Ok(())
}

#[cfg(feature = "server")]
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received Ctrl+C, shutting down...");
        },
        _ = terminate => {
            tracing::info!("Received SIGTERM, shutting down...");
        },
    }
}
