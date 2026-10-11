use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub const LABEL_PHONE: &str = "caw-pair-phone";
pub const LABEL_DESKTOP: &str = "caw-pair-desktop";

/// 16 random Crockford base32 characters (80 bits), already normalized.
pub fn new_code() -> String {
    let mut bytes = [0u8; 10];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut out = String::with_capacity(16);
    let (mut buf, mut bits) = (0u32, 0u32);
    for b in bytes {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buf >> bits) & 31) as usize] as char);
        }
        buf &= (1 << bits) - 1;
    }
    out
}

/// Uppercase, drop separators, and map look-alike letters to digits.
pub fn normalize(code: &str) -> String {
    code.chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect()
}

/// `XXXX-XXXX-XXXX-XXXX` for display.
pub fn display(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-")
}

fn mac(code: &str, label: &str, desktop_fp: &[u8; 32], phone_fp: &[u8; 32]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(normalize(code).as_bytes()).expect("HMAC takes any key length");
    mac.update(label.as_bytes());
    mac.update(desktop_fp);
    mac.update(phone_fp);
    mac
}

pub fn proof(code: &str, label: &str, desktop_fp: &[u8; 32], phone_fp: &[u8; 32]) -> Vec<u8> {
    mac(code, label, desktop_fp, phone_fp).finalize().into_bytes().to_vec()
}

/// Constant-time check of a proof.
pub fn verify(code: &str, label: &str, desktop_fp: &[u8; 32], phone_fp: &[u8; 32], proof: &[u8]) -> bool {
    mac(code, label, desktop_fp, phone_fp).verify_slice(proof).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_shape() {
        let code = new_code();
        assert_eq!(code.len(), 16);
        assert!(code.bytes().all(|b| ALPHABET.contains(&b)));
        assert_eq!(normalize(&display(&code)), code);
    }

    #[test]
    fn normalize_lookalikes() {
        assert_eq!(normalize("abcd-oil0 ijkl"), "ABCD01101JK1");
    }

    #[test]
    fn proofs_bind_everything() {
        let (d, p) = ([1u8; 32], [2u8; 32]);
        let good = proof("ABCD-EFGH-JKMN-PQRS", LABEL_PHONE, &d, &p);
        assert!(verify("abcdefghjkmnpqrs", LABEL_PHONE, &d, &p, &good));
        assert!(!verify("ABCDEFGHJKMNPQRT", LABEL_PHONE, &d, &p, &good));
        assert!(!verify("ABCDEFGHJKMNPQRS", LABEL_DESKTOP, &d, &p, &good));
        assert!(!verify("ABCDEFGHJKMNPQRS", LABEL_PHONE, &p, &d, &good));
    }
}
