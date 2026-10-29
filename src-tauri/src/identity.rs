use anyhow::{Context, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use std::path::Path;

/// This device's long-lived identity: a self-signed certificate whose SHA-256 is the device id.
pub struct Identity {
    pub cert: CertificateDer<'static>,
    pub key: PrivatePkcs8KeyDer<'static>,
    pub fp: String,
}

impl Identity {
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let (cp, kp) = (dir.join("identity.crt"), dir.join("identity.key"));
        if !cp.exists() || !kp.exists() {
            let ck = rcgen::generate_simple_self_signed(vec!["bekdesk".into()])?;
            std::fs::write(&kp, ck.key_pair.serialize_der())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&kp, std::fs::Permissions::from_mode(0o600))?;
            }
            std::fs::write(&cp, ck.cert.der())?;
        }
        let cert = CertificateDer::from(std::fs::read(&cp).context("read identity.crt")?);
        let key = PrivatePkcs8KeyDer::from(std::fs::read(&kp).context("read identity.key")?);
        Ok(Identity { fp: fingerprint(&cert), cert, key })
    }

    pub fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(self.key.clone_key())
    }
}

pub fn fingerprint(der: &[u8]) -> String {
    hex::encode(Sha256::digest(der))
}

/// Short-authentication string both screens show during pairing; equal codes rule out a man in the middle.
pub fn pairing_code(a: &str, b: &str) -> String {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    let h = Sha256::digest(format!("bekdesk-pair:{lo}:{hi}"));
    let n = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_symmetric_and_distinct() {
        assert_eq!(pairing_code("aa", "bb"), pairing_code("bb", "aa"));
        assert_ne!(pairing_code("aa", "bb"), pairing_code("aa", "cc"));
        assert_eq!(pairing_code("aa", "bb").len(), 7);
    }

    #[test]
    fn identity_persists() {
        let d = tempfile::tempdir().unwrap();
        let a = Identity::load_or_create(d.path()).unwrap();
        let b = Identity::load_or_create(d.path()).unwrap();
        assert_eq!(a.fp, b.fp);
        assert_eq!(a.fp.len(), 64);
    }
}
