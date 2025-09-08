use bip39::{Language, Mnemonic};
use hkdf::Hkdf;
use openssl::{rsa::Rsa, pkey::Private, bn::BigNum};
use rand::{SeedableRng, RngCore, rngs::StdRng};
use sha2::Sha256;

fn generate_deterministic_rsa_key(mnemonic: &Mnemonic, index: u32) -> Result<Rsa<Private>, Box<dyn std::error::Error>> {
    // Convert mnemonic to seed using BIP39 derivation
    let seed = mnemonic.to_seed("");
    let seed_bytes = &seed;

    // Create info string for HKDF using the derivation index
    let info = format!("anchor-rsa-key-{}", index);
    
    // Use HKDF to derive key material for RSA parameters
    let hkdf = Hkdf::<Sha256>::new(None, seed_bytes);
    
    // We need to derive enough random bytes for RSA key generation
    let mut key_material = [0u8; 512];
    hkdf.expand(info.as_bytes(), &mut key_material)
        .map_err(|e| format!("HKDF expansion failed: {}", e))?;
    
    // Use the derived key material to deterministically generate RSA parameters
    generate_rsa_from_seed(&key_material)
}

fn generate_rsa_from_seed(seed: &[u8]) -> Result<Rsa<Private>, Box<dyn std::error::Error>> {
    // Create a deterministic RNG from the seed
    let mut rng = StdRng::from_seed({
        let mut seed_array = [0u8; 32];
        seed_array.copy_from_slice(&seed[..32]);
        seed_array
    });
    
    // Generate deterministic but cryptographically secure primes
    let mut p_bytes = [0u8; 128]; // 1024 bits
    let mut q_bytes = [0u8; 128]; // 1024 bits
    
    rng.fill_bytes(&mut p_bytes);
    rng.fill_bytes(&mut q_bytes);
    
    // Set the high bit to ensure we get numbers of the right size
    p_bytes[0] |= 0x80;
    q_bytes[0] |= 0x80;
    
    // Set the low bit to ensure odd numbers (required for primes)
    p_bytes[127] |= 0x01;
    q_bytes[127] |= 0x01;
    
    let mut p = BigNum::from_slice(&p_bytes)?;
    let mut q = BigNum::from_slice(&q_bytes)?;
    
    // Find next prime from our deterministic starting points
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
    Ok(Rsa::from_private_components(n, e, d, p, q, dmp1, dmq1, iqmp)?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing deterministic RSA key generation...");
    
    // Test with a standard BIP39 test mnemonic
    let mnemonic_str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, mnemonic_str)?;
    
    println!("Mnemonic: {}", mnemonic_str);
    
    // Generate the same key twice
    let key1 = generate_deterministic_rsa_key(&mnemonic, 0)?;
    let key2 = generate_deterministic_rsa_key(&mnemonic, 0)?;
    
    // Convert to PEM for comparison
    let pem1 = key1.private_key_to_pem()?;
    let pem2 = key2.private_key_to_pem()?;
    
    println!("Key 1 size: {} bits", key1.size() * 8);
    println!("Key 2 size: {} bits", key2.size() * 8);
    println!("Keys are identical: {}", pem1 == pem2);
    
    // Test different indices produce different keys
    let key3 = generate_deterministic_rsa_key(&mnemonic, 1)?;
    let pem3 = key3.private_key_to_pem()?;
    println!("Key with index 0 vs 1 are different: {}", pem1 != pem3);
    
    // Verify key validity
    println!("Key 1 is valid: {}", key1.check_key().is_ok());
    println!("Key 2 is valid: {}", key2.check_key().is_ok());
    println!("Key 3 is valid: {}", key3.check_key().is_ok());
    
    Ok(())
}