//! A tiny stable string hash, so nothing here needs a hashing crate.

/// FNV-1a 64-bit, rendered as 16 hex chars. Used wherever picky needs a short
/// stable fingerprint of a string: a cache filename keyed by remote URL, or a
/// recorded working-tree state. Not cryptographic; both uses only ever compare
/// a value against one picky itself wrote.
pub fn fnv1a(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::fnv1a;

    #[test]
    fn fnv_is_stable_and_hex() {
        assert_eq!(fnv1a("https://example.com/x.git").len(), 16);
        assert_eq!(fnv1a("a"), fnv1a("a"));
        assert_ne!(fnv1a("a"), fnv1a("b"));
    }
}
