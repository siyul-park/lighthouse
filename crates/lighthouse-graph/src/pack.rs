//! What `graph.db` stores of a value or a list of dependencies: the bytes as
//! they are when they are short or do not shrink, else compressed. The first byte
//! says which. A value is hashed before it is packed, so the hash is the same
//! whichever way it is stored.

use crate::Error;

/// Below this many bytes a value is stored as it is.
const MIN_PACKED: usize = 128;

const RAW: u8 = 0;
const PACKED: u8 = 1;

/// The bytes to store for `bytes`.
pub(crate) fn pack(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() >= MIN_PACKED {
        let mut packed = vec![PACKED];
        packed.extend(lz4_flex::block::compress_prepend_size(bytes));
        if packed.len() < bytes.len() {
            return packed;
        }
    }
    let mut raw = Vec::with_capacity(bytes.len() + 1);
    raw.push(RAW);
    raw.extend_from_slice(bytes);
    raw
}

/// The bytes that were packed into `stored`.
pub(crate) fn unpack(stored: &[u8]) -> Result<Vec<u8>, Error> {
    match stored.split_first() {
        Some((&RAW, bytes)) => Ok(bytes.to_vec()),
        Some((&PACKED, packed)) => lz4_flex::block::decompress_size_prepended(packed)
            .map_err(|e| Error::Encode(e.to_string())),
        _ => Err(Error::Encode("a stored value is not packed".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_packed_is_unpacked_whether_it_shrank_or_not() {
        let text = "a stored value ".repeat(100).into_bytes();
        let packed = pack(&text);
        assert!(packed.len() < text.len() / 4);
        assert_eq!(unpack(&packed).unwrap(), text);
        let short = b"short".to_vec();
        assert_eq!(unpack(&pack(&short)).unwrap(), short);
        let noise: Vec<u8> = (0..400u32)
            .map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        assert_eq!(unpack(&pack(&noise)).unwrap(), noise);
        assert!(unpack(&[]).is_err());
    }
}
