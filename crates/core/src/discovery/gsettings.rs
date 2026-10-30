//! GSettings schema probing (03_ENVIRONMENT_DISCOVERY.md §4).
//!
//! `gschemas.compiled` is a GVDB ("GVariant database") file. This module
//! implements the minimal **read-only enumeration** needed to report which
//! schemas and key names are installed: header validation, root hash table,
//! the empty-key root list item, and per-schema sub-tables. No hash lookup
//! is required, so the DJB hash and bucket traversal are deliberately absent.
//!
//! Format reference: GLib `gio/gvdb/gvdb-format.h` and `gvdb-reader.c`
//! (LGPL-2.1). Verified against a real `gschemas.compiled` during Phase 03;
//! see docs/architecture/discovery.md for the layout notes.
//!
//! Bounds checking is exhaustive: malformed or truncated input yields an
//! error, never a panic. Only little-endian files are supported (both
//! supported targets are LE; big-endian files fail header validation).

/// Maximum schema/key strings accepted per file — a bound, not a limit on
/// real systems (thousands of schemas parse fine).
const MAX_ENTRIES: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GvdbError {
    TooSmall,
    BadSignature,
    UnsupportedByteOrder,
    Truncated(&'static str),
}

/// Pointer into the file: `[start, end)` byte range.
#[derive(Debug, Clone, Copy)]
struct Pointer {
    start: usize,
    end: usize,
}

struct HashTable {
    /// Byte offset of the first hash item.
    items_off: usize,
    n_items: usize,
}

const ITEM_SIZE: usize = 24;
const PARENT_NONE: u32 = 0xffff_ffff;

fn u32_le(data: &[u8], off: usize, what: &'static str) -> Result<u32, GvdbError> {
    let end = off.checked_add(4).ok_or(GvdbError::Truncated(what))?;
    if end > data.len() {
        return Err(GvdbError::Truncated(what));
    }
    Ok(u32::from_le_bytes([
        data[off],
        data[off + 1],
        data[off + 2],
        data[off + 3],
    ]))
}

fn deref_pointer(
    data: &[u8],
    start: u32,
    end: u32,
    alignment: usize,
) -> Result<Pointer, GvdbError> {
    let (start, end) = (start as usize, end as usize);
    if start > end || end > data.len() || start % alignment != 0 {
        return Err(GvdbError::Truncated("pointer out of range"));
    }
    Ok(Pointer { start, end })
}

/// Parse a hash-table region (root or sub-table): 8-byte header (bloom words
/// count in the low 27 bits, bucket count), bloom words, bucket index, then
/// 24-byte items.
fn parse_hash_table(data: &[u8], pointer: Pointer) -> Result<HashTable, GvdbError> {
    if pointer.end - pointer.start < 8 {
        return Err(GvdbError::Truncated("hash table header"));
    }
    let n_bloom_raw = u32_le(data, pointer.start, "n_bloom_words")?;
    let n_bloom = (n_bloom_raw & ((1 << 27) - 1)) as usize;
    let n_buckets = u32_le(data, pointer.start + 4, "n_buckets")? as usize;
    let buckets_off = pointer
        .start
        .checked_add(8)
        .and_then(|off| off.checked_add(n_bloom * 4))
        .ok_or(GvdbError::Truncated("bloom words"))?;
    let items_off = buckets_off
        .checked_add(n_buckets * 4)
        .ok_or(GvdbError::Truncated("buckets"))?;
    if items_off > pointer.end || !(pointer.end - items_off).is_multiple_of(ITEM_SIZE) {
        return Err(GvdbError::Truncated("item table size"));
    }
    Ok(HashTable {
        items_off,
        n_items: (pointer.end - items_off) / ITEM_SIZE,
    })
}

/// One 24-byte hash item: hash u32, parent u32, key_start u32, key_size u16,
/// type u8, unused u8, value pointer 8 bytes.
struct Item<'a> {
    parent: u32,
    key: &'a [u8],
    item_type: u8,
    value: [u8; 8],
}

fn read_item<'a>(data: &'a [u8], table: &HashTable, index: usize) -> Result<Item<'a>, GvdbError> {
    let base = table
        .items_off
        .checked_add(index * ITEM_SIZE)
        .ok_or(GvdbError::Truncated("item offset"))?;
    if base + ITEM_SIZE > data.len() {
        return Err(GvdbError::Truncated("item"));
    }
    let parent = u32_le(data, base + 4, "item parent")?;
    let key_start = u32_le(data, base + 8, "item key_start")? as usize;
    let key_size = u16::from_le_bytes([data[base + 12], data[base + 13]]) as usize;
    let key_end = key_start
        .checked_add(key_size)
        .ok_or(GvdbError::Truncated("key end"))?;
    if key_end > data.len() {
        return Err(GvdbError::Truncated("key range"));
    }
    let mut value = [0u8; 8];
    value.copy_from_slice(&data[base + 16..base + 24]);
    Ok(Item {
        parent,
        key: &data[key_start..key_end],
        item_type: data[base + 14],
        value,
    })
}

fn parse_pointer(value: [u8; 8]) -> (u32, u32) {
    (
        u32::from_le_bytes([value[0], value[1], value[2], value[3]]),
        u32::from_le_bytes([value[4], value[5], value[6], value[7]]),
    )
}

/// The item indices listed by a type-`'L'` item — the canonical children of
/// a table (what `gvdb_table_list` returns).
fn list_children<'a>(
    data: &'a [u8],
    table: &HashTable,
    item: &Item<'a>,
) -> Result<Vec<usize>, GvdbError> {
    let (start, end) = parse_pointer(item.value);
    let pointer = deref_pointer(data, start, end, 4)?;
    let len = pointer.end - pointer.start;
    if len % 4 != 0 || len / 4 > MAX_ENTRIES {
        return Err(GvdbError::Truncated("list length"));
    }
    let mut indices = Vec::with_capacity(len / 4);
    for i in 0..(len / 4) {
        let index = u32_le(data, pointer.start + i * 4, "list entry")? as usize;
        if index >= table.n_items {
            return Err(GvdbError::Truncated("list entry out of range"));
        }
        indices.push(index);
    }
    Ok(indices)
}

/// Pure GVDB parser: enumerates the root table's children (schema ids) and,
/// for each one that is a hash table, its key names.
pub fn parse_compiled_schemas(data: &[u8]) -> Result<Vec<(String, Vec<String>)>, GvdbError> {
    if data.len() < 24 {
        return Err(GvdbError::TooSmall);
    }
    // "GVariant" magic as two little-endian u32s; version must be 0.
    let sig0 = u32_le(data, 0, "signature0")?;
    let sig1 = u32_le(data, 4, "signature1")?;
    if (sig0, sig1) != (1_918_981_703, 1_953_390_953) {
        return Err(GvdbError::BadSignature);
    }
    if u32_le(data, 8, "version")? != 0 {
        return Err(GvdbError::UnsupportedByteOrder);
    }
    let root = deref_pointer(
        data,
        u32_le(data, 16, "root start")?,
        u32_le(data, 20, "root end")?,
        4,
    )?;
    let root_table = parse_hash_table(data, root)?;

    // The root list item: empty key, no parent, type 'L'.
    let mut root_list: Option<Item> = None;
    for index in 0..root_table.n_items {
        let item = read_item(data, &root_table, index)?;
        if item.parent == PARENT_NONE && item.key.is_empty() && item.item_type == b'L' {
            root_list = Some(item);
            break;
        }
    }
    let Some(root_list) = root_list else {
        return Err(GvdbError::Truncated("root list item"));
    };

    let mut schemas = Vec::new();
    for index in list_children(data, &root_table, &root_list)? {
        let item = read_item(data, &root_table, index)?;
        if item.item_type != b'H' {
            continue;
        }
        let schema_name = String::from_utf8_lossy(item.key).into_owned();
        let mut keys = Vec::new();
        // Parse the schema's sub-table for key names; a schema whose table
        // cannot be parsed still shows up with an empty key list.
        let (start, end) = parse_pointer(item.value);
        if let Ok(pointer) = deref_pointer(data, start, end, 4)
            && let Ok(sub_table) = parse_hash_table(data, pointer)
        {
            for sub_index in 0..sub_table.n_items {
                let sub_item = read_item(data, &sub_table, sub_index)?;
                if sub_item.parent == PARENT_NONE
                    && sub_item.key.is_empty()
                    && sub_item.item_type == b'L'
                {
                    if let Ok(children) = list_children(data, &sub_table, &sub_item) {
                        for child in children {
                            let child_item = read_item(data, &sub_table, child)?;
                            if !child_item.key.is_empty() {
                                keys.push(String::from_utf8_lossy(child_item.key).into_owned());
                            }
                        }
                    }
                    break;
                }
            }
        }
        keys.sort();
        schemas.push((schema_name, keys));
        if schemas.len() > MAX_ENTRIES {
            return Err(GvdbError::Truncated("schema count"));
        }
    }
    schemas.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(schemas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage_and_truncation() {
        assert_eq!(parse_compiled_schemas(b""), Err(GvdbError::TooSmall));
        assert_eq!(
            parse_compiled_schemas(&[0u8; 24]),
            Err(GvdbError::BadSignature)
        );
        let mut header = vec![0u8; 24];
        header[0..4].copy_from_slice(&1_918_981_703_u32.to_le_bytes());
        header[4..8].copy_from_slice(&1_953_390_953_u32.to_le_bytes());
        // version != 0 → unsupported
        header[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            parse_compiled_schemas(&header),
            Err(GvdbError::UnsupportedByteOrder)
        );
        // valid magic, root pointing past the file → truncated
        header[8..12].copy_from_slice(&0u32.to_le_bytes());
        header[16..20].copy_from_slice(&24u32.to_le_bytes());
        header[20..24].copy_from_slice(&9999u32.to_le_bytes());
        assert!(matches!(
            parse_compiled_schemas(&header),
            Err(GvdbError::Truncated(_))
        ));
    }
}
