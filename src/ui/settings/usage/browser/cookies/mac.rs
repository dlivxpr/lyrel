//! macOS 官方 Keychain 授权与 Chromium v10 Cookie 解密；不创建或修改钥匙串条目。

use super::super::{Browser, BrowserError};
use sha2::{Digest, Sha256};
use std::ffi::{c_char, c_int, c_uint, c_void};
use std::ptr;
use zeroize::Zeroizing;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecClass: *const c_void;
    static kSecClassGenericPassword: *const c_void;
    static kSecAttrService: *const c_void;
    static kSecAttrAccount: *const c_void;
    static kSecReturnData: *const c_void;
    static kSecMatchLimit: *const c_void;
    static kSecMatchLimitOne: *const c_void;
    static kSecUseAuthenticationUI: *const c_void;
    static kSecUseAuthenticationUIAllow: *const c_void;
    static kSecUseAuthenticationUISkip: *const c_void;
    fn SecItemCopyMatching(query: *const c_void, result: *mut *const c_void) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: *const c_void;
    // 只传递官方回调结构的地址；不解释其私有布局。
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFStringCreateWithBytes(
        allocator: *const c_void,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> *const c_void;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *const c_void;
    fn CFGetTypeID(value: *const c_void) -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFDataGetLength(data: *const c_void) -> isize;
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFRelease(value: *const c_void);
}

#[link(name = "System")]
unsafe extern "C" {
    fn CCKeyDerivationPBKDF(
        algorithm: c_uint,
        password: *const c_char,
        password_length: usize,
        salt: *const u8,
        salt_length: usize,
        prf: c_uint,
        rounds: c_uint,
        key: *mut u8,
        key_length: usize,
    ) -> c_int;
    fn CCCrypt(
        operation: c_uint,
        algorithm: c_uint,
        options: c_uint,
        key: *const c_void,
        key_length: usize,
        iv: *const c_void,
        input: *const c_void,
        input_length: usize,
        output: *mut c_void,
        output_capacity: usize,
        output_length: *mut usize,
    ) -> c_int;
}

struct Owned(*const c_void);

impl Owned {
    fn new(value: *const c_void) -> Result<Self, BrowserError> {
        if value.is_null() {
            Err(BrowserError::Unreadable)
        } else {
            Ok(Self(value))
        }
    }

    fn string(value: &str) -> Result<Self, BrowserError> {
        // kCFStringEncodingUTF8；服务名与账户名都是固定的非秘密字符串。
        Self::new(unsafe {
            CFStringCreateWithBytes(
                ptr::null(),
                value.as_ptr(),
                value.len() as isize,
                0x0800_0100,
                0,
            )
        })
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

pub(super) fn password(
    browser: Browser,
    interactive: bool,
) -> Result<Zeroizing<Vec<u8>>, BrowserError> {
    let (service, account) = match browser {
        Browser::Chrome => ("Chrome Safe Storage", "Chrome"),
        Browser::Edge => ("Microsoft Edge Safe Storage", "Microsoft Edge"),
        Browser::Brave => ("Brave Safe Storage", "Brave"),
        Browser::Chromium => ("Chromium Safe Storage", "Chromium"),
        Browser::Firefox => return Err(BrowserError::Unsupported),
    };
    let service = Owned::string(service)?;
    let account = Owned::string(account)?;
    // SecItemCopyMatching 是唯一的系统交互。后台显式 UISkip，不使用命令行
    // security、不解锁钥匙串、不改 ACL、不在失败时改用可弹窗的 API。
    let (keys, values) = unsafe {
        (
            [
                kSecClass,
                kSecAttrService,
                kSecAttrAccount,
                kSecReturnData,
                kSecMatchLimit,
                kSecUseAuthenticationUI,
            ],
            [
                kSecClassGenericPassword,
                service.0,
                account.0,
                kCFBooleanTrue,
                kSecMatchLimitOne,
                if interactive {
                    kSecUseAuthenticationUIAllow
                } else {
                    kSecUseAuthenticationUISkip
                },
            ],
        )
    };
    let query = Owned::new(unsafe {
        CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        )
    })?;
    let mut result = ptr::null();
    let status = unsafe { SecItemCopyMatching(query.0, &mut result) };
    if status != 0 {
        if !result.is_null() {
            drop(Owned(result));
        }
        return Err(match status {
            // errSecUserCanceled / AuthFailed / InteractionNotAllowed /
            // ItemNotFound（UISkip 会隐藏需要授权的条目）/ NotAvailable。
            -128 | -25293 | -25308 | -25300 | -25291 => BrowserError::Denied,
            _ => BrowserError::Unreadable,
        });
    }
    let data = Owned::new(result)?;
    if unsafe { CFGetTypeID(data.0) != CFDataGetTypeID() } {
        return Err(BrowserError::Unreadable);
    }
    let length = unsafe { CFDataGetLength(data.0) };
    if !(1..=64 * 1024).contains(&length) {
        return Err(BrowserError::Unreadable);
    }
    let bytes = unsafe { CFDataGetBytePtr(data.0) };
    if bytes.is_null() {
        return Err(BrowserError::Unreadable);
    }
    Ok(Zeroizing::new(
        unsafe { std::slice::from_raw_parts(bytes, length as usize) }.to_vec(),
    ))
}

pub(super) fn decrypt(
    encrypted: &[u8],
    password: &[u8],
    host: &str,
    version: i64,
) -> Result<Zeroizing<String>, BrowserError> {
    let ciphertext = encrypted
        .strip_prefix(b"v10")
        .ok_or(BrowserError::Unsupported)?;
    if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
        return Err(BrowserError::Unreadable);
    }
    let mut key = Zeroizing::new([0u8; 16]);
    // Chromium macOS v10：PBKDF2-HMAC-SHA1, saltysalt, 1003 次, AES-128-CBC。
    let status = unsafe {
        CCKeyDerivationPBKDF(
            2,
            password.as_ptr().cast(),
            password.len(),
            b"saltysalt".as_ptr(),
            9,
            1,
            1003,
            key.as_mut_ptr(),
            key.len(),
        )
    };
    if status != 0 {
        return Err(BrowserError::Unreadable);
    }
    let iv = [0x20u8; 16];
    let mut plaintext = Zeroizing::new(vec![0u8; ciphertext.len()]);
    let mut length = 0;
    // kCCDecrypt=1, kCCAlgorithmAES=0, kCCOptionPKCS7Padding=1。
    let status = unsafe {
        CCCrypt(
            1,
            0,
            1,
            key.as_ptr().cast(),
            key.len(),
            iv.as_ptr().cast(),
            ciphertext.as_ptr().cast(),
            ciphertext.len(),
            plaintext.as_mut_ptr().cast(),
            plaintext.len(),
            &mut length,
        )
    };
    if status != 0 || length > plaintext.len() {
        return Err(BrowserError::Unreadable);
    }
    let bytes = &plaintext[..length];
    let bytes = if version >= 24 {
        // v24 强制域名绑定：先验证 SHA-256(host_key)，再剥离，不猜前缀。
        let digest = Sha256::digest(host.as_bytes());
        if bytes.len() < 32 || bytes[..32] != digest[..] {
            return Err(BrowserError::Unreadable);
        }
        &bytes[32..]
    } else {
        bytes
    };
    let text = std::str::from_utf8(bytes).map_err(|_| BrowserError::Unreadable)?;
    Ok(Zeroizing::new(text.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(hex: &str) -> Vec<u8> {
        let mut bytes = b"v10".to_vec();
        for pair in hex.as_bytes().chunks_exact(2) {
            bytes.push(u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap());
        }
        bytes
    }

    #[test]
    fn chromium_legacy_and_domain_bound_ciphertexts_decode_without_keychain_access() {
        // 独立 WebCrypto PBKDF2/AES-CBC 生成的隔离固定向量，不读取系统钥匙串。
        let old = fixture("fcd2ce06b826b3276034d811a079d21e6f767004236d7ab48e1d95ffa170552a");
        let current = fixture(
            "25eac2f932956f0265764aa4193049df65c01b98f13af13a7b3e5f4079c929ec22db767437b350c64171c3b4db67dd87ec1ece8ad54d9fd3531e33a96e819c21",
        );
        assert_eq!(
            decrypt(&old, b"isolated-test-password", ".stepfun.com", 23)
                .unwrap()
                .as_str(),
            "fixture-encrypted-token"
        );
        assert_eq!(
            decrypt(&current, b"isolated-test-password", ".stepfun.com", 24)
                .unwrap()
                .as_str(),
            "fixture-encrypted-token"
        );
        assert!(matches!(
            decrypt(&current, b"isolated-test-password", ".xiaomimimo.com", 24),
            Err(BrowserError::Unreadable)
        ));
        assert!(matches!(
            decrypt(&old, b"isolated-test-password", ".stepfun.com", 24),
            Err(BrowserError::Unreadable)
        ));
        assert!(matches!(
            decrypt(&current, b"wrong-password", ".stepfun.com", 24),
            Err(BrowserError::Unreadable)
        ));
    }
}
