use std::{fs, io, path::PathBuf};

use bip39::{Language, Mnemonic};
use clap::Parser;
use global_config::data_dir::DataDir;
use hkdf::Hkdf;
use openssl::{bn::BigNum, error::ErrorStack, pkey::Private, rsa::Rsa};
use operator_key::{
    ConversionError,
    encrypted::{EncryptedKey, EncryptionError},
    public, unencrypted,
};
use rand::{RngCore, rng};
use sha2::Sha256;
use thiserror::Error;
use tracing::{error, info};
use zeroize::Zeroizing;

#[derive(Error, Debug)]
pub enum KeygenError {
    #[error("Failed to generate new private key: {0}")]
    Generate(#[from] ErrorStack),

    #[error("Failed to convert key to PEM: {0}")]
    Conversion(#[from] ConversionError),

    #[error("Failed to read password: {0}")]
    Password(#[source] io::Error),

    #[error("Failed to write key: {0}")]
    KeyOutput(#[source] io::Error),

    #[error("Failed to encrypt the key: {0}")]
    EncryptionError(#[from] EncryptionError),

    #[error("Failed to convert output data to JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Key file(s) already exist in {0}")]
    Exists(String),

    #[error("Invalid mnemonic phrase: {0}")]
    InvalidMnemonic(String),

    #[error("Failed to derive deterministic key: {0}")]
    DeterministicKeyDerivation(String),

    #[error("Mnemonic file not found or unreadable: {0}")]
    MnemonicFile(#[source] io::Error),
}

#[derive(Parser, Clone, Debug)]
#[clap(
    name = "keygen",
    about = "RSA key generation tool. Outputs key to data directory."
)]
pub struct Keygen {
    #[clap(
        long,
        help = "Force file overwrite",
        value_name = "FORCE",
        default_value = "false"
    )]
    pub force: bool,

    #[clap(
        long,
        help = "Enable password encryption. Password is read from terminal or via --password-file"
    )]
    pub encrypt: bool,

    #[clap(
        long,
        help = "Path to a file containing the password to use",
        requires = "encrypt"
    )]
    pub password_file: Option<PathBuf>,

    #[clap(
        long,
        help = "Generate deterministic key from BIP39 mnemonic seed. Mnemonic will be generated if not provided."
    )]
    pub deterministic: bool,

    #[clap(
        long,
        help = "BIP39 mnemonic phrase for deterministic key generation (12 or 24 words)",
        requires = "deterministic"
    )]
    pub mnemonic: Option<String>,

    #[clap(
        long,
        help = "Path to file containing BIP39 mnemonic phrase",
        requires = "deterministic"
    )]
    pub mnemonic_file: Option<PathBuf>,

    #[clap(
        long,
        help = "Derivation path index for deterministic key generation",
        requires = "deterministic",
        default_value = "0"
    )]
    pub index: u32,
}

// Generate a deterministic RSA key from a mnemonic seed
fn generate_deterministic_rsa_key(
    mnemonic: &Mnemonic,
    index: u32,
) -> Result<Rsa<Private>, KeygenError> {
    // Convert mnemonic to seed using BIP39 derivation
    let seed = mnemonic.to_seed("");
    let seed_bytes = &seed;

    // Create info string for HKDF using the derivation index
    let info = format!("anchor-rsa-key-{}", index);

    // Use HKDF to derive key material for RSA parameters
    let hkdf = Hkdf::<Sha256>::new(None, seed_bytes);

    // Derive 512 bytes for maximum entropy - we'll use all of it for secure key generation
    let mut key_material = [0u8; 512];
    hkdf.expand(info.as_bytes(), &mut key_material)
        .map_err(|e| {
            KeygenError::DeterministicKeyDerivation(format!("HKDF expansion failed: {}", e))
        })?;

    // Use the derived key material to deterministically generate RSA parameters
    generate_rsa_from_seed(&key_material)
}

// Generate RSA key from deterministic seed material
fn generate_rsa_from_seed(seed: &[u8]) -> Result<Rsa<Private>, KeygenError> {
    // Use maximum entropy by creating separate RNG instances for p and q generation
    // This ensures complete independence and uses all 512 bytes of derived entropy
    use rand::{SeedableRng, rngs::StdRng};

    if seed.len() != 512 {
        return Err(KeygenError::DeterministicKeyDerivation(format!(
            "Expected 512 bytes of seed material, got {}",
            seed.len()
        )));
    }

    // Split the 512 bytes into separate entropy sources for maximum security:
    // - First 32 bytes: RNG for p prime generation
    // - Next 32 bytes: RNG for q prime generation
    // - Next 128 bytes: Direct entropy for p starting point
    // - Next 128 bytes: Direct entropy for q starting point
    // - Remaining 192 bytes: Additional entropy for other operations

    let p_rng_seed: [u8; 32] = seed[0..32].try_into().unwrap();
    let q_rng_seed: [u8; 32] = seed[32..64].try_into().unwrap();
    let p_direct_entropy = &seed[64..192]; // 128 bytes for p
    let q_direct_entropy = &seed[192..320]; // 128 bytes for q
    let extra_entropy = &seed[320..512]; // 192 bytes for additional operations

    let mut p_rng = StdRng::from_seed(p_rng_seed);
    let mut q_rng = StdRng::from_seed(q_rng_seed);

    // Generate deterministic but cryptographically secure primes using dedicated entropy
    let mut p_bytes = [0u8; 128]; // 1024 bits
    let mut q_bytes = [0u8; 128]; // 1024 bits

    // Use direct entropy for the base, then add RNG randomness
    p_bytes.copy_from_slice(p_direct_entropy);
    q_bytes.copy_from_slice(q_direct_entropy);

    // Mix in additional randomness from dedicated RNGs
    for i in 0..128 {
        p_bytes[i] ^= p_rng.next_u32() as u8;
        q_bytes[i] ^= q_rng.next_u32() as u8;
    }

    // Use extra entropy to further randomize the prime candidates for maximum security
    // XOR the first 128 bytes of extra entropy into p_bytes
    for i in 0..128 {
        p_bytes[i] ^= extra_entropy[i];
    }
    // XOR the remaining 64 bytes of extra entropy into q_bytes (cycling through)
    for i in 0..64 {
        q_bytes[i] ^= extra_entropy[128 + i];
        q_bytes[i + 64] ^= extra_entropy[128 + i]; // Use each byte twice for full coverage
    }

    // Set the high bit to ensure we get numbers of the right size
    p_bytes[0] |= 0x80;
    q_bytes[0] |= 0x80;

    // Set the low bit to ensure odd numbers (required for primes)
    p_bytes[127] |= 0x01;
    q_bytes[127] |= 0x01;

    let mut p = BigNum::from_slice(&p_bytes)?;
    let mut q = BigNum::from_slice(&q_bytes)?;

    // Find next prime from our deterministic starting points
    // This maintains determinism while ensuring cryptographic security
    let mut ctx = openssl::bn::BigNumContext::new()?;

    // Find the next prime after our deterministic starting point
    loop {
        if p.is_prime(64, &mut ctx)? {
            break;
        }
        p.add_word(2)?; // Only check odd numbers
    }

    loop {
        if q.is_prime(64, &mut ctx)? && p != q {
            break;
        }
        q.add_word(2)?; // Only check odd numbers
    }

    // Calculate n = p * q
    let mut n = BigNum::new()?;
    n.checked_mul(&p, &q, &mut ctx)?;

    // Calculate φ(n) = (p-1)(q-1)
    let mut p_minus_1 = BigNum::new()?;
    let mut q_minus_1 = BigNum::new()?;
    let mut phi = BigNum::new()?;
    let one = BigNum::from_u32(1)?;

    p_minus_1.checked_sub(&p, &one)?;
    q_minus_1.checked_sub(&q, &one)?;
    phi.checked_mul(&p_minus_1, &q_minus_1, &mut ctx)?;

    // Choose e = 65537 (standard)
    let e = BigNum::from_u32(65537)?;

    // Calculate d = e^(-1) mod φ(n)
    let mut d = BigNum::new()?;
    d.mod_inverse(&e, &phi, &mut ctx)?;

    // Calculate additional CRT parameters
    let mut dmp1 = BigNum::new()?;
    let mut dmq1 = BigNum::new()?;
    let mut iqmp = BigNum::new()?;

    dmp1.mod_inverse(&e, &p_minus_1, &mut ctx)?;
    dmq1.mod_inverse(&e, &q_minus_1, &mut ctx)?;
    iqmp.mod_inverse(&q, &p, &mut ctx)?;

    // Build the RSA key with all components
    Rsa::from_private_components(n, e, d, p, q, dmp1, dmq1, iqmp).map_err(KeygenError::Generate)
}

// Get or generate mnemonic based on keygen options
fn get_mnemonic(keygen: &Keygen) -> Result<(Mnemonic, bool), KeygenError> {
    let (mnemonic, was_generated) = if let Some(ref mnemonic_str) = keygen.mnemonic {
        // Use provided mnemonic string
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, mnemonic_str)
            .map_err(|e| KeygenError::InvalidMnemonic(e.to_string()))?;
        (mnemonic, false)
    } else if let Some(ref mnemonic_file) = keygen.mnemonic_file {
        // Read mnemonic from file
        let mnemonic_str = fs::read_to_string(mnemonic_file).map_err(KeygenError::MnemonicFile)?;
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, mnemonic_str.trim())
            .map_err(|e| KeygenError::InvalidMnemonic(e.to_string()))?;
        (mnemonic, false)
    } else {
        // Generate new mnemonic - 24 words requires 32 bytes of entropy
        let mut entropy = [0u8; 32];
        let mut rng = rng();
        rng.fill_bytes(&mut entropy);
        let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy)
            .map_err(|e| KeygenError::InvalidMnemonic(e.to_string()))?;
        (mnemonic, true)
    };

    Ok((mnemonic, was_generated))
}

// Run RSA keygeneration
pub fn run_keygen(keygen: Keygen, data_dir: &DataDir) -> Result<Rsa<Private>, KeygenError> {
    // Generate the new rsa private key
    let private_key = if keygen.deterministic {
        let (mnemonic, was_generated) = get_mnemonic(&keygen)?;

        if was_generated {
            info!(
                "Generated new mnemonic phrase: {}",
                mnemonic.words().collect::<Vec<_>>().join(" ")
            );
            info!("IMPORTANT: Save this mnemonic phrase in a secure location!");
            info!("You will need it to regenerate the same key deterministically.");
        }

        generate_deterministic_rsa_key(&mnemonic, keygen.index)?
    } else {
        Rsa::generate(2048)?
    };

    let public_key = public::to_base64(&private_key)?;

    // Create output paths for both files
    let private_key_file = if keygen.encrypt {
        data_dir.encrypted_private_key_file()
    } else {
        data_dir.unencrypted_private_key_file()
    };
    let pubkey_file = data_dir.public_key_file();

    if !keygen.force && private_key_file.exists() {
        return Err(KeygenError::Exists(private_key_file.display().to_string()));
    }

    if !keygen.force && pubkey_file.exists() {
        return Err(KeygenError::Exists(pubkey_file.display().to_string()));
    }

    // If the user would like to password encrypt the key
    if keygen.encrypt {
        let password = if let Some(password_file) = keygen.password_file {
            // Zeroize the original allocation
            let full =
                Zeroizing::new(fs::read_to_string(password_file).map_err(KeygenError::Password)?);
            // Zeroize the allocation with the trimmed string
            Zeroizing::new(full.trim().to_string())
        } else {
            read_password_from_user(true)?
        };

        // Encrypt the private key
        let encrypted_private = EncryptedKey::encrypt(&private_key, &password)?;

        fs::write(&private_key_file, &String::try_from(encrypted_private)?)
            .map_err(KeygenError::KeyOutput)?;
        info!(
            "Encrypted private key written to: {}",
            private_key_file.display()
        );
    } else {
        info!("Password not supplied. Private key will NOT be encrypted");

        fs::write(&private_key_file, &unencrypted::to_base64(&private_key)?)
            .map_err(KeygenError::KeyOutput)?;
        info!("Private key written to: {}", private_key_file.display());
    }

    // Log the public key
    info!("Generated public key: {public_key}");
    fs::write(&pubkey_file, &public_key).map_err(KeygenError::KeyOutput)?;

    Ok(private_key)
}

pub fn read_password_from_user(confirm: bool) -> Result<Zeroizing<String>, KeygenError> {
    loop {
        // Prompt for password
        let password = Zeroizing::new(
            rpassword::prompt_password("Enter password for keyfile: ")
                .map_err(KeygenError::Password)?,
        );

        if !confirm {
            return Ok(password);
        }

        // Confirm password
        let confirmation = Zeroizing::new(
            rpassword::prompt_password("Re-enter password to confirm: ")
                .map_err(KeygenError::Password)?,
        );

        // Verify passwords match
        if password == confirmation {
            return Ok(password);
        }
        error!("Passwords do not match. Please try again.");
    }
}

#[cfg(test)]
mod tests {
    use bip39::{Language, Mnemonic};

    use super::*;

    #[test]
    fn test_deterministic_key_generation() {
        let mnemonic_str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, mnemonic_str).unwrap();

        // Generate the same key twice with the same mnemonic and index
        let key1 = generate_deterministic_rsa_key(&mnemonic, 0).unwrap();
        let key2 = generate_deterministic_rsa_key(&mnemonic, 0).unwrap();

        // Keys should be identical
        let key1_pem = unencrypted::to_base64(&key1).unwrap();
        let key2_pem = unencrypted::to_base64(&key2).unwrap();
        assert_eq!(key1_pem, key2_pem);
    }

    #[test]
    fn test_different_indices_generate_different_keys() {
        let mnemonic_str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, mnemonic_str).unwrap();

        // Generate keys with different indices
        let key1 = generate_deterministic_rsa_key(&mnemonic, 0).unwrap();
        let key2 = generate_deterministic_rsa_key(&mnemonic, 1).unwrap();

        // Keys should be different
        let key1_pem = unencrypted::to_base64(&key1).unwrap();
        let key2_pem = unencrypted::to_base64(&key2).unwrap();
        assert_ne!(key1_pem, key2_pem);
    }

    #[test]
    fn test_different_mnemonics_generate_different_keys() {
        let mnemonic1_str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mnemonic2_str =
            "legal winner thank year wave sausage worth useful legal winner thank yellow";

        let mnemonic1 = Mnemonic::parse_in_normalized(Language::English, mnemonic1_str).unwrap();
        let mnemonic2 = Mnemonic::parse_in_normalized(Language::English, mnemonic2_str).unwrap();

        // Generate keys with same index but different mnemonics
        let key1 = generate_deterministic_rsa_key(&mnemonic1, 0).unwrap();
        let key2 = generate_deterministic_rsa_key(&mnemonic2, 0).unwrap();

        // Keys should be different
        let key1_pem = unencrypted::to_base64(&key1).unwrap();
        let key2_pem = unencrypted::to_base64(&key2).unwrap();
        assert_ne!(key1_pem, key2_pem);
    }

    #[test]
    fn test_generated_keys_are_valid() {
        let mnemonic_str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, mnemonic_str).unwrap();

        let key = generate_deterministic_rsa_key(&mnemonic, 0).unwrap();

        // Test that we can use the key for basic operations
        assert!(key.check_key().is_ok());
        assert_eq!(key.size(), 256); // 2048 bits / 8 = 256 bytes

        // Test that we can convert to PEM format
        let pem = unencrypted::to_base64(&key).unwrap();
        assert!(!pem.is_empty());

        // Test that we can derive public key
        let public_pem = public::to_base64(&key).unwrap();
        assert!(!public_pem.is_empty());
    }

    #[test]
    fn test_mnemonic_validation() {
        // Test valid mnemonic
        let valid_mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        assert!(Mnemonic::parse_in_normalized(Language::English, valid_mnemonic).is_ok());

        // Test invalid mnemonic (wrong word count)
        let invalid_mnemonic = "abandon abandon abandon";
        assert!(Mnemonic::parse_in_normalized(Language::English, invalid_mnemonic).is_err());

        // Test invalid mnemonic (invalid word)
        let invalid_mnemonic2 = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon invalid";
        assert!(Mnemonic::parse_in_normalized(Language::English, invalid_mnemonic2).is_err());
    }
}
