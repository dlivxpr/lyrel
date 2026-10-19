//! Windows Chromium 的同用户 DPAPI 与 AES-GCM；绝不调用 ABE 或可弹窗 API。

use super::{BrowserError, Profile, decode_plaintext, read_file};
use aes_gcm::{Aes256Gcm, Nonce, aead::{AeadInOut, KeyInit}};
use base64::Engine as _;
use std::ptr;
use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
};
use zeroize::{Zeroize, Zeroizing};

pub(super) struct Crypt {
    wrapped_key: Option<Zeroizing<Vec<u8>>>,
    key: Option<Zeroizing<Vec<u8>>>,
}

impl Crypt {
    pub(super) fn new(profile: &Profile) -> Result<Self, BrowserError> {
        let root = profile.path.parent().ok_or(BrowserError::Unreadable)?;
        let bytes = match read_file(root, &root.join("Local State"), 4 * 1024 * 1024) {
            Ok(bytes) => Some(bytes),
            Err(BrowserError::NoSession) => None,
            Err(error) => return Err(error),
        };
        let mut wrapped_key = None;
        if let Some(bytes) = bytes {
            let state: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| BrowserError::Unreadable)?;
            if let Some(crypt) = state.get("os_crypt") {
                if crypt.get("app_bound_encrypted_key").is_some() {
                    return Err(BrowserError::Protected);
                }
                if let Some(encoded) = crypt.get("encrypted_key") {
                    let encoded = encoded.as_str().ok_or(BrowserError::Unreadable)?;
                    let decoded = Zeroizing::new(base64::engine::general_purpose::STANDARD
                        .decode(encoded).map_err(|_| BrowserError::Unreadable)?);
                    if decoded.starts_with(b"APPB") {
                        return Err(BrowserError::Protected);
                    }
                    if !decoded.starts_with(b"DPAPI") || decoded.len() <= 5 {
                        return Err(BrowserError::Unreadable);
                    }
                    wrapped_key = Some(decoded);
                }
            }
        }
        Ok(Self { wrapped_key, key: None })
    }

    pub(super) fn decrypt(&mut self, encrypted: &[u8], host: &str, version: i64)
        -> Result<Zeroizing<String>, BrowserError>
    {
        if encrypted.starts_with(b"v20") || encrypted.starts_with(b"APPB") {
            return Err(BrowserError::Protected);
        }
        let mut plaintext = if encrypted.starts_with(b"v10") || encrypted.starts_with(b"v11") {
            if encrypted.len() < 3 + 12 + 16 {
                return Err(BrowserError::Unreadable);
            }
            if self.key.is_none() {
                let wrapped = self.wrapped_key.as_ref().ok_or(BrowserError::Unsupported)?;
                let key = unprotect(&wrapped[5..])?;
                if key.len() != 32 {
                    return Err(BrowserError::Unreadable);
                }
                self.key = Some(key);
            }
            let cipher = Aes256Gcm::new_from_slice(self.key.as_ref().ok_or(BrowserError::Unreadable)?)
                .map_err(|_| BrowserError::Unreadable)?;
            let nonce = Nonce::from(<[u8; 12]>::try_from(&encrypted[3..15])
                .map_err(|_| BrowserError::Unreadable)?);
            let mut bytes = Zeroizing::new(encrypted[15..].to_vec());
            cipher.decrypt_in_place(&nonce, b"", &mut *bytes)
                .map_err(|_| BrowserError::Unreadable)?;
            bytes
        } else {
            // 未知带版本前缀不得误当传统 DPAPI。
            if encrypted.first() == Some(&b'v') {
                return Err(BrowserError::Unsupported);
            }
            unprotect(encrypted)?
        };
        let text = decode_plaintext(&plaintext, host, version)?;
        plaintext.zeroize();
        Ok(text)
    }
}

fn unprotect(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>, BrowserError> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(|_| BrowserError::Unreadable)?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // 不传 prompt/description，不解锁凭据，不提升权限；交互导入也禁止 UI。
    let success = unsafe {
        CryptUnprotectData(&input, ptr::null_mut(), ptr::null(), ptr::null(), ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN, &mut output)
    };
    let error = if success == 0 { Some(unsafe { GetLastError() }) } else { None };
    let result = if let Some(error) = error {
        Err(match error {
            5 | 86 | 1325 => BrowserError::Denied,
            _ => BrowserError::Unreadable,
        })
    } else if output.pbData.is_null() || output.cbData > 64 * 1024 + 128 {
        Err(BrowserError::Unreadable)
    } else {
        Ok(Zeroizing::new(unsafe {
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
        }))
    };
    if !output.pbData.is_null() {
        // DPAPI 的 LocalAlloc 缓冲区也在释放前擦除。
        unsafe {
            std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize).zeroize();
            LocalFree(output.pbData.cast());
        }
    }
    result
}
