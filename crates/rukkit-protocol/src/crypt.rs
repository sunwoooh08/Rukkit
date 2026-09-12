//! Stream encryption for an authenticated connection.
//!
//! Minecraft encrypts the post-login byte stream with AES-128 in CFB-8 mode,
//! keyed and IV'd by the same 16-byte shared secret. CFB-8 shifts one byte at a
//! time, so it costs a full AES block operation *per byte* — there is no way
//! around that, it is what the protocol specifies. What it does mean is that
//! the AES implementation matters enormously, and the `aes` crate dispatches to
//! AES-NI / ARMv8 crypto instructions at runtime where available.

use cfb8::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};

/// Length of the shared secret, which doubles as the IV.
pub const SHARED_SECRET_LEN: usize = 16;

type Aes128Cfb8Enc = cfb8::Encryptor<aes::Aes128>;
type Aes128Cfb8Dec = cfb8::Decryptor<aes::Aes128>;

/// Error returned when a shared secret is not exactly 16 bytes.
#[derive(Debug, thiserror::Error)]
#[error("shared secret must be {SHARED_SECRET_LEN} bytes, got {0}")]
pub struct BadSecretLength(pub usize);

/// Outbound cipher. Stateful: each byte depends on all bytes before it, so one
/// instance must be kept for the life of the connection.
pub struct Encryptor {
    inner: Aes128Cfb8Enc,
}

impl std::fmt::Debug for Encryptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print cipher state: it is key-equivalent material.
        f.write_str("Encryptor(..)")
    }
}

impl Encryptor {
    pub fn new(secret: &[u8]) -> Result<Self, BadSecretLength> {
        if secret.len() != SHARED_SECRET_LEN {
            return Err(BadSecretLength(secret.len()));
        }
        Ok(Self {
            inner: Aes128Cfb8Enc::new_from_slices(secret, secret).expect("length already checked"),
        })
    }

    /// Encrypts in place, advancing the keystream.
    pub fn encrypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            self.inner.encrypt_block_mut(
                cfb8::cipher::generic_array::GenericArray::from_mut_slice(std::slice::from_mut(
                    byte,
                )),
            );
        }
    }
}

/// Inbound cipher, the mirror of [`Encryptor`].
pub struct Decryptor {
    inner: Aes128Cfb8Dec,
}

impl std::fmt::Debug for Decryptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Decryptor(..)")
    }
}

impl Decryptor {
    pub fn new(secret: &[u8]) -> Result<Self, BadSecretLength> {
        if secret.len() != SHARED_SECRET_LEN {
            return Err(BadSecretLength(secret.len()));
        }
        Ok(Self {
            inner: Aes128Cfb8Dec::new_from_slices(secret, secret).expect("length already checked"),
        })
    }

    /// Decrypts in place, advancing the keystream.
    pub fn decrypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            self.inner.decrypt_block_mut(
                cfb8::cipher::generic_array::GenericArray::from_mut_slice(std::slice::from_mut(
                    byte,
                )),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8; 16] = b"0123456789abcdef";

    #[test]
    fn round_trips_a_single_buffer() {
        let mut enc = Encryptor::new(SECRET).unwrap();
        let mut dec = Decryptor::new(SECRET).unwrap();

        let plain = b"a handshake, then some packet bytes".to_vec();
        let mut data = plain.clone();
        enc.encrypt(&mut data);
        assert_ne!(data, plain, "ciphertext must differ from plaintext");
        dec.decrypt(&mut data);
        assert_eq!(data, plain);
    }

    #[test]
    fn keystream_is_continuous_across_calls() {
        // The whole point of CFB-8 state: encrypting in two chunks must equal
        // encrypting in one. A codec that rebuilt the cipher per packet would
        // pass a single-buffer test and then corrupt the second packet.
        let mut one = Encryptor::new(SECRET).unwrap();
        let mut whole = b"first-half|second-half".to_vec();
        one.encrypt(&mut whole);

        let mut two = Encryptor::new(SECRET).unwrap();
        let mut split = b"first-half|second-half".to_vec();
        let (a, b) = split.split_at_mut(10);
        two.encrypt(a);
        two.encrypt(b);

        assert_eq!(whole, split);
    }

    #[test]
    fn identical_plaintext_blocks_encrypt_differently() {
        // CFB is a stream mode; repeated input must not produce repeated output.
        let mut enc = Encryptor::new(SECRET).unwrap();
        let mut data = vec![0u8; 64];
        enc.encrypt(&mut data);
        assert_ne!(&data[..32], &data[32..]);
    }

    #[test]
    fn wrong_secret_length_is_rejected() {
        assert!(Encryptor::new(b"short").is_err());
        assert!(Decryptor::new(&[0u8; 17]).is_err());
        assert!(Encryptor::new(&[0u8; 16]).is_ok());
    }

    #[test]
    fn empty_input_is_a_no_op() {
        let mut enc = Encryptor::new(SECRET).unwrap();
        enc.encrypt(&mut []);
    }

    #[test]
    fn debug_does_not_leak_cipher_state() {
        let enc = Encryptor::new(SECRET).unwrap();
        let rendered = format!("{enc:?}");
        assert_eq!(rendered, "Encryptor(..)");
        assert!(!rendered.contains("0123"));
    }
}
