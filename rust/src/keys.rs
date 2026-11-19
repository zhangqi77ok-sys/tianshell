//! Turn everyday OpenSSH / PEM private keys into material libssh2 can load.
//!
//! libssh2's `userauth_pubkey_file` asks OpenSSL to parse the file. OpenSSL does
//! not understand `BEGIN OPENSSH PRIVATE KEY` (the format Windows `ssh-keygen`
//! writes for ed25519, including after `ssh-keygen -p -m PEM`). That shows up as
//! `[Session(-1)] unknown error`. We parse those keys here, decrypt them when a
//! passphrase is set, and hand libssh2 a PKCS#8 PEM instead.

use base64::Engine;
use pkcs8::EncodePrivateKey;
use rsa::{BigUint, RsaPrivateKey};
use ssh_key::private::KeypairData;
use ssh_key::PrivateKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFormat {
    /// OpenSSH private key converted to unencrypted PKCS#8.
    OpensshConverted,
    /// `BEGIN PRIVATE KEY` already suitable for OpenSSL.
    Pkcs8,
    /// `BEGIN RSA PRIVATE KEY`.
    Pkcs1Rsa,
    /// `BEGIN EC PRIVATE KEY`.
    EcPem,
    /// OpenSSL `ENCRYPTED PRIVATE KEY` or legacy `Proc-Type: 4,ENCRYPTED`.
    EncryptedPem,
}

#[derive(Debug, Clone)]
pub struct PreparedKey {
    pub material: String,
    /// Passphrase libssh2 must still apply. `None` when this crate already decrypted the key.
    pub passphrase: Option<String>,
    pub format: KeyFormat,
    pub algorithm: String,
}

pub fn prepare_private_key(raw: &str, passphrase: Option<&str>) -> Result<PreparedKey, String> {
    let text = normalize(raw);
    if text.is_empty() {
        return Err("私钥是空的".into());
    }
    if text.contains("PuTTY-User-Key-File-") {
        return Err("暂不支持 PuTTY .ppk。请在本机用 ssh-keygen 转成 OpenSSH 私钥后再粘贴。".into());
    }
    if is_public_only(&text) {
        return Err("这是公钥，请粘贴私钥（BEGIN OPENSSH PRIVATE KEY、BEGIN PRIVATE KEY 或 BEGIN RSA PRIVATE KEY）。".into());
    }
    if text.contains("BEGIN OPENSSH PRIVATE KEY") {
        return prepare_openssh(&slice_from(&text, "-----BEGIN OPENSSH PRIVATE KEY-----"), passphrase);
    }
    if text.contains("BEGIN ENCRYPTED PRIVATE KEY") || text.contains("Proc-Type: 4,ENCRYPTED") {
        let pass = required_passphrase(passphrase)?;
        return Ok(PreparedKey {
            material: ensure_newline(slice_from(&text, "-----BEGIN")),
            passphrase: Some(pass),
            format: KeyFormat::EncryptedPem,
            algorithm: "encrypted-pem".into(),
        });
    }
    if text.contains("BEGIN RSA PRIVATE KEY") {
        return Ok(PreparedKey {
            material: ensure_newline(slice_from(&text, "-----BEGIN RSA PRIVATE KEY-----")),
            passphrase: blank_to_none(passphrase),
            format: KeyFormat::Pkcs1Rsa,
            algorithm: "ssh-rsa".into(),
        });
    }
    if text.contains("BEGIN EC PRIVATE KEY") {
        return Ok(PreparedKey {
            material: ensure_newline(slice_from(&text, "-----BEGIN EC PRIVATE KEY-----")),
            passphrase: blank_to_none(passphrase),
            format: KeyFormat::EcPem,
            algorithm: "ecdsa".into(),
        });
    }
    if text.contains("BEGIN PRIVATE KEY") {
        let body = ensure_newline(slice_from(&text, "-----BEGIN PRIVATE KEY-----"));
        let algorithm = pkcs8_algorithm(&body);
        return Ok(PreparedKey {
            material: body,
            passphrase: None,
            format: KeyFormat::Pkcs8,
            algorithm,
        });
    }
    Err("无法识别私钥格式。支持 OpenSSH（含 ed25519，可带口令）、PKCS#8、PEM RSA 和 EC PEM。不支持 .ppk 和 FIDO/sk 安全密钥。".into())
}

fn prepare_openssh(text: &str, passphrase: Option<&str>) -> Result<PreparedKey, String> {
    let key = PrivateKey::from_openssh(text).map_err(|err| format!("无法解析 OpenSSH 私钥: {err}"))?;
    let key = if key.is_encrypted() {
        let pass = required_passphrase(passphrase)?;
        key.decrypt(pass).map_err(|_| "私钥口令不正确".to_string())?
    } else {
        key
    };
    let algorithm = algorithm_name(&key);
    if algorithm.contains("sk-") {
        return Err(format!("FIDO 安全密钥（{algorithm}）需要硬件触碰，当前登录路径不能使用。"));
    }
    let material = match key.key_data() {
        KeypairData::Ed25519(pair) => pkcs8_ed25519(&pair.private.to_bytes()),
        KeypairData::Rsa(pair) => pkcs8_rsa(pair)?,
        KeypairData::Ecdsa(pair) => pkcs8_ecdsa(pair)?,
        _ => {
            return Err(format!(
                "这种密钥类型当前不能交给 SSH 登录使用（{algorithm}）。支持 ed25519、RSA 和 ECDSA（P-256/P-384/P-521）。"
            ));
        }
    };
    Ok(PreparedKey {
        material,
        passphrase: None,
        format: KeyFormat::OpensshConverted,
        algorithm,
    })
}

fn algorithm_name(key: &PrivateKey) -> String {
    key.algorithm().as_str().to_string()
}

fn pkcs8_ed25519(seed: &[u8; 32]) -> String {
    let oid = der_tlv(0x06, &[0x2b, 0x65, 0x70]);
    let alg = der_tlv(0x30, &oid);
    let inner = der_tlv(0x04, seed);
    let private_key = der_tlv(0x04, &inner);
    let mut body = der_tlv(0x02, &[0x00]);
    body.extend(alg);
    body.extend(private_key);
    pem_wrap("PRIVATE KEY", &der_tlv(0x30, &body))
}

fn rsa_private(pair: &ssh_key::private::RsaKeypair) -> Result<RsaPrivateKey, String> {
    let n = BigUint::from_bytes_be(positive(&pair.public.n, "RSA 模数")?);
    let e = BigUint::from_bytes_be(positive(&pair.public.e, "RSA 公钥指数")?);
    let d = BigUint::from_bytes_be(positive(&pair.private.d, "RSA 私钥指数")?);
    let p = BigUint::from_bytes_be(positive(&pair.private.p, "RSA 素数")?);
    let q = BigUint::from_bytes_be(positive(&pair.private.q, "RSA 素数")?);
    RsaPrivateKey::from_components(n, e, d, vec![p, q]).map_err(|err| format!("无法编码 RSA 私钥: {err}"))
}

fn pkcs8_rsa(pair: &ssh_key::private::RsaKeypair) -> Result<String, String> {
    let pem = rsa_private(pair)?
        .to_pkcs8_pem(pkcs8::LineEnding::LF)
        .map_err(|err| format!("无法编码 RSA PKCS#8: {err}"))?;
    Ok(pem.to_string())
}

fn pkcs8_ecdsa(pair: &ssh_key::private::EcdsaKeypair) -> Result<String, String> {
    use ssh_key::private::EcdsaKeypair;
    let (oid, scalar): (&[u8], &[u8]) = match pair {
        EcdsaKeypair::NistP256 { private, .. } => (&[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07], private.as_slice()),
        EcdsaKeypair::NistP384 { private, .. } => (&[0x2b, 0x81, 0x04, 0x00, 0x22], private.as_slice()),
        EcdsaKeypair::NistP521 { private, .. } => (&[0x2b, 0x81, 0x04, 0x00, 0x23], private.as_slice()),
    };
    let id_ec = der_tlv(0x06, &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01]);
    let curve = der_tlv(0x06, oid);
    let mut alg_body = id_ec;
    alg_body.extend(curve);
    let alg = der_tlv(0x30, &alg_body);
    let mut ec = der_integer_unsigned(&[1]);
    ec.extend(der_tlv(0x04, scalar));
    let private_key = der_tlv(0x04, &der_tlv(0x30, &ec));
    let mut body = der_tlv(0x02, &[0x00]);
    body.extend(alg);
    body.extend(private_key);
    Ok(pem_wrap("PRIVATE KEY", &der_tlv(0x30, &body)))
}

fn pkcs8_algorithm(pem: &str) -> String {
    let der = decode_pem_body(pem);
    if contains_oid(&der, &[0x06, 0x03, 0x2b, 0x65, 0x70]) {
        return "ssh-ed25519".into();
    }
    if contains_oid(&der, &[0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01]) {
        return "ssh-rsa".into();
    }
    if contains_oid(&der, &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]) {
        return "ecdsa-sha2-nistp256".into();
    }
    if contains_oid(&der, &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x22]) {
        return "ecdsa-sha2-nistp384".into();
    }
    if contains_oid(&der, &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x23]) {
        return "ecdsa-sha2-nistp521".into();
    }
    "pkcs8".into()
}

fn decode_pem_body(pem: &str) -> Vec<u8> {
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    base64::engine::general_purpose::STANDARD.decode(body).unwrap_or_default()
}

fn contains_oid(der: &[u8], oid: &[u8]) -> bool {
    der.windows(oid.len()).any(|window| window == oid)
}

fn positive<'a>(value: &'a ssh_key::Mpint, label: &str) -> Result<&'a [u8], String> {
    value.as_positive_bytes().filter(|bytes| !bytes.is_empty()).ok_or_else(|| format!("{label}无效"))
}

fn pem_wrap(label: &str, der: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

fn der_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len() + 4);
    out.push(tag);
    out.extend(der_len(content.len()));
    out.extend_from_slice(content);
    out
}

fn der_len(len: usize) -> Vec<u8> {
    if len < 0x80 {
        vec![len as u8]
    } else if len <= 0xff {
        vec![0x81, len as u8]
    } else {
        vec![0x82, (len >> 8) as u8, (len & 0xff) as u8]
    }
}

fn der_integer_unsigned(bytes: &[u8]) -> Vec<u8> {
    let mut body = bytes.to_vec();
    while body.len() > 1 && body[0] == 0 {
        body.remove(0);
    }
    if body.is_empty() {
        body.push(0);
    }
    if body[0] & 0x80 != 0 {
        body.insert(0, 0);
    }
    der_tlv(0x02, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pkcs1::EncodeRsaPrivateKey;
    use pkcs8::DecodePrivateKey;
    use rand::rngs::OsRng;
    use rsa::traits::{PrivateKeyParts, PublicKeyParts};
    use ssh_key::private::{EcdsaKeypair, KeypairData};
    use ssh_key::{Algorithm, EcdsaCurve, LineEnding, PrivateKey};

    fn openssh(algorithm: Algorithm) -> PrivateKey {
        PrivateKey::random(&mut OsRng, algorithm).expect("generate")
    }

    fn assert_pkcs8(pem: &str, algorithm: &str) {
        assert!(pem.contains("BEGIN PRIVATE KEY"), "{pem}");
        assert!(!pem.contains("OPENSSH PRIVATE KEY"), "{pem}");
        assert!(pem.contains("END PRIVATE KEY"));
        let again = prepare_private_key(pem, None).unwrap();
        assert_eq!(again.format, KeyFormat::Pkcs8);
        assert_eq!(again.algorithm, algorithm);
        let der = decode_pem_body(pem);
        assert!(der.first() == Some(&0x30), "PKCS#8 must be a DER sequence");
    }

    #[test]
    fn openssh_ed25519_becomes_pkcs8() {
        let key = openssh(Algorithm::Ed25519);
        let seed = {
            let KeypairData::Ed25519(pair) = key.key_data() else {
                panic!("expected ed25519");
            };
            pair.private.to_bytes()
        };
        let armored = key.to_openssh(LineEnding::LF).unwrap().to_string();
        assert!(armored.contains("BEGIN OPENSSH PRIVATE KEY"));
        let prepared = prepare_private_key(&armored, None).unwrap();
        assert_eq!(prepared.format, KeyFormat::OpensshConverted);
        assert_eq!(prepared.algorithm, "ssh-ed25519");
        assert!(prepared.passphrase.is_none());
        assert_pkcs8(&prepared.material, "ssh-ed25519");
        let der = decode_pem_body(&prepared.material);
        assert!(contains_oid(&der, &[0x06, 0x03, 0x2b, 0x65, 0x70]));
        assert!(der.windows(seed.len()).any(|window| window == seed), "PKCS#8 dropped the ed25519 seed");
    }

    #[test]
    fn encrypted_openssh_ed25519_needs_the_passphrase() {
        let key = openssh(Algorithm::Ed25519);
        let seed = {
            let KeypairData::Ed25519(pair) = key.key_data() else {
                panic!("expected ed25519");
            };
            pair.private.to_bytes()
        };
        let encrypted = key.encrypt(&mut OsRng, b"correct horse").unwrap();
        let armored = encrypted.to_openssh(LineEnding::LF).unwrap().to_string();
        let missing = prepare_private_key(&armored, None).unwrap_err();
        assert!(missing.contains("口令"), "{missing}");
        let wrong = prepare_private_key(&armored, Some("nope")).unwrap_err();
        assert!(wrong.contains("口令"), "{wrong}");
        let prepared = prepare_private_key(&armored, Some("correct horse")).unwrap();
        assert_eq!(prepared.format, KeyFormat::OpensshConverted);
        assert!(prepared.passphrase.is_none());
        assert_pkcs8(&prepared.material, "ssh-ed25519");
        let der = decode_pem_body(&prepared.material);
        assert!(der.windows(seed.len()).any(|window| window == seed));
    }

    #[test]
    fn openssh_rsa_and_traditional_pkcs1_are_accepted() {
        let key = openssh(Algorithm::Rsa { hash: None });
        let original = {
            let KeypairData::Rsa(pair) = key.key_data() else {
                panic!("expected rsa");
            };
            rsa_private(pair).unwrap()
        };
        let prepared = prepare_private_key(&key.to_openssh(LineEnding::LF).unwrap(), None).unwrap();
        assert_eq!(prepared.format, KeyFormat::OpensshConverted);
        assert_eq!(prepared.algorithm, "ssh-rsa");
        assert_pkcs8(&prepared.material, "ssh-rsa");
        let loaded = RsaPrivateKey::from_pkcs8_pem(&prepared.material).expect("rsa pkcs8");
        assert_eq!(loaded.n(), original.n());
        assert_eq!(loaded.e(), original.e());
        assert_eq!(loaded.d(), original.d());

        let pkcs1 = original.to_pkcs1_pem(pkcs1::LineEnding::LF).unwrap().to_string();
        assert!(pkcs1.contains("BEGIN RSA PRIVATE KEY"));
        let traditional = prepare_private_key(&pkcs1, None).unwrap();
        assert_eq!(traditional.format, KeyFormat::Pkcs1Rsa);
        assert_eq!(traditional.algorithm, "ssh-rsa");
        assert!(traditional.material.contains("BEGIN RSA PRIVATE KEY"));
    }

    #[test]
    fn openssh_p256_becomes_pkcs8() {
        let key = openssh(Algorithm::Ecdsa { curve: EcdsaCurve::NistP256 });
        let scalar = {
            let KeypairData::Ecdsa(EcdsaKeypair::NistP256 { private, .. }) = key.key_data() else {
                panic!("expected p256");
            };
            private.as_slice().to_vec()
        };
        let prepared = prepare_private_key(&key.to_openssh(LineEnding::LF).unwrap(), None).unwrap();
        assert_eq!(prepared.algorithm, "ecdsa-sha2-nistp256");
        assert_pkcs8(&prepared.material, "ecdsa-sha2-nistp256");
        let der = decode_pem_body(&prepared.material);
        assert!(contains_oid(&der, &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]));
        assert!(der.windows(scalar.len()).any(|window| window == scalar));
    }

    #[test]
    fn public_keys_and_putty_files_are_refused() {
        let key = openssh(Algorithm::Ed25519);
        let public = key.public_key().to_openssh().unwrap();
        let err = prepare_private_key(&public, None).unwrap_err();
        assert!(err.contains("公钥"), "{err}");
        let err = prepare_private_key("PuTTY-User-Key-File-3: ssh-ed25519\n", None).unwrap_err();
        assert!(err.contains(".ppk"), "{err}");
    }
}

fn required_passphrase(passphrase: Option<&str>) -> Result<String, String> {
    match passphrase.map(str::trim).filter(|item| !item.is_empty()) {
        Some(pass) => Ok(pass.to_string()),
        None => Err("这把私钥有口令，请在主机里填写私钥口令".into()),
    }
}

fn blank_to_none(passphrase: Option<&str>) -> Option<String> {
    passphrase.map(str::trim).filter(|item| !item.is_empty()).map(str::to_string)
}

fn normalize(raw: &str) -> String {
    raw.trim_start_matches('\u{feff}').replace("\r\n", "\n").trim().to_string()
}

fn slice_from(text: &str, marker: &str) -> String {
    match text.find(marker) {
        Some(index) => text[index..].trim().to_string(),
        None => text.to_string(),
    }
}

fn ensure_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn is_public_only(text: &str) -> bool {
    if text.contains("PRIVATE KEY") {
        return false;
    }
    text.contains("BEGIN OPENSSH PUBLIC KEY")
        || text.contains("BEGIN PUBLIC KEY")
        || text.starts_with("ssh-ed25519 ")
        || text.starts_with("ssh-rsa ")
        || text.starts_with("ecdsa-sha2-")
        || text.starts_with("sk-ssh-")
}
