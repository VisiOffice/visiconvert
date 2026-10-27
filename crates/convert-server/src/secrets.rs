use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, AeadCore, OsRng, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use uuid::Uuid;

#[derive(Clone)]
pub struct Vault(Aes256Gcm);
impl Vault {
    pub fn from_hex(value: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()),
            "VISICONVERT_ENCRYPTION_KEY must contain 64 hexadecimal characters"
        );
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)?;
        }
        Ok(Self(
            Aes256Gcm::new_from_slice(&key)
                .map_err(|_| anyhow::anyhow!("Invalid encryption key"))?,
        ))
    }
    fn context(workspace: Uuid, account: &str) -> String {
        format!("visiconvert:twilio:v1:{workspace}:{account}")
    }
    pub fn seal(&self, workspace: Uuid, account: &str, token: &str) -> Result<String, ()> {
        self.seal_context(&Self::context(workspace, account), token)
    }
    pub fn seal_google(&self, workspace: Uuid, purpose: &str, token: &str) -> Result<String, ()> {
        self.seal_context(
            &format!("visiconvert:google:v1:{workspace}:{purpose}"),
            token,
        )
    }
    pub fn seal_signalwire(
        &self,
        workspace: Uuid,
        space: &str,
        project: Uuid,
        purpose: &str,
        value: &str,
    ) -> Result<String, ()> {
        self.seal_context(
            &format!("visiconvert:signalwire:v1:{workspace}:{space}:{project}:{purpose}"),
            value,
        )
    }
    pub fn open_signalwire(
        &self,
        workspace: Uuid,
        space: &str,
        project: Uuid,
        purpose: &str,
        value: &str,
    ) -> Result<String, ()> {
        self.open_context(
            &format!("visiconvert:signalwire:v1:{workspace}:{space}:{project}:{purpose}"),
            value,
        )
    }
    fn seal_context(&self, context: &str, token: &str) -> Result<String, ()> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let encrypted = self
            .0
            .encrypt(
                &nonce,
                Payload {
                    msg: token.as_bytes(),
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| ())?;
        let mut bytes = nonce.to_vec();
        bytes.extend(encrypted);
        Ok(format!("v1.{}", STANDARD.encode(bytes)))
    }
    pub fn open(&self, workspace: Uuid, account: &str, encrypted: &str) -> Result<String, ()> {
        self.open_context(&Self::context(workspace, account), encrypted)
    }
    pub fn open_google(
        &self,
        workspace: Uuid,
        purpose: &str,
        encrypted: &str,
    ) -> Result<String, ()> {
        self.open_context(
            &format!("visiconvert:google:v1:{workspace}:{purpose}"),
            encrypted,
        )
    }
    fn open_context(&self, context: &str, encrypted: &str) -> Result<String, ()> {
        let bytes = STANDARD
            .decode(encrypted.strip_prefix("v1.").ok_or(())?)
            .map_err(|_| ())?;
        if bytes.len() < 28 {
            return Err(());
        }
        let plain = self
            .0
            .decrypt(
                Nonce::from_slice(&bytes[..12]),
                Payload {
                    msg: &bytes[12..],
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| ())?;
        String::from_utf8(plain).map_err(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_are_bound_to_workspace_account_key_and_authenticated_ciphertext() {
        let vault = Vault::from_hex(&"a".repeat(64)).unwrap();
        let id = Uuid::new_v4();
        let token = "private-auth-token";
        let encrypted = vault.seal(id, "account", token).unwrap();
        assert!(!encrypted.contains(token));
        assert_ne!(encrypted, vault.seal(id, "account", token).unwrap());
        assert_eq!(vault.open(id, "account", &encrypted).unwrap(), token);
        assert!(vault.open(Uuid::new_v4(), "account", &encrypted).is_err());
        assert!(vault.open(id, "other-account", &encrypted).is_err());
        assert!(
            Vault::from_hex(&"b".repeat(64))
                .unwrap()
                .open(id, "account", &encrypted)
                .is_err()
        );
        assert!(vault.open(id, "account", "v1.YmFk").is_err());
        assert!(Vault::from_hex("not-a-key").is_err());
    }
}
