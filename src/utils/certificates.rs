use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(test)]
use std::sync::Mutex;

use anyhow::anyhow;
use chrono::{DateTime, TimeZone, Utc};
use ring::{
    rand::SystemRandom,
    signature::{
        EcdsaKeyPair, Ed25519KeyPair, KeyPair, RsaKeyPair, ECDSA_P256_SHA256_ASN1_SIGNING,
        ECDSA_P384_SHA384_ASN1_SIGNING,
    },
};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use simple_asn1::{ASN1Block, ASN1Class, BigInt, OID};

use crate::errors::TlsError;

/// Metadata extracted from the primary leaf certificate for logging and validation.
#[derive(Debug, Clone)]
pub struct CertificateInfo {
    pub subject: String,
    pub issuer: String,
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
}

/// Loaded certificate materials used for configuring TLS listeners.
#[derive(Debug)]
pub struct CertificateBundle {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub chain_path: Option<PathBuf>,
    pub leaf: CertificateDer<'static>,
    pub intermediates: Vec<CertificateDer<'static>>,
    pub private_key: PrivateKeyDer<'static>,
    pub info: CertificateInfo,
    pub public_key_algorithm: String,
    pub public_key_data: Vec<u8>,
}

/// Load and validate certificate materials from disk.
pub fn load_certificate_bundle(
    cert_path: &Path,
    key_path: &Path,
    chain_path: Option<&Path>,
) -> Result<CertificateBundle, TlsError> {
    let cert_bytes = fs::read(cert_path)
        .map_err(|e| TlsError::CertificateReadError { path: cert_path.to_path_buf(), source: e })?;

    let mut leaf_chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&cert_bytes)
        .map(|result| {
            result.map_err(|err| TlsError::InvalidCertificatePem {
                path: cert_path.to_path_buf(),
                source: anyhow!(err),
            })
        })
        .collect::<Result<_, _>>()?;

    if leaf_chain.is_empty() {
        return Err(TlsError::EmptyCertificateChain { path: cert_path.to_path_buf() });
    }

    let leaf = leaf_chain.remove(0);
    let mut intermediates = leaf_chain;

    if let Some(chain_path) = chain_path {
        let chain_bytes = fs::read(chain_path)
            .map_err(|e| TlsError::ChainReadError { path: chain_path.to_path_buf(), source: e })?;

        let additional: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&chain_bytes)
            .map(|result| {
                result.map_err(|err| TlsError::InvalidChainPem {
                    path: chain_path.to_path_buf(),
                    source: anyhow!(err),
                })
            })
            .collect::<Result<_, _>>()?;

        intermediates.extend(additional);
    }

    let key_bytes = fs::read(key_path)
        .map_err(|e| TlsError::PrivateKeyReadError { path: key_path.to_path_buf(), source: e })?;

    let private_key = PrivateKeyDer::from_pem_slice(&key_bytes).map_err(|err| {
        TlsError::InvalidPrivateKey { path: key_path.to_path_buf(), source: Some(anyhow!(err)) }
    })?;

    let parsed = parse_certificate_metadata(&leaf, cert_path)?;

    validate_certificate_dates(&parsed.info, cert_path)?;
    validate_key_pair(&parsed, &private_key, key_path)?;

    Ok(CertificateBundle {
        cert_path: cert_path.to_path_buf(),
        key_path: key_path.to_path_buf(),
        chain_path: chain_path.map(Path::to_path_buf),
        leaf,
        intermediates,
        private_key,
        info: parsed.info,
        public_key_algorithm: parsed.algorithm_oid,
        public_key_data: parsed.public_key,
    })
}

struct ParsedCertificate {
    info: CertificateInfo,
    algorithm_oid: String,
    public_key: Vec<u8>,
}

fn parse_certificate_metadata(
    cert: &CertificateDer<'static>,
    path: &Path,
) -> Result<ParsedCertificate, TlsError> {
    let blocks = simple_asn1::from_der(cert.as_ref()).map_err(|err| {
        TlsError::CertificateMetadata { path: path.to_path_buf(), source: anyhow!(err) }
    })?;

    let cert_seq = match blocks.first() {
        Some(ASN1Block::Sequence(_, items)) => items,
        _ => {
            return Err(TlsError::CertificateMetadata {
                path: path.to_path_buf(),
                source: anyhow!("certificate missing outer sequence"),
            })
        }
    };

    let tbs_seq = match cert_seq.first() {
        Some(ASN1Block::Sequence(_, items)) => items,
        _ => {
            return Err(TlsError::CertificateMetadata {
                path: path.to_path_buf(),
                source: anyhow!("certificate missing tbsCertificate"),
            })
        }
    };

    let mut fields = tbs_seq.iter();

    // Optional version field [0] EXPLICIT Version
    if let Some(ASN1Block::Explicit(ASN1Class::ContextSpecific, _, tag, _)) = fields.next() {
        if tag != &0u8.into() {
            return Err(TlsError::CertificateMetadata {
                path: path.to_path_buf(),
                source: anyhow!("unexpected context-specific field before serial number"),
            });
        }
    }

    // serial number
    fields.next();
    // signature algorithm
    fields.next();

    let issuer_block = fields.next().ok_or_else(|| TlsError::CertificateMetadata {
        path: path.to_path_buf(),
        source: anyhow!("certificate missing issuer"),
    })?;

    let validity_block = fields.next().ok_or_else(|| TlsError::CertificateMetadata {
        path: path.to_path_buf(),
        source: anyhow!("certificate missing validity"),
    })?;

    let subject_block = fields.next().ok_or_else(|| TlsError::CertificateMetadata {
        path: path.to_path_buf(),
        source: anyhow!("certificate missing subject"),
    })?;

    let spki_block = fields.next().ok_or_else(|| TlsError::CertificateMetadata {
        path: path.to_path_buf(),
        source: anyhow!("certificate missing subjectPublicKeyInfo"),
    })?;

    let issuer = parse_name(issuer_block, path)?;
    let subject = parse_name(subject_block, path)?;
    let (not_before, not_after) = parse_validity(validity_block, path)?;
    let public_info = parse_public_key_info(spki_block, path)?;

    Ok(ParsedCertificate {
        info: CertificateInfo { subject, issuer, not_before, not_after },
        algorithm_oid: public_info.algorithm_oid,
        public_key: public_info.public_key,
    })
}

struct PublicKeyInfo {
    algorithm_oid: String,
    public_key: Vec<u8>,
}

fn parse_public_key_info(block: &ASN1Block, path: &Path) -> Result<PublicKeyInfo, TlsError> {
    let items = match block {
        ASN1Block::Sequence(_, items) => items,
        _ => {
            return Err(TlsError::CertificateMetadata {
                path: path.to_path_buf(),
                source: anyhow!("subjectPublicKeyInfo is not a sequence"),
            })
        }
    };

    if items.len() < 2 {
        return Err(TlsError::CertificateMetadata {
            path: path.to_path_buf(),
            source: anyhow!("subjectPublicKeyInfo missing fields"),
        });
    }

    let algorithm_seq = match &items[0] {
        ASN1Block::Sequence(_, seq) => seq,
        _ => {
            return Err(TlsError::CertificateMetadata {
                path: path.to_path_buf(),
                source: anyhow!("algorithm identifier missing"),
            })
        }
    };

    let algorithm_oid = match algorithm_seq.first() {
        Some(ASN1Block::ObjectIdentifier(_, oid)) => oid_to_string(oid),
        _ => {
            return Err(TlsError::CertificateMetadata {
                path: path.to_path_buf(),
                source: anyhow!("algorithm identifier missing OID"),
            })
        }
    };

    let (bit_len, public_key) = match &items[1] {
        ASN1Block::BitString(_, nbits, bytes) => (*nbits, bytes.clone()),
        _ => {
            return Err(TlsError::CertificateMetadata {
                path: path.to_path_buf(),
                source: anyhow!("subject public key is not a bit string"),
            })
        }
    };

    if bit_len % 8 != 0 || public_key.len() * 8 != bit_len {
        return Err(TlsError::CertificateMetadata {
            path: path.to_path_buf(),
            source: anyhow!("subject public key contains unused bits"),
        });
    }

    Ok(PublicKeyInfo { algorithm_oid, public_key })
}

fn parse_name(block: &ASN1Block, path: &Path) -> Result<String, TlsError> {
    let rdns = match block {
        ASN1Block::Sequence(_, items) => items,
        _ => {
            return Err(TlsError::CertificateMetadata {
                path: path.to_path_buf(),
                source: anyhow!("name is not a sequence"),
            })
        }
    };

    let mut components = Vec::new();
    for rdn in rdns {
        let set_items = match rdn {
            ASN1Block::Set(_, items) => items,
            _ => continue,
        };

        for attr in set_items {
            if let ASN1Block::Sequence(_, attr_items) = attr {
                if attr_items.len() < 2 {
                    continue;
                }
                if let ASN1Block::ObjectIdentifier(_, oid) = &attr_items[0] {
                    if let Some(value) = extract_string_value(&attr_items[1]) {
                        let oid_string = oid_to_string(oid);
                        let short = match oid_string.as_str() {
                            "2.5.4.3" => "CN",
                            "2.5.4.6" => "C",
                            "2.5.4.7" => "L",
                            "2.5.4.8" => "ST",
                            "2.5.4.10" => "O",
                            "2.5.4.11" => "OU",
                            other => other,
                        };
                        components.push(format!("{short}={value}"));
                    }
                }
            }
        }
    }

    Ok(components.join(", "))
}

fn extract_string_value(block: &ASN1Block) -> Option<String> {
    match block {
        ASN1Block::UTF8String(_, value)
        | ASN1Block::PrintableString(_, value)
        | ASN1Block::IA5String(_, value)
        | ASN1Block::TeletexString(_, value)
        | ASN1Block::UniversalString(_, value)
        | ASN1Block::BMPString(_, value) => Some(value.clone()),
        _ => None,
    }
}

fn parse_validity(
    block: &ASN1Block,
    path: &Path,
) -> Result<(DateTime<Utc>, DateTime<Utc>), TlsError> {
    let entries = match block {
        ASN1Block::Sequence(_, items) => items,
        _ => {
            return Err(TlsError::CertificateMetadata {
                path: path.to_path_buf(),
                source: anyhow!("validity is not a sequence"),
            })
        }
    };

    if entries.len() < 2 {
        return Err(TlsError::CertificateMetadata {
            path: path.to_path_buf(),
            source: anyhow!("validity sequence missing entries"),
        });
    }

    let not_before = time_block_to_chrono(&entries[0], path)?;
    let not_after = time_block_to_chrono(&entries[1], path)?;

    Ok((not_before, not_after))
}

fn time_block_to_chrono(block: &ASN1Block, path: &Path) -> Result<DateTime<Utc>, TlsError> {
    let primitive = match block {
        ASN1Block::UTCTime(_, value) | ASN1Block::GeneralizedTime(_, value) => value,
        _ => {
            return Err(TlsError::CertificateMetadata {
                path: path.to_path_buf(),
                source: anyhow!("time value not in expected format"),
            })
        }
    };

    let dt = primitive.assume_utc();
    let timestamp = dt.unix_timestamp();
    let nanos = dt.nanosecond();

    let chrono_dt = Utc.timestamp_opt(timestamp, nanos).single().ok_or_else(|| {
        TlsError::CertificateMetadata {
            path: path.to_path_buf(),
            source: anyhow!("failed to convert certificate time"),
        }
    })?;

    Ok(chrono_dt)
}

fn validate_certificate_dates(info: &CertificateInfo, path: &Path) -> Result<(), TlsError> {
    let now = current_time();
    if info.not_before > now {
        return Err(TlsError::CertificateNotYetValid {
            path: path.to_path_buf(),
            not_before: info.not_before,
        });
    }
    if info.not_after <= now {
        return Err(TlsError::CertificateExpired {
            path: path.to_path_buf(),
            not_after: info.not_after,
        });
    }
    Ok(())
}

fn validate_key_pair(
    certificate: &ParsedCertificate,
    private_key: &PrivateKeyDer<'static>,
    key_path: &Path,
) -> Result<(), TlsError> {
    enforce_public_key_match(
        &certificate.algorithm_oid,
        &certificate.public_key,
        private_key,
        key_path,
    )
}

fn enforce_public_key_match(
    algorithm_oid: &str,
    public_key: &[u8],
    private_key: &PrivateKeyDer<'static>,
    key_path: &Path,
) -> Result<(), TlsError> {
    let key_bytes = private_key.secret_der();

    match algorithm_oid {
        "1.3.101.112" => {
            let key_pair = Ed25519KeyPair::from_pkcs8(key_bytes)
                .map_err(|_| TlsError::CertificateKeyMismatch)?;
            if key_pair.public_key().as_ref() == public_key {
                Ok(())
            } else {
                Err(TlsError::CertificateKeyMismatch)
            }
        }
        "1.2.840.10045.2.1" => {
            let rng = SystemRandom::new();
            if let Ok(key_pair) =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, key_bytes, &rng)
            {
                return compare_bytes(key_pair.public_key().as_ref(), public_key);
            }

            let rng = SystemRandom::new();
            if let Ok(key_pair) =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P384_SHA384_ASN1_SIGNING, key_bytes, &rng)
            {
                return compare_bytes(key_pair.public_key().as_ref(), public_key);
            }

            Err(TlsError::CertificateKeyMismatch)
        }
        "1.2.840.113549.1.1.1" => {
            if let Ok(key_pair) = RsaKeyPair::from_pkcs8(key_bytes) {
                return compare_rsa_public_key(&key_pair, public_key)
                    .map_err(|_| TlsError::CertificateKeyMismatch);
            }
            if let Ok(key_pair) = RsaKeyPair::from_der(key_bytes) {
                return compare_rsa_public_key(&key_pair, public_key)
                    .map_err(|_| TlsError::CertificateKeyMismatch);
            }
            Err(TlsError::InvalidPrivateKey { path: key_path.to_path_buf(), source: None })
        }
        _ => Ok(()),
    }
}

fn compare_bytes(expected: &[u8], actual: &[u8]) -> Result<(), TlsError> {
    if expected == actual {
        Ok(())
    } else {
        Err(TlsError::CertificateKeyMismatch)
    }
}

fn compare_rsa_public_key(key_pair: &RsaKeyPair, public_key: &[u8]) -> Result<(), anyhow::Error> {
    let subject_blocks = simple_asn1::from_der(public_key)?;
    let subject_seq = match subject_blocks.first() {
        Some(ASN1Block::Sequence(_, items)) => items,
        _ => return Err(anyhow!("RSA public key is not a sequence")),
    };

    if subject_seq.len() < 2 {
        return Err(anyhow!("RSA public key missing modulus/exponent"));
    }

    let subject_modulus = match &subject_seq[0] {
        ASN1Block::Integer(_, value) => bigint_to_bytes(value),
        _ => return Err(anyhow!("RSA modulus missing")),
    };

    let subject_exponent = match &subject_seq[1] {
        ASN1Block::Integer(_, value) => bigint_to_bytes(value),
        _ => return Err(anyhow!("RSA exponent missing")),
    };

    let key_blocks = simple_asn1::from_der(key_pair.public().as_ref())?;
    let key_seq = match key_blocks.first() {
        Some(ASN1Block::Sequence(_, items)) => items,
        _ => return Err(anyhow!("RSA key is not a sequence")),
    };

    if key_seq.len() < 2 {
        return Err(anyhow!("RSA key missing modulus/exponent"));
    }

    let key_modulus = match &key_seq[0] {
        ASN1Block::Integer(_, value) => bigint_to_bytes(value),
        _ => return Err(anyhow!("RSA key modulus missing")),
    };

    let key_exponent = match &key_seq[1] {
        ASN1Block::Integer(_, value) => bigint_to_bytes(value),
        _ => return Err(anyhow!("RSA key exponent missing")),
    };

    if subject_modulus == key_modulus && subject_exponent == key_exponent {
        Ok(())
    } else {
        Err(anyhow!("RSA key mismatch"))
    }
}

fn bigint_to_bytes(value: &BigInt) -> Vec<u8> {
    value.to_biguint().map_or_else(Vec::new, |v| v.to_bytes_be())
}

fn oid_to_string(oid: &OID) -> String {
    oid.as_vec::<u64>()
        .map(|components| {
            components.into_iter().map(|n| n.to_string()).collect::<Vec<_>>().join(".")
        })
        .unwrap_or_else(|_| "unknown".to_string())
}

fn current_time() -> DateTime<Utc> {
    #[cfg(test)]
    {
        if let Some(now) = NOW_OVERRIDE.lock().unwrap().as_ref() {
            return now.clone();
        }
    }
    Utc::now()
}

#[cfg(test)]
static NOW_OVERRIDE: Mutex<Option<DateTime<Utc>>> = Mutex::new(None);

#[cfg(test)]
pub fn set_mock_time(moment: Option<DateTime<Utc>>) {
    *NOW_OVERRIDE.lock().unwrap() = moment;
}
