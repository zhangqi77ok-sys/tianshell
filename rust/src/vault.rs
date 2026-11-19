use std::fs;
use std::path::Path;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use thiserror::Error;

const VAULT_VERSION: &str = "v1";
const TS1D_HEADER: &[u8; 4] = b"TS1D";
const TS1S_HEADER: &[u8; 4] = b"TS1S";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const KEY_LEN: usize = 32;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("加密钥匙长度不对，期望 32 字节")]
    InvalidKeyLength,
    #[error("凭据数据格式不对")]
    InvalidSealedFormat,
    #[error("解密凭据失败，密文已被篡改或主密钥不匹配")]
    DecryptionFailed,
    #[error("旧版 Electron TS1S 密钥已弃用，请先备份或重新生成 TS1D 密钥")]
    LegacyElectronKey,
    #[error("密钥文件损坏或格式不受支持")]
    CorruptedKeyFile,
    #[error("Windows DPAPI 保护失败: 错误码 {0}")]
    DpapiError(u32),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("Base64 解码错误: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("UTF-8 编码错误: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[cfg(target_os = "windows")]
#[link(name = "crypt32")]
#[link(name = "kernel32")]
extern "system" {
    fn CryptProtectData(
        pDataIn: *mut DataBlob,
        szDataDescr: *const u16,
        pOptionalEntropy: *mut DataBlob,
        pvReserved: *mut std::ffi::c_void,
        pPromptStruct: *mut std::ffi::c_void,
        dwFlags: u32,
        pDataOut: *mut DataBlob,
    ) -> i32;

    fn CryptUnprotectData(
        pDataIn: *mut DataBlob,
        ppszDataDescr: *mut *mut u16,
        pOptionalEntropy: *mut DataBlob,
        pvReserved: *mut std::ffi::c_void,
        pPromptStruct: *mut std::ffi::c_void,
        dwFlags: u32,
        pDataOut: *mut DataBlob,
    ) -> i32;

    fn LocalFree(hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn GetLastError() -> u32;
}

#[cfg(target_os = "windows")]
pub fn dpapi_protect(data: &[u8]) -> Result<Vec<u8>, VaultError> {
    let mut in_blob = DataBlob {
        cb_data: data.len() as u32,
        pb_data: data.as_ptr() as *mut u8,
    };
    let mut out_blob = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };
    unsafe {
        let ok = CryptProtectData(
            &mut in_blob,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut out_blob,
        );
        if ok == 0 {
            return Err(VaultError::DpapiError(GetLastError()));
        }
        let slice = std::slice::from_raw_parts(out_blob.pb_data, out_blob.cb_data as usize);
        let out = slice.to_vec();
        LocalFree(out_blob.pb_data as _);
        Ok(out)
    }
}

#[cfg(target_os = "windows")]
pub fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>, VaultError> {
    let mut in_blob = DataBlob {
        cb_data: data.len() as u32,
        pb_data: data.as_ptr() as *mut u8,
    };
    let mut out_blob = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };
    unsafe {
        let ok = CryptUnprotectData(
            &mut in_blob,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut out_blob,
        );
        if ok == 0 {
            return Err(VaultError::DpapiError(GetLastError()));
        }
        let slice = std::slice::from_raw_parts(out_blob.pb_data, out_blob.cb_data as usize);
        let out = slice.to_vec();
        LocalFree(out_blob.pb_data as _);
        Ok(out)
    }
}

#[cfg(not(target_os = "windows"))]
pub fn dpapi_protect(data: &[u8]) -> Result<Vec<u8>, VaultError> {
    Ok(data.to_vec())
}

#[cfg(not(target_os = "windows"))]
pub fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>, VaultError> {
    Ok(data.to_vec())
}

pub fn load_or_create_master_key(key_path: &Path) -> Result<[u8; KEY_LEN], VaultError> {
    if key_path.exists() {
        let raw = fs::read(key_path)?;
        if raw.starts_with(TS1S_HEADER) {
            return Err(VaultError::LegacyElectronKey);
        }
        if !raw.starts_with(TS1D_HEADER) || raw.len() <= TS1D_HEADER.len() {
            return Err(VaultError::CorruptedKeyFile);
        }
        let decrypted = dpapi_unprotect(&raw[TS1D_HEADER.len()..])?;
        if decrypted.len() != KEY_LEN {
            return Err(VaultError::CorruptedKeyFile);
        }
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(&decrypted);
        return Ok(key);
    }

    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut key = [0u8; KEY_LEN];
    rand::thread_rng().fill_bytes(&mut key);
    let protected = dpapi_protect(&key)?;
    let mut file_bytes = Vec::with_capacity(TS1D_HEADER.len() + protected.len());
    file_bytes.extend_from_slice(TS1D_HEADER);
    file_bytes.extend_from_slice(&protected);
    fs::write(key_path, file_bytes)?;
    Ok(key)
}

pub fn seal(key: &[u8; KEY_LEN], plaintext: &str) -> Result<String, VaultError> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| VaultError::InvalidKeyLength)?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext_and_tag = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|_| VaultError::DecryptionFailed)?;
    if ciphertext_and_tag.len() < TAG_LEN {
        return Err(VaultError::DecryptionFailed);
    }
    let split_pos = ciphertext_and_tag.len() - TAG_LEN;
    let ciphertext = &ciphertext_and_tag[..split_pos];
    let tag = &ciphertext_and_tag[split_pos..];

    let iv_b64 = URL_SAFE_NO_PAD.encode(nonce_bytes);
    let tag_b64 = URL_SAFE_NO_PAD.encode(tag);
    let cipher_b64 = URL_SAFE_NO_PAD.encode(ciphertext);
    Ok(format!("{}:{}:{}:{}", VAULT_VERSION, iv_b64, tag_b64, cipher_b64))
}

pub fn open(key: &[u8; KEY_LEN], sealed: &str) -> Result<String, VaultError> {
    let parts: Vec<&str> = sealed.split(':').collect();
    if parts.len() != 4 || parts[0] != VAULT_VERSION {
        return Err(VaultError::InvalidSealedFormat);
    }
    let iv_bytes = URL_SAFE_NO_PAD.decode(parts[1])?;
    let tag_bytes = URL_SAFE_NO_PAD.decode(parts[2])?;
    let cipher_bytes = URL_SAFE_NO_PAD.decode(parts[3])?;
    if iv_bytes.len() != NONCE_LEN || tag_bytes.len() != TAG_LEN {
        return Err(VaultError::InvalidSealedFormat);
    }
    let mut combined = Vec::with_capacity(cipher_bytes.len() + TAG_LEN);
    combined.extend_from_slice(&cipher_bytes);
    combined.extend_from_slice(&tag_bytes);

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| VaultError::InvalidKeyLength)?;
    let nonce = Nonce::from_slice(&iv_bytes);
    let decrypted = cipher
        .decrypt(nonce, combined.as_ref())
        .map_err(|_| VaultError::DecryptionFailed)?;
    Ok(String::from_utf8(decrypted)?)
}
