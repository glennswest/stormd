//! Certificates for the TLS tests only (stormd#32): a test CA, a server pair
//! for 127.0.0.1 (two, to test rotation), a client pair the CA issued, and
//! one from an unrelated CA. Generated with rcgen once per test run and never
//! written anywhere but the tests' temp dirs, so no private key lives in the
//! repository (a committed key is flagged by secret scanners, test or not).

use std::sync::OnceLock;

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};

pub struct Fixtures {
    pub ca_cert: String,
    pub server_cert: String,
    pub server_key: String,
    pub server2_cert: String,
    pub server2_key: String,
    pub client_cert: String,
    pub client_key: String,
    pub stranger_cert: String,
    pub stranger_key: String,
}

pub fn fixtures() -> &'static Fixtures {
    static F: OnceLock<Fixtures> = OnceLock::new();
    F.get_or_init(|| {
        let (ca, ca_key) = ca("stormd test ca");
        let (stranger_ca, stranger_ca_key) = ca("stormd stranger ca");
        let server = || {
            leaf(
                "server",
                &["127.0.0.1", "localhost"],
                ExtendedKeyUsagePurpose::ServerAuth,
                &ca,
                &ca_key,
            )
        };
        let (server_cert, server_key) = server();
        let (server2_cert, server2_key) = server();
        let (client_cert, client_key) = leaf(
            "client",
            &[],
            ExtendedKeyUsagePurpose::ClientAuth,
            &ca,
            &ca_key,
        );
        let (stranger_cert, stranger_key) = leaf(
            "stranger",
            &[],
            ExtendedKeyUsagePurpose::ClientAuth,
            &stranger_ca,
            &stranger_ca_key,
        );
        Fixtures {
            ca_cert: ca.pem(),
            server_cert,
            server_key,
            server2_cert,
            server2_key,
            client_cert,
            client_key,
            stranger_cert,
            stranger_key,
        }
    })
}

fn ca(name: &str) -> (Certificate, KeyPair) {
    let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
    p.distinguished_name.push(DnType::CommonName, name);
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let key = KeyPair::generate().unwrap();
    let cert = p.self_signed(&key).unwrap();
    (cert, key)
}

/// A leaf the CA issued, as (cert PEM, PKCS#8 key PEM). An IP literal in
/// `names` becomes an IP SAN.
fn leaf(
    cn: &str,
    names: &[&str],
    usage: ExtendedKeyUsagePurpose,
    ca: &Certificate,
    ca_key: &KeyPair,
) -> (String, String) {
    let mut p =
        CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
    p.distinguished_name.push(DnType::CommonName, cn);
    p.is_ca = IsCa::ExplicitNoCa;
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![usage];
    let key = KeyPair::generate().unwrap();
    let cert = p.signed_by(&key, ca, ca_key).unwrap();
    (cert.pem(), key.serialize_pem())
}
