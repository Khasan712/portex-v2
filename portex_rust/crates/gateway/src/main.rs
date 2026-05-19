use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tracing_subscriber::EnvFilter;

mod acme;
mod auth;
mod config;
mod ingress;
mod metrics;
mod proxy;
mod registry;
mod tls;
mod tunnel;

use acme::AcmeConfig;
use config::Args;
use metrics::Metrics;
use registry::Registry;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_target(false)
        .init();

    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut args = Args::parse();
    let registry = Arc::new(Registry::new());
    let metrics = Arc::new(Metrics::new());
    let auth = Arc::new(auth::Authenticator::from_args(&args).await?);

    // ACME bootstrap (if configured): obtain cert before we build the TLS state.
    let acme_paths = if let Some(cfg) = AcmeConfig::from_args(&args) {
        let paths = acme::bootstrap(&cfg).await.context("acme bootstrap")?;
        args.tls_cert = Some(paths.cert.clone());
        args.tls_key = Some(paths.key.clone());
        Some((cfg, paths))
    } else {
        None
    };

    let tls = tls::build_reloadable(args.tls_cert.as_deref(), args.tls_key.as_deref())?;

    let acme_renewal: Option<tokio::task::JoinHandle<()>> = acme_paths.map(|(cfg, paths)| {
        let reloadable = tls.clone();
        tokio::spawn(acme::renewal_loop(cfg, paths, reloadable))
    });

    // SIGHUP → reload cert + key from --tls-cert / --tls-key paths. Useful
    // when an external process (Caddy, cert-manager, manual swap) rotates
    // the on-disk files.
    if let (Some(cert), Some(key)) = (args.tls_cert.clone(), args.tls_key.clone()) {
        let reloadable = tls.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let mut sighup = match signal(SignalKind::hangup()) {
                    Ok(s) => s,
                    Err(err) => {
                        tracing::warn!(?err, "SIGHUP handler unavailable");
                        return;
                    }
                };
                while sighup.recv().await.is_some() {
                    match tls::reload_from_disk(&reloadable, &cert, &key) {
                        Ok(()) => tracing::info!("reload: cert reloaded via SIGHUP"),
                        Err(err) => tracing::warn!(?err, "reload: SIGHUP reload failed"),
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = reloadable;
                let _ = (cert, key);
            }
        });
    }

    let tunnel_handle = tokio::spawn(tunnel::serve(
        args.tunnel_addr,
        registry.clone(),
        auth.clone(),
        metrics.clone(),
        tls.clone(),
    ));

    let ingress_handle = tokio::spawn(ingress::serve(
        args.http_addr,
        registry.clone(),
        metrics.clone(),
        args.base_domain.clone(),
    ));

    let metrics_handle = args.metrics_addr.map(|addr| {
        tokio::spawn(metrics::serve(addr, metrics.clone(), registry.clone()))
    });

    let https_handle = args.https_addr.map(|addr| {
        tokio::spawn(ingress::serve_https(
            addr,
            registry.clone(),
            metrics.clone(),
            args.base_domain.clone(),
            tls.clone(),
        ))
    });

    tokio::select! {
        res = tunnel_handle => res.context("tunnel task panicked")??,
        res = ingress_handle => res.context("ingress task panicked")??,
        res = maybe(https_handle) => res?,
        res = maybe(metrics_handle) => res?,
        _ = maybe_unit(acme_renewal) => {},
    }
    Ok(())
}

async fn maybe_unit(h: Option<tokio::task::JoinHandle<()>>) {
    match h {
        Some(handle) => {
            let _ = handle.await;
        }
        None => std::future::pending::<()>().await,
    }
}

async fn maybe<T>(h: Option<tokio::task::JoinHandle<anyhow::Result<T>>>) -> anyhow::Result<()> {
    match h {
        Some(handle) => {
            handle.await.context("task panicked")??;
            Ok(())
        }
        None => std::future::pending().await,
    }
}
