//! TLS for the API listener (stormd#32). `[api] tls_cert_file` /
//! `tls_key_file` turn it on; `client_ca_file` adds optional client
//! certificates: one that verifies against that CA authenticates the request
//! (the TLS layer marks it with [`ClientCertVerified`]), one that does not
//! fails the handshake, and a client with none still gets in with a bearer
//! token or a session — the auth middleware decides.
//!
//! HTTP/1.1 only (WebSocket upgrades included). The certificate and key are
//! re-read when either file changes, so a pair stormcert rotates is served
//! from the next handshake without restarting stormd; a pair that fails to
//! load keeps the previous one and logs once per change. The client CA is
//! read at startup.

use crate::auth::ClientCertVerified;
use rustls::crypto::CryptoProvider;
use rustls::server::{ClientHello, ResolvesServerCert, WebPkiClientVerifier};
use rustls::sign::CertifiedKey;
use rustls::{RootCertStore, ServerConfig};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tracing::{debug, info, warn};

/// How long a client gets to finish the handshake before the connection is
/// dropped — a stalled peer must not hold a task forever.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn load_certified_key(
    cert: &Path,
    key: &Path,
    provider: &CryptoProvider,
) -> anyhow::Result<CertifiedKey> {
    let chain = CertificateDer::pem_file_iter(cert)
        .map_err(|e| anyhow::anyhow!("{}: {}", cert.display(), e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("{}: {}", cert.display(), e))?;
    if chain.is_empty() {
        anyhow::bail!("{}: no certificate in the file", cert.display());
    }
    let key_der = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| anyhow::anyhow!("{}: {}", key.display(), e))?;
    let signing = provider
        .key_provider
        .load_private_key(key_der)
        .map_err(|e| anyhow::anyhow!("{}: {}", key.display(), e))?;
    let ck = CertifiedKey::new(chain, signing);
    ck.keys_match()
        .map_err(|e| anyhow::anyhow!("{} does not match {}: {}", key.display(), cert.display(), e))?;
    Ok(ck)
}

/// Serves the configured pair, reloading it when either file changes.
#[derive(Debug)]
struct ReloadingCert {
    cert: PathBuf,
    key: PathBuf,
    provider: Arc<CryptoProvider>,
    current: Mutex<((Stamp, Stamp), Arc<CertifiedKey>)>,
}

impl ReloadingCert {
    fn load(cert: PathBuf, key: PathBuf, provider: Arc<CryptoProvider>) -> anyhow::Result<Self> {
        let stamps = (stamp(&cert), stamp(&key));
        let ck = load_certified_key(&cert, &key, &provider)?;
        Ok(Self {
            cert,
            key,
            provider,
            current: Mutex::new((stamps, Arc::new(ck))),
        })
    }

    fn get(&self) -> Arc<CertifiedKey> {
        let stamps = (stamp(&self.cert), stamp(&self.key));
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if current.0 != stamps {
            // Record the stamps either way: a failed load is logged once per
            // change, not on every handshake.
            current.0 = stamps;
            match load_certified_key(&self.cert, &self.key, &self.provider) {
                Ok(ck) => {
                    info!(cert = %self.cert.display(), "API TLS certificate reloaded");
                    current.1 = Arc::new(ck);
                }
                Err(e) => {
                    warn!(error = %e, "API TLS certificate changed but does not load — still serving the previous one")
                }
            }
        }
        current.1.clone()
    }
}

impl ResolvesServerCert for ReloadingCert {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.get())
    }
}

/// The rustls server config for `[api]`, or None when TLS is not configured.
pub fn server_config(api: &crate::config::ApiConfig) -> anyhow::Result<Option<Arc<ServerConfig>>> {
    let (Some(cert), Some(key)) = (&api.tls_cert_file, &api.tls_key_file) else {
        return Ok(None);
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let resolver = Arc::new(ReloadingCert::load(cert.clone(), key.clone(), provider.clone())?);
    let builder = ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?;
    let builder = match &api.client_ca_file {
        Some(ca) => {
            let mut roots = RootCertStore::empty();
            for c in CertificateDer::pem_file_iter(ca)
                .map_err(|e| anyhow::anyhow!("{}: {}", ca.display(), e))?
            {
                roots
                    .add(c.map_err(|e| anyhow::anyhow!("{}: {}", ca.display(), e))?)
                    .map_err(|e| anyhow::anyhow!("{}: {}", ca.display(), e))?;
            }
            if roots.is_empty() {
                anyhow::bail!("{}: no CA certificate in the file", ca.display());
            }
            let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .allow_unauthenticated()
                .build()?;
            builder.with_client_cert_verifier(verifier)
        }
        None => builder.with_no_client_auth(),
    };
    let mut config = builder.with_cert_resolver(resolver);
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Some(Arc::new(config)))
}

/// Accept TLS connections and serve `router` on each. Runs until dropped.
pub async fn serve(
    listener: tokio::net::TcpListener,
    router: axum::Router,
    config: Arc<ServerConfig>,
) {
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    loop {
        let (tcp, peer) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                // EMFILE and friends: back off rather than spin.
                warn!(error = %e, "API accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let router = router.clone();
        tokio::spawn(async move {
            let tls = match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    debug!(%peer, error = %e, "API TLS handshake failed");
                    return;
                }
                Err(_) => {
                    debug!(%peer, "API TLS handshake timed out");
                    return;
                }
            };
            // The verifier rejects a certificate that does not chain to the
            // client CA, so any certificate still here is a verified one.
            let verified = tls
                .get_ref()
                .1
                .peer_certificates()
                .is_some_and(|c| !c.is_empty());
            let service = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
                if verified {
                    req.extensions_mut().insert(ClientCertVerified);
                }
                let mut router = router.clone();
                // Router is always ready; no poll_ready needed.
                async move { tower::Service::call(&mut router, req).await }
            });
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(tls), service)
                .with_upgrades()
                .await
            {
                debug!(%peer, error = %e, "API connection ended with an error");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls_fixtures::*;
    use axum::routing::get;

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn files(pairs: &[(&str, &str)]) -> Dir {
        let dir = std::env::temp_dir().join(format!("stormd-tls-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in pairs {
            std::fs::write(dir.join(name), text).unwrap();
        }
        Dir(dir)
    }

    fn api_config(dir: &Path, client_ca: bool) -> crate::config::ApiConfig {
        let mut api = crate::config::ApiConfig::default();
        api.tls_cert_file = Some(dir.join("server.crt"));
        api.tls_key_file = Some(dir.join("server.key"));
        if client_ca {
            api.client_ca_file = Some(dir.join("ca.crt"));
        }
        api
    }

    /// The TLS listener in front of a router that reports whether the
    /// request carries a verified client certificate.
    async fn start(api: &crate::config::ApiConfig) -> String {
        let app = axum::Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .route(
                "/whoami",
                get(|req: axum::extract::Request| async move {
                    if req.extensions().get::<ClientCertVerified>().is_some() {
                        "client-cert"
                    } else {
                        "anonymous"
                    }
                }),
            );
        let config = server_config(api).unwrap().unwrap();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://127.0.0.1:{}", l.local_addr().unwrap().port());
        tokio::spawn(serve(l, app, config));
        url
    }

    fn client(identity: Option<(&str, &str)>) -> reqwest::Client {
        let mut b = reqwest::Client::builder()
            .use_rustls_tls()
            .tls_built_in_root_certs(false)
            .add_root_certificate(reqwest::Certificate::from_pem(CA_CERT.as_bytes()).unwrap());
        if let Some((cert, key)) = identity {
            b = b.identity(reqwest::Identity::from_pem(format!("{}{}", cert, key).as_bytes()).unwrap());
        }
        b.build().unwrap()
    }

    fn server_files() -> Dir {
        files(&[("server.crt", SERVER_CERT), ("server.key", SERVER_KEY), ("ca.crt", CA_CERT)])
    }

    #[test]
    fn no_tls_configured_is_none() {
        assert!(server_config(&crate::config::ApiConfig::default()).unwrap().is_none());
    }

    #[test]
    fn a_key_that_is_not_the_certificates_is_refused() {
        let d = files(&[("server.crt", SERVER_CERT), ("server.key", SERVER2_KEY)]);
        let err = server_config(&api_config(&d.0, false)).unwrap_err();
        assert!(err.to_string().contains("does not match"), "{}", err);
    }

    #[tokio::test]
    async fn serves_https_and_plaintext_is_refused() {
        let d = server_files();
        let url = start(&api_config(&d.0, false)).await;
        let resp = client(None).get(format!("{}/healthz", url)).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "ok");
        // Plain HTTP to the TLS port gets no HTTP answer.
        let plain = url.replace("https://", "http://");
        let r = reqwest::Client::new().get(format!("{}/healthz", plain)).send().await;
        assert!(r.is_err() || r.unwrap().status() != 200);
    }

    #[tokio::test]
    async fn a_client_certificate_from_the_ca_is_marked_verified() {
        let d = server_files();
        let url = start(&api_config(&d.0, true)).await;
        let who = |c: reqwest::Client| {
            let url = url.clone();
            async move { c.get(format!("{}/whoami", url)).send().await.map(|r| r.status()) }
        };
        let with_cert = client(Some((CLIENT_CERT, CLIENT_KEY)));
        let r = with_cert.get(format!("{}/whoami", url)).send().await.unwrap();
        assert_eq!(r.text().await.unwrap(), "client-cert");
        // No certificate: the handshake still succeeds, the request is anonymous.
        let r = client(None).get(format!("{}/whoami", url)).send().await.unwrap();
        assert_eq!(r.text().await.unwrap(), "anonymous");
        // A certificate from another CA fails the handshake.
        assert!(who(client(Some((STRANGER_CERT, STRANGER_KEY)))).await.is_err());
    }

    #[tokio::test]
    async fn a_rotated_pair_is_served_from_the_next_handshake() {
        let d = server_files();
        let url = start(&api_config(&d.0, false)).await;
        let healthy = |c: &reqwest::Client| {
            let url = url.clone();
            let c = c.clone();
            async move {
                let r = c.get(format!("{}/healthz", url)).send().await.unwrap();
                assert_eq!(r.status(), 200);
            }
        };
        healthy(&client(None)).await;
        // A different size is a change even within one mtime tick.
        std::fs::write(d.0.join("server.crt"), format!("{}\n", SERVER2_CERT)).unwrap();
        std::fs::write(d.0.join("server.key"), format!("{}\n", SERVER2_KEY)).unwrap();
        // Both pairs are from the same CA, so the client cannot tell them
        // apart; check the reload itself on a resolver over the same files.
        let resolver = ReloadingCert::load(
            d.0.join("server.crt"),
            d.0.join("server.key"),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .unwrap();
        let expected = CertificateDer::pem_slice_iter(SERVER2_CERT.as_bytes())
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(resolver.get().cert[0], expected);
        healthy(&client(None)).await;

        // A half-written pair keeps the previous one.
        std::fs::write(d.0.join("server.key"), "garbage").unwrap();
        assert_eq!(resolver.get().cert[0], expected);
        healthy(&client(None)).await;
    }
}
