use std::error::Error;
use std::fmt::{self, Debug, Display, Formatter};

use aes_gcm::aead::{Aead, Generate, KeyInit, Payload, consts::U12};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE;
use music_application::assistant::{
    EncryptedProviderCredential, ProviderCredentialCipher, ProviderCredentialError, ProviderSecret,
};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

const MASTER_KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const AAD_PREFIX: &[u8] = b"assistant-provider-credential/v1:";
const REDACTED_PREFIX: &str = "••••";
const PASSWORD_MEMORY_KIB: u32 = 65_536;
const PASSWORD_ITERATIONS: u32 = 3;
const PASSWORD_LANES: u32 = 4;
const PASSWORD_OUTPUT_BYTES: usize = 32;

type CredentialNonce = Nonce<U12>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    InvalidMasterKey,
    NonAsciiConnectionId,
    EmptyCredential,
    CredentialEncryptionFailed,
    CredentialUnreadable,
    PasswordHashFailed,
    InvalidPasswordHash,
    PasswordVerificationFailed,
}

impl Display for CryptoError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidMasterKey => "credential master key is invalid",
            Self::NonAsciiConnectionId => "credential connection ID must be ASCII",
            Self::EmptyCredential => "credential must not be empty",
            Self::CredentialEncryptionFailed => "credential encryption failed",
            Self::CredentialUnreadable => "credential is unreadable",
            Self::PasswordHashFailed => "password hashing failed",
            Self::InvalidPasswordHash => "stored password hash is invalid",
            Self::PasswordVerificationFailed => "password verification failed",
        };
        formatter.write_str(message)
    }
}

impl Error for CryptoError {}

pub struct SecretString(Zeroizing<String>);

impl SecretString {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    #[must_use]
    pub fn expose_secret(&self) -> &str {
        self.0.as_str()
    }
}

impl Debug for SecretString {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedCredential {
    pub ciphertext: String,
    pub nonce: String,
    pub hint: String,
}

pub struct CredentialVault {
    cipher: Aes256Gcm,
    key_id: String,
}

impl Debug for CredentialVault {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialVault")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl CredentialVault {
    pub fn from_encoded_key(encoded_key: &str) -> Result<Self, CryptoError> {
        let decoded = Zeroizing::new(
            URL_SAFE
                .decode(encoded_key.trim())
                .map_err(|_| CryptoError::InvalidMasterKey)?,
        );
        let key: [u8; MASTER_KEY_BYTES] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::InvalidMasterKey)?;
        Self::from_key(key)
    }

    pub fn from_key(key: [u8; MASTER_KEY_BYTES]) -> Result<Self, CryptoError> {
        let key = Zeroizing::new(key);
        let digest = Sha256::digest(key.as_ref());
        let cipher =
            Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| CryptoError::InvalidMasterKey)?;
        let mut key_id = String::with_capacity(16);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in &digest[..8] {
            key_id.push(char::from(HEX[usize::from(byte >> 4)]));
            key_id.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Ok(Self { cipher, key_id })
    }

    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn encrypt(
        &self,
        connection_id: &str,
        api_key: &str,
    ) -> Result<EncryptedCredential, CryptoError> {
        let nonce = CredentialNonce::generate();
        self.encrypt_with_nonce(connection_id, api_key, &nonce)
    }

    pub fn decrypt(
        &self,
        connection_id: &str,
        ciphertext: &str,
        nonce: &str,
    ) -> Result<SecretString, CryptoError> {
        let aad = associated_data(connection_id)?;
        let encrypted = URL_SAFE
            .decode(ciphertext)
            .map_err(|_| CryptoError::CredentialUnreadable)?;
        let nonce_bytes = URL_SAFE
            .decode(nonce)
            .map_err(|_| CryptoError::CredentialUnreadable)?;
        let nonce_bytes: [u8; NONCE_BYTES] = nonce_bytes
            .try_into()
            .map_err(|_| CryptoError::CredentialUnreadable)?;
        let nonce = CredentialNonce::from(nonce_bytes);
        let mut plaintext = self
            .cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &encrypted,
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::CredentialUnreadable)?;
        let cleartext = match std::str::from_utf8(&plaintext) {
            Ok(value) => value.to_owned(),
            Err(_) => {
                plaintext.zeroize();
                return Err(CryptoError::CredentialUnreadable);
            }
        };
        plaintext.zeroize();
        Ok(SecretString(Zeroizing::new(cleartext)))
    }

    fn encrypt_with_nonce(
        &self,
        connection_id: &str,
        api_key: &str,
        nonce: &CredentialNonce,
    ) -> Result<EncryptedCredential, CryptoError> {
        let secret = api_key.trim();
        if secret.is_empty() {
            return Err(CryptoError::EmptyCredential);
        }
        let aad = associated_data(connection_id)?;
        let encrypted = self
            .cipher
            .encrypt(
                nonce,
                Payload {
                    msg: secret.as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::CredentialEncryptionFailed)?;
        Ok(EncryptedCredential {
            ciphertext: URL_SAFE.encode(encrypted),
            nonce: URL_SAFE.encode(&nonce[..]),
            hint: credential_hint(secret),
        })
    }
}

impl ProviderCredentialCipher for CredentialVault {
    fn encrypt(
        &self,
        connection_id: &str,
        api_key: &str,
    ) -> Result<EncryptedProviderCredential, ProviderCredentialError> {
        CredentialVault::encrypt(self, connection_id, api_key)
            .map(|encrypted| EncryptedProviderCredential {
                ciphertext: encrypted.ciphertext,
                nonce: encrypted.nonce,
                hint: encrypted.hint,
            })
            .map_err(provider_credential_error)
    }

    fn decrypt(
        &self,
        connection_id: &str,
        ciphertext: &str,
        nonce: &str,
    ) -> Result<ProviderSecret, ProviderCredentialError> {
        CredentialVault::decrypt(self, connection_id, ciphertext, nonce)
            .map(|secret| ProviderSecret::new(secret.expose_secret()))
            .map_err(provider_credential_error)
    }
}

fn provider_credential_error(error: CryptoError) -> ProviderCredentialError {
    let code = match error {
        CryptoError::InvalidMasterKey => "invalid_master_key",
        CryptoError::NonAsciiConnectionId
        | CryptoError::EmptyCredential
        | CryptoError::CredentialEncryptionFailed
        | CryptoError::CredentialUnreadable
        | CryptoError::PasswordHashFailed
        | CryptoError::InvalidPasswordHash
        | CryptoError::PasswordVerificationFailed => "credential_unreadable",
    };
    ProviderCredentialError {
        code: code.to_owned(),
    }
}

pub fn hash_password(password: &str) -> Result<String, CryptoError> {
    password_argon2()?
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|_| CryptoError::PasswordHashFailed)
}

pub fn verify_password(encoded_hash: &str, password: &str) -> Result<bool, CryptoError> {
    let parsed = PasswordHash::new(encoded_hash).map_err(|_| CryptoError::InvalidPasswordHash)?;
    match password_argon2()?.verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::PasswordInvalid) => Ok(false),
        Err(_) => Err(CryptoError::PasswordVerificationFailed),
    }
}

fn password_argon2() -> Result<Argon2<'static>, CryptoError> {
    let parameters = Params::new(
        PASSWORD_MEMORY_KIB,
        PASSWORD_ITERATIONS,
        PASSWORD_LANES,
        Some(PASSWORD_OUTPUT_BYTES),
    )
    .map_err(|_| CryptoError::PasswordHashFailed)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, parameters))
}

fn associated_data(connection_id: &str) -> Result<Vec<u8>, CryptoError> {
    if !connection_id.is_ascii() {
        return Err(CryptoError::NonAsciiConnectionId);
    }
    let mut aad = Vec::with_capacity(AAD_PREFIX.len() + connection_id.len());
    aad.extend_from_slice(AAD_PREFIX);
    aad.extend_from_slice(connection_id.as_bytes());
    Ok(aad)
}

fn credential_hint(secret: &str) -> String {
    let suffix = if secret.chars().count() > 4 {
        secret
            .chars()
            .rev()
            .take(4)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    } else {
        String::new()
    };
    format!("{REDACTED_PREFIX}{suffix}")
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use aes_gcm::Nonce;
    use aes_gcm::aead::consts::U12;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE;
    use serde::Deserialize;

    use super::{CredentialVault, hash_password, verify_password};

    const COMPATIBILITY_DATA: &str =
        include_str!("../../../contracts/reference/v1/compatibility-data.json");

    #[derive(Deserialize)]
    struct CompatibilityFixture {
        aes_256_gcm: CredentialFixture,
        argon2id: PasswordFixture,
    }

    #[derive(Deserialize)]
    struct CredentialFixture {
        ciphertext_urlsafe_base64: String,
        connection_id: String,
        key_id: String,
        key_urlsafe_base64: String,
        nonce_urlsafe_base64: String,
        plaintext: String,
    }

    #[derive(Deserialize)]
    struct PasswordFixture {
        invalid_password: String,
        password: String,
        phc: String,
    }

    fn fixture() -> Result<CompatibilityFixture, serde_json::Error> {
        serde_json::from_str(COMPATIBILITY_DATA)
    }

    #[test]
    fn decrypts_and_recreates_python_aes_gcm_fixture() -> Result<(), Box<dyn Error>> {
        let fixture = fixture()?.aes_256_gcm;
        let vault = CredentialVault::from_encoded_key(&fixture.key_urlsafe_base64)?;
        assert_eq!(vault.key_id(), fixture.key_id);
        assert_eq!(
            vault
                .decrypt(
                    &fixture.connection_id,
                    &fixture.ciphertext_urlsafe_base64,
                    &fixture.nonce_urlsafe_base64,
                )?
                .expose_secret(),
            fixture.plaintext
        );

        let nonce = URL_SAFE.decode(&fixture.nonce_urlsafe_base64)?;
        let nonce: [u8; 12] = nonce.try_into().map_err(|_| "fixture nonce length")?;
        let encrypted = vault.encrypt_with_nonce(
            &fixture.connection_id,
            &fixture.plaintext,
            &Nonce::<U12>::from(nonce),
        )?;
        assert_eq!(encrypted.ciphertext, fixture.ciphertext_urlsafe_base64);
        assert_eq!(encrypted.nonce, fixture.nonce_urlsafe_base64);
        assert_eq!(encrypted.hint, "••••cret");
        Ok(())
    }

    #[test]
    fn rejects_wrong_aad_and_redacts_debug_output() -> Result<(), Box<dyn Error>> {
        let fixture = fixture()?.aes_256_gcm;
        let vault = CredentialVault::from_encoded_key(&fixture.key_urlsafe_base64)?;
        assert!(
            vault
                .decrypt(
                    "different-connection",
                    &fixture.ciphertext_urlsafe_base64,
                    &fixture.nonce_urlsafe_base64,
                )
                .is_err()
        );
        let secret = vault.decrypt(
            &fixture.connection_id,
            &fixture.ciphertext_urlsafe_base64,
            &fixture.nonce_urlsafe_base64,
        )?;
        assert_eq!(format!("{secret:?}"), "SecretString([REDACTED])");
        Ok(())
    }

    #[test]
    fn verifies_python_argon2id_fixture() -> Result<(), Box<dyn Error>> {
        let fixture = fixture()?.argon2id;
        assert!(verify_password(&fixture.phc, &fixture.password)?);
        assert!(!verify_password(&fixture.phc, &fixture.invalid_password)?);
        Ok(())
    }

    #[test]
    fn hashes_new_passwords_with_a_verifiable_phc_string() -> Result<(), Box<dyn Error>> {
        let hash = hash_password("new-fixture-password")?;
        assert!(hash.starts_with("$argon2id$v=19$m=65536,t=3,p=4$"));
        assert!(verify_password(&hash, "new-fixture-password")?);
        assert!(!verify_password(&hash, "wrong")?);
        Ok(())
    }
}
