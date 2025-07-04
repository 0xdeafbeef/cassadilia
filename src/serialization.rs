use std::collections::BTreeMap;

use thiserror::Error;

use crate::types::{BlobHash, HASH_SIZE, WalOpRaw};

#[derive(Error, Debug)]
pub enum SerializationError {
    #[error("unexpected end of data while parsing {parsing_context}")]
    UnexpectedEof { parsing_context: &'static str },

    #[error(
        "insufficient data for {entity}: expected {expected} bytes, found {found} while parsing {parsing_context}"
    )]
    InsufficientData {
        entity: &'static str,
        expected: usize,
        found: usize,
        parsing_context: &'static str,
    },

    #[error("invalid variant tag {tag} for {enum_name} while parsing {parsing_context}")]
    InvalidVariantTag { tag: u8, enum_name: &'static str, parsing_context: &'static str },
}

/// serialize `BTreeMap`<Vec<u8>, `BlobHash`> using hand-rolled format
/// format: [u32 `num_entries`][[u32 `key_len`][key_bytes][`32_bytes_hash`]]...
pub(crate) fn serialize_btreemap_bytes_to_hash(
    map: &BTreeMap<Vec<u8>, BlobHash>,
) -> Result<Vec<u8>, SerializationError> {
    let mut result = Vec::new();

    // write number of entries
    let num_entries = map.len() as u32;
    result.extend_from_slice(&num_entries.to_le_bytes());

    // write each entry
    for (key, hash) in map {
        // write key length
        let key_len = key.len() as u32;
        result.extend_from_slice(&key_len.to_le_bytes());

        // write key bytes
        result.extend_from_slice(key);

        // write hash (always 32 bytes)
        result.extend_from_slice(hash.as_bytes());
    }

    Ok(result)
}

/// deserialize `BTreeMap`<Vec<u8>, `BlobHash`> using hand-rolled format
pub(crate) fn deserialize_btreemap_bytes_to_hash(
    bytes: &[u8],
) -> Result<BTreeMap<Vec<u8>, BlobHash>, SerializationError> {
    let mut bytes = bytes;

    // read number of entries
    let num_entries = read_u32(&mut bytes, "BTreeMap header")?;

    let mut map = BTreeMap::new();

    // read each entry
    for _ in 0..num_entries {
        // read key
        let key = read_bytes_with_len(&mut bytes, "BTreeMap key")?;

        // read hash
        let hash = read_fixed_bytes::<HASH_SIZE>(&mut bytes, "BTreeMap hash")?;
        let hash = BlobHash::from_bytes(hash);

        map.insert(key, hash);
    }

    Ok(map)
}

/// serialize `WalOpRaw` using hand-rolled format
/// format: [u8 `variant_tag`][variant_data]
///   Put: 0 + [u32 `key_len`][key_bytes][`32_bytes_hash`]
///   Remove: 1 + [`keys_data` bytes...]
pub(crate) fn serialize_wal_op_raw(op: &WalOpRaw) -> Result<Vec<u8>, SerializationError> {
    let mut result = Vec::new();

    match op {
        WalOpRaw::Put { key_bytes, hash } => {
            // variant tag
            result.push(0);

            // key length and bytes
            let key_len = key_bytes.len() as u32;
            result.extend_from_slice(&key_len.to_le_bytes());
            result.extend_from_slice(key_bytes);

            // hash
            result.extend_from_slice(hash.as_bytes());
        }
        WalOpRaw::Remove { keys_bytes } => {
            // variant tag
            result.push(1);

            // serialize the keys: [num_keys][len1][key1][len2][key2]...
            let num_keys = keys_bytes.len() as u32;
            result.extend_from_slice(&num_keys.to_le_bytes());

            for key_bytes in keys_bytes {
                let key_len = key_bytes.len() as u32;
                result.extend_from_slice(&key_len.to_le_bytes());
                result.extend_from_slice(key_bytes);
            }
        }
    }

    Ok(result)
}

/// deserialize `WalOpRaw` using hand-rolled format
pub(crate) fn deserialize_wal_op_raw(bytes: &[u8]) -> Result<WalOpRaw, SerializationError> {
    let mut bytes = bytes;

    let variant_tag = read_u8(&mut bytes, "WalOpRaw variant")?;

    match variant_tag {
        0 => {
            // Put variant
            let key_bytes = read_bytes_with_len(&mut bytes, "WalOpRaw Put key")?;
            let hash = read_fixed_bytes::<HASH_SIZE>(&mut bytes, "WalOpRaw Put hash")?;
            let hash = BlobHash::from_bytes(hash);
            Ok(WalOpRaw::Put { key_bytes, hash })
        }
        1 => {
            // Remove variant
            // deserialize the keys: [num_keys][len1][key1][len2][key2]...
            let num_keys = read_u32(&mut bytes, "Remove num_keys")? as usize;

            let mut keys_bytes = Vec::new();
            for _ in 0..num_keys {
                let key_bytes = read_bytes_with_len(&mut bytes, "Remove key")?;
                keys_bytes.push(key_bytes);
            }

            Ok(WalOpRaw::Remove { keys_bytes })
        }
        _ => Err(SerializationError::InvalidVariantTag {
            tag: variant_tag,
            enum_name: "WalOpRaw",
            parsing_context: "WalOpRaw deserialization",
        }),
    }
}

// helper functions for reading from byte slices
#[inline]
fn read_u8(bytes: &mut &[u8], parsing_context: &'static str) -> Result<u8, SerializationError> {
    if bytes.is_empty() {
        return Err(SerializationError::UnexpectedEof { parsing_context });
    }
    let value = bytes[0];
    *bytes = &bytes[1..];
    Ok(value)
}

#[inline]
fn read_u32(bytes: &mut &[u8], parsing_context: &'static str) -> Result<u32, SerializationError> {
    if bytes.len() < 4 {
        return Err(SerializationError::InsufficientData {
            entity: "u32",
            expected: 4,
            found: bytes.len(),
            parsing_context,
        });
    }
    let value = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    *bytes = &bytes[4..];
    Ok(value)
}

#[inline]
fn read_fixed_bytes<const N: usize>(
    bytes: &mut &[u8],
    parsing_context: &'static str,
) -> Result<[u8; N], SerializationError> {
    if bytes.len() < N {
        return Err(SerializationError::InsufficientData {
            entity: "fixed bytes",
            expected: N,
            found: bytes.len(),
            parsing_context,
        });
    }
    let array: [u8; N] = bytes[..N].try_into().unwrap();
    *bytes = &bytes[N..];
    Ok(array)
}

#[inline]
fn read_bytes_with_len(
    bytes: &mut &[u8],
    parsing_context: &'static str,
) -> Result<Vec<u8>, SerializationError> {
    let len = read_u32(bytes, parsing_context)? as usize;
    if bytes.len() < len {
        return Err(SerializationError::InsufficientData {
            entity: "variable-length bytes",
            expected: len,
            found: bytes.len(),
            parsing_context,
        });
    }
    let data = bytes[..len].to_vec();
    *bytes = &bytes[len..];
    Ok(data)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::types::{BlobHash, HASH_SIZE, WalOpRaw};

    fn make_test_hash(byte: u8) -> BlobHash {
        let mut bytes = [0u8; HASH_SIZE];
        bytes[0] = byte;
        BlobHash::from_bytes(bytes)
    }

    #[test]
    fn test_btreemap_serialization_empty() {
        let map: BTreeMap<Vec<u8>, BlobHash> = BTreeMap::new();
        let serialized = serialize_btreemap_bytes_to_hash(&map).unwrap();
        let deserialized = deserialize_btreemap_bytes_to_hash(&serialized).unwrap();
        assert_eq!(map, deserialized);
    }

    #[test]
    fn test_btreemap_serialization_single_empty_key() {
        let mut map = BTreeMap::new();
        map.insert(vec![], make_test_hash(1));
        let serialized = serialize_btreemap_bytes_to_hash(&map).unwrap();
        let deserialized = deserialize_btreemap_bytes_to_hash(&serialized).unwrap();
        assert_eq!(map, deserialized);
    }

    #[test]
    fn test_btreemap_serialization_multiple_keys() {
        let mut map = BTreeMap::new();
        map.insert(vec![], make_test_hash(1)); // empty key
        map.insert(vec![1, 2, 3], make_test_hash(2)); // small key
        map.insert(vec![255; 1000], make_test_hash(3)); // large key
        map.insert(vec![0], make_test_hash(4)); // single byte key

        let serialized = serialize_btreemap_bytes_to_hash(&map).unwrap();
        let deserialized = deserialize_btreemap_bytes_to_hash(&serialized).unwrap();
        assert_eq!(map, deserialized);
    }

    #[test]
    fn test_wal_op_put_empty_key() {
        let op = WalOpRaw::Put { key_bytes: vec![], hash: make_test_hash(42) };
        let serialized = serialize_wal_op_raw(&op).unwrap();
        let deserialized = deserialize_wal_op_raw(&serialized).unwrap();

        match deserialized {
            WalOpRaw::Put { key_bytes, hash } => {
                assert_eq!(key_bytes, vec![]);
                assert_eq!(hash, make_test_hash(42));
            }
            WalOpRaw::Remove { .. } => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_wal_op_put_large_key() {
        let large_key = vec![0xAB; 10000];
        let op = WalOpRaw::Put { key_bytes: large_key.clone(), hash: make_test_hash(99) };
        let serialized = serialize_wal_op_raw(&op).unwrap();
        let deserialized = deserialize_wal_op_raw(&serialized).unwrap();

        match deserialized {
            WalOpRaw::Put { key_bytes, hash } => {
                assert_eq!(key_bytes, large_key);
                assert_eq!(hash, make_test_hash(99));
            }
            WalOpRaw::Remove { .. } => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_wal_op_remove_empty() {
        // empty list of keys
        let op = WalOpRaw::Remove { keys_bytes: vec![] };
        let serialized = serialize_wal_op_raw(&op).unwrap();
        let deserialized = deserialize_wal_op_raw(&serialized).unwrap();

        match deserialized {
            WalOpRaw::Remove { keys_bytes } => {
                assert_eq!(keys_bytes.len(), 0);
            }
            WalOpRaw::Put { .. } => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_wal_op_remove_single_empty_key() {
        // single empty key
        let op = WalOpRaw::Remove { keys_bytes: vec![b"".to_vec()] };
        let serialized = serialize_wal_op_raw(&op).unwrap();
        let deserialized = deserialize_wal_op_raw(&serialized).unwrap();

        match deserialized {
            WalOpRaw::Remove { keys_bytes } => {
                assert_eq!(keys_bytes.len(), 1);
                assert_eq!(keys_bytes[0], b"");
            }
            WalOpRaw::Put { .. } => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_wal_op_remove_mixed_keys() {
        // multiple keys of different sizes including empty
        let large_key = vec![0xFF; 1000];
        let op = WalOpRaw::Remove {
            keys_bytes: vec![
                b"".to_vec(),      // empty key
                b"hello".to_vec(), // small key
                large_key.clone(), // large key
            ],
        };
        let serialized = serialize_wal_op_raw(&op).unwrap();
        let deserialized = deserialize_wal_op_raw(&serialized).unwrap();

        match deserialized {
            WalOpRaw::Remove { keys_bytes } => {
                assert_eq!(keys_bytes.len(), 3);
                assert_eq!(keys_bytes[0], b"");
                assert_eq!(keys_bytes[1], b"hello");
                assert_eq!(keys_bytes[2], large_key);
            }
            WalOpRaw::Put { .. } => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_btreemap_invalid_data() {
        // test with invalid data that's too short
        let invalid_data = vec![1, 2, 3]; // not enough bytes for even a header
        let result = deserialize_btreemap_bytes_to_hash(&invalid_data);
        assert!(result.is_err());

        // test with valid header but invalid entry
        let mut invalid_data = Vec::new();
        invalid_data.extend_from_slice(&1u32.to_le_bytes()); // 1 entry
        invalid_data.extend_from_slice(&4u32.to_le_bytes()); // key len = 4
        invalid_data.extend_from_slice(b"hi"); // but only 2 bytes provided
        let result = deserialize_btreemap_bytes_to_hash(&invalid_data);
        assert!(result.is_err());
    }

    #[test]
    fn test_wal_op_invalid_data() {
        // test with empty data
        let result = deserialize_wal_op_raw(&[]);
        assert!(result.is_err());

        // test with invalid variant tag
        let result = deserialize_wal_op_raw(&[99]); // invalid tag
        assert!(result.is_err());

        // test with valid tag but invalid data
        let mut invalid_data = vec![0]; // Put variant
        invalid_data.extend_from_slice(&4u32.to_le_bytes()); // key len = 4
        invalid_data.extend_from_slice(b"hi"); // but only 2 bytes
        let result = deserialize_wal_op_raw(&invalid_data);
        assert!(result.is_err());
    }

    #[test]
    fn test_helper_functions() {
        let data = [42, 1, 0, 0, 0, 5, 6, 7, 8, 9];
        let mut slice = &data[..];

        // test read_u8
        assert_eq!(read_u8(&mut slice, "test").unwrap(), 42);

        // test read_u32
        assert_eq!(read_u32(&mut slice, "test").unwrap(), 1);

        // test read_fixed_bytes
        let fixed: [u8; 3] = read_fixed_bytes(&mut slice, "test").unwrap();
        assert_eq!(fixed, [5, 6, 7]);

        // test read_bytes_with_len (remaining 2 bytes should be interpreted as len=2056 which
        // should fail)
        let result = read_bytes_with_len(&mut slice, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_helper_functions_errors() {
        // test read_u8 with empty data
        let mut empty = &[][..];
        assert!(read_u8(&mut empty, "test").is_err());

        // test read_u32 with insufficient data
        let mut short = &[1, 2][..];
        assert!(read_u32(&mut short, "test").is_err());

        // test read_fixed_bytes with insufficient data
        let mut short = &[1, 2][..];
        let result: Result<[u8; 5], _> = read_fixed_bytes(&mut short, "test");
        assert!(result.is_err());

        // test read_bytes_with_len with insufficient data
        let mut data = &[5u8, 0, 0, 0, 1, 2][..]; // len=5 but only 2 bytes follow
        assert!(read_bytes_with_len(&mut data, "test").is_err());
    }
}
