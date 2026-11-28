mod der;
mod leaf;
pub mod trust;

pub use leaf::{Leaf, LeafResolver, LEAF_LIFETIME};

use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use lucidos_local_token::loopback_cert::PUBLIC_CERT_FILE;
use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType, GeneralSubtree,
    IsCa, KeyPair, KeyUsagePurpose, NameConstraints,
};
use rustls::pki_types::CertificateDer;

pub const CERT_FILE: &str = "ca.pem";

pub const KEY_FILE: &str = "ca-key.pem";

pub const COMMON_NAME: &str = "Lucidos loopback CA";

const LOCK_FILE: &str = ".ca.lock";

const CA_LIFETIME: Duration = Duration::from_secs(20 * 365 * 24 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaDir {
    dir: PathBuf,
    public_cert: PathBuf,
}

impl CaDir {
    pub fn resolve() -> Option<CaDir> {
        let dir = lucidos_home::private_entry(lucidos_home::private::LOOPBACK_CA_DIR)?;
        let public_cert = lucidos_local_token::loopback_cert::public_cert_path()?;
        Some(CaDir { dir, public_cert })
    }

    pub fn at(path: impl Into<PathBuf>) -> CaDir {
        let dir = path.into();
        let public_cert = dir.with_file_name(PUBLIC_CERT_FILE);
        CaDir { dir, public_cert }
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub fn cert_path(&self) -> PathBuf {
        self.dir.join(CERT_FILE)
    }

    pub fn key_path(&self) -> PathBuf {
        self.dir.join(KEY_FILE)
    }

    pub fn public_cert_path(&self) -> &Path {
        &self.public_cert
    }

    pub fn read_certificate(&self) -> io::Result<String> {
        lucidos_local_token::loopback_cert::read_at(self.public_cert_path())
    }
}

pub struct LoopbackCa {
    issuer: rcgen::Certificate,
    key: KeyPair,
    cert_pem: String,
    cert_der: CertificateDer<'static>,
}

impl LoopbackCa {
    pub fn ensure(dir: &CaDir) -> io::Result<LoopbackCa> {
        lucidos_home::ensure_storage_root(dir.path())?;
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.path().join(LOCK_FILE))?;
        lock.lock()?;
        let ca = if dir.key_path().exists() && dir.cert_path().exists() {
            LoopbackCa::load(dir)?
        } else {
            LoopbackCa::create(dir, SystemTime::now())?
        };
        publish_public_copy(dir, ca.cert_pem())?;
        Ok(ca)
    }

    pub fn load(dir: &CaDir) -> io::Result<LoopbackCa> {
        check_owner_only(dir)?;
        let key_pem = std::fs::read_to_string(dir.key_path())?;
        let cert_pem = std::fs::read_to_string(dir.cert_path())?;
        LoopbackCa::from_pem(&cert_pem, &key_pem).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "the loopback CA in {} is unreadable: {e}. Delete that directory and run \
                     `lucidos tls trust` to make a new one.",
                    dir.path().display()
                ),
            )
        })
    }

    fn create(dir: &CaDir, now: SystemTime) -> io::Result<LoopbackCa> {
        let key = KeyPair::generate().map_err(io::Error::other)?;
        let mut params = ca_params();
        params.not_before = (now - leaf::BACKDATE).into();
        params.not_after = (now + CA_LIFETIME).into();
        let cert = params.self_signed(&key).map_err(io::Error::other)?;
        let cert_pem = cert.pem();
        publish(dir, KEY_FILE, &key.serialize_pem())?;
        publish(dir, CERT_FILE, &cert_pem)?;
        LoopbackCa::from_pem(&cert_pem, &key.serialize_pem()).map_err(io::Error::other)
    }

    fn from_pem(cert_pem: &str, key_pem: &str) -> Result<LoopbackCa, String> {
        let key = KeyPair::from_pem(key_pem).map_err(|e| format!("bad key: {e}"))?;
        let cert_der: CertificateDer<'static> =
            rustls::pki_types::pem::PemObject::from_pem_slice(cert_pem.as_bytes())
                .map_err(|e| format!("bad certificate: {e}"))?;
        let matches = der::subject_public_key_info(&cert_der)
            .is_some_and(|spki| spki == key.public_key_der().as_slice());
        if !matches {
            return Err("the certificate does not belong to the key".to_string());
        }
        let issuer = ca_params().self_signed(&key).map_err(|e| e.to_string())?;
        Ok(LoopbackCa {
            issuer,
            key,
            cert_pem: cert_pem.to_string(),
            cert_der,
        })
    }

    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    pub fn cert_der(&self) -> &CertificateDer<'static> {
        &self.cert_der
    }
}

fn ca_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::OrganizationName, "Lucidos");
    name.push(DnType::CommonName, COMMON_NAME);
    params.distinguished_name = name;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.name_constraints = Some(NameConstraints {
        permitted_subtrees: vec![
            GeneralSubtree::DnsName("localhost".to_string()),
            GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix([127, 0, 0, 0], 8)),
            GeneralSubtree::IpAddress(CidrSubnet::from_v6_prefix(
                std::net::Ipv6Addr::LOCALHOST.octets(),
                128,
            )),
        ],
        excluded_subtrees: Vec::new(),
    });
    params
}

fn publish(dir: &CaDir, name: &str, contents: &str) -> io::Result<()> {
    write_through_rename(&dir.path().join(name), contents)
}

fn publish_public_copy(dir: &CaDir, cert_pem: &str) -> io::Result<()> {
    let target = dir.public_cert_path();
    if std::fs::read_to_string(target).is_ok_and(|on_disk| on_disk == cert_pem) {
        return Ok(());
    }
    write_through_rename(target, cert_pem)
}

fn write_through_rename(target: &Path, contents: &str) -> io::Result<()> {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let staged = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    lucidos_local_token::write_owner_only(&staged, contents)?;
    std::fs::rename(&staged, target)
}

fn check_owner_only(dir: &CaDir) -> io::Result<()> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    check_owner_only_as(dir, unsafe { libc::geteuid() })
}

fn check_owner_only_as(dir: &CaDir, me: u32) -> io::Result<()> {
    let dir_meta = std::fs::metadata(dir.path())?;
    let key_path = dir.key_path();
    let key_meta = std::fs::metadata(&key_path)?;
    let cure = format!(
        "Delete {} and run `lucidos tls trust` to make a new one.",
        dir.path().display()
    );
    let foreign = [
        (dir.path(), dir_meta.uid()),
        (key_path.as_path(), key_meta.uid()),
    ]
    .into_iter()
    .find(|(_, owner)| *owner != me);
    if let Some((path, owner)) = foreign {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} belongs to uid {owner}, not to this user (uid {me}), so the loopback CA \
                 cannot be trusted. {cure}",
                path.display()
            ),
        ));
    }
    let loose = [
        (dir.path(), dir_meta.permissions().mode()),
        (key_path.as_path(), key_meta.permissions().mode()),
    ]
    .into_iter()
    .find(|(_, mode)| mode & 0o077 != 0);
    if let Some((path, mode)) = loose {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is open to other users (mode {:o}), so the loopback CA key may have leaked. \
                 {cure}",
                path.display(),
                mode & 0o777
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
