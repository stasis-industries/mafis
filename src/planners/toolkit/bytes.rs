//! Byte encodings for planner snapshots.

/// Little-endian bytes of each value, in order.
pub fn f32s_to_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Inverse of `f32s_to_bytes`; trailing bytes short of a full value are ignored.
pub fn f32s_from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(size_of::<f32>())
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_bytes_round_trip() {
        let values = [0.0, -0.0, 1.5, f32::MAX, 1e-30];
        let back = f32s_from_bytes(&f32s_to_bytes(&values));
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&back), bits(&values));
    }

    #[test]
    fn f32_bytes_are_little_endian() {
        // 1.5 is 0x3FC0_0000.
        assert_eq!(f32s_to_bytes(&[1.5]), vec![0x00, 0x00, 0xC0, 0x3F]);
    }
}
