//! macOS Keychain access for provider and MCP credentials.
//!
//! Secrets are never written to configuration, SQLite, logs, or error text.

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CredentialError {
    #[error("credential is empty or too long")]
    InvalidSecret,
    #[error("credential storage is unavailable on this platform")]
    Unsupported,
    #[error("the macOS Keychain could not be accessed")]
    Keychain,
}

const SERVICE: &str = "com.nexus.app.credentials";
const MAX_SECRET_BYTES: usize = 16_384;

pub fn provider_account(profile_id: u64, origin: &str) -> String {
    format!("provider:{profile_id}|{origin}")
}

pub fn mcp_account(server_id: u64, origin: &str) -> String {
    format!("mcp:{server_id}|{origin}")
}

fn validate_secret(secret: &str) -> Result<&str, CredentialError> {
    let secret = secret.trim();
    if secret.is_empty() || secret.len() > MAX_SECRET_BYTES {
        return Err(CredentialError::InvalidSecret);
    }
    Ok(secret)
}

#[cfg(target_os = "macos")]
pub fn store(account: &str, secret: &str) -> Result<(), CredentialError> {
    let secret = validate_secret(secret)?;
    let entry = keyring::Entry::new(SERVICE, account).map_err(|_| CredentialError::Keychain)?;
    entry
        .set_password(secret)
        .map_err(|_| CredentialError::Keychain)
}

#[cfg(target_os = "macos")]
pub fn load(account: &str) -> Result<Option<String>, CredentialError> {
    let entry = keyring::Entry::new(SERVICE, account).map_err(|_| CredentialError::Keychain)?;
    match entry.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(CredentialError::Keychain),
    }
}

#[cfg(target_os = "macos")]
pub fn delete(account: &str) -> Result<(), CredentialError> {
    let entry = keyring::Entry::new(SERVICE, account).map_err(|_| CredentialError::Keychain)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(CredentialError::Keychain),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn store(_account: &str, secret: &str) -> Result<(), CredentialError> {
    validate_secret(secret)?;
    Err(CredentialError::Unsupported)
}

#[cfg(not(target_os = "macos"))]
pub fn load(_account: &str) -> Result<Option<String>, CredentialError> {
    Err(CredentialError::Unsupported)
}

#[cfg(not(target_os = "macos"))]
pub fn delete(_account: &str) -> Result<(), CredentialError> {
    Err(CredentialError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_are_scoped_to_profile_and_origin() {
        assert_eq!(
            provider_account(1, "https://a.example"),
            "provider:1|https://a.example"
        );
        assert_ne!(
            provider_account(1, "https://a.example"),
            provider_account(2, "https://a.example")
        );
        assert_ne!(
            provider_account(1, "https://a.example"),
            provider_account(1, "https://b.example")
        );
        assert_eq!(
            mcp_account(3, "http://127.0.0.1:9000"),
            "mcp:3|http://127.0.0.1:9000"
        );
    }

    #[test]
    fn empty_and_oversized_secrets_are_rejected() {
        assert_eq!(validate_secret("   "), Err(CredentialError::InvalidSecret));
        let oversized = "x".repeat(MAX_SECRET_BYTES + 1);
        assert_eq!(
            validate_secret(&oversized),
            Err(CredentialError::InvalidSecret)
        );
    }
}
