use anyhow::{Context, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use std::path::Path;

/// A long-lived EC P-256 key and self-signed certificate.
pub struct Identity {
    pub cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
}

impl Identity {
    /// Load `cert.der` / `key.der` from `dir`, creating them on first run.
    pub fn load_or_create(dir: &Path, common_name: &str) -> Result<Self> {
        let cert_path = dir.join("cert.der");
        let key_path = dir.join("key.der");
        if cert_path.exists() && key_path.exists() {
            let cert = std::fs::read(&cert_path).context("reading cert.der")?;
            let key = std::fs::read(&key_path).context("reading key.der")?;
            return Ok(Self { cert: CertificateDer::from(cert), key: PrivatePkcs8KeyDer::from(key) });
        }

        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let cert = rcgen::CertificateParams::new(vec![common_name.to_string()])?.self_signed(&key)?;
        let cert_der = cert.der().to_vec();
        let key_der = key.serialize_der();
        write_private(&key_path, &key_der)?;
        std::fs::write(&cert_path, &cert_der)?;
        Ok(Self { cert: CertificateDer::from(cert_der), key: PrivatePkcs8KeyDer::from(key_der) })
    }

    pub fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(self.key.clone_key())
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        fingerprint(self.cert.as_ref())
    }
}

/// SHA-256 of a DER certificate.
pub fn fingerprint(cert_der: &[u8]) -> [u8; 32] {
    Sha256::digest(cert_der).into()
}

/// device_id / desktop_id: first 16 bytes of the fingerprint as hex.
pub fn short_id(fp: &[u8; 32]) -> String {
    hex(&fp[..16])
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    opts.mode(0o600);
    opts.open(path)?.write_all(data)?;
    Ok(())
}
