//! Bounded decoder for the **framed non-normal GVariant** values GVDB
//! stores (dconf user/system databases and `gschemas.compiled` schema
//! defaults).
//!
//! Layout, verified byte-for-byte against a real `~/.config/dconf/user`,
//! a `dconf compile` scratch database, and the system
//! `/usr/share/glib-2.0/schemas/gschemas.compiled`:
//!
//! ```text
//! [value body][NUL padding][type signature]
//! ```
//!
//! The signature never contains NUL, so splitting at the **last** NUL byte
//! separates body from signature regardless of how many pad/trailer bytes
//! the non-normal body carries. Bodies use GLib's *non-normal* forms —
//! strings are NUL-terminated without a length prefix, tuples align fields
//! naturally from the body start. Only the value forms our settings can
//! declare are decoded (scalars, strings, tuples, variant, maybe); arrays
//! and dictionaries yield an explicit unsupported-type error, never a
//! guess and never a panic.

#[derive(Debug, Clone, PartialEq)]
pub enum GVariantError {
    /// No NUL byte → no signature framing; not a GVDB value.
    Unframed,
    /// The trailing signature is not a complete valid signature.
    BadSignature,
    /// The body does not carry enough bytes for the declared type.
    Truncated,
    /// Body bytes are not valid UTF-8 for a string type.
    InvalidUtf8,
    /// The value type exists but is outside the decoded subset (arrays,
    /// dictionaries, handles). Reported, never guessed.
    UnsupportedType(&'static str),
}

/// A decoded value. Numbers keep their GVariant width so typed mismatches
/// against a descriptor can be reported precisely.
#[derive(Debug, Clone, PartialEq)]
pub enum GVariantValue {
    Bool(bool),
    Byte(u8),
    Int(i64),
    Uint(u64),
    Double(f64),
    Str(String),
    /// A maybe: `None` = nothing present.
    Maybe(Option<Box<GVariantValue>>),
    /// A variant wrapper: the inner signature and value.
    Variant(String, Box<GVariantValue>),
    /// A tuple: decoded fields (only the leading fields our callers need;
    /// decoding stops once the requested prefix is available).
    Tuple(Vec<GVariantValue>),
}

/// Alignment of one complete type (GLib rules: fixed-width types align to
/// their size capped at 8; strings/variants to 1).
fn alignment_of(sig: &[u8]) -> usize {
    match sig.first() {
        Some(b'b') | Some(b'y') | Some(b'g') | Some(b's') | Some(b'o') | Some(b'm')
        | Some(b'v') => 1,
        Some(b'n') | Some(b'q') => 2,
        Some(b'i') | Some(b'u') => 4,
        Some(b'x') | Some(b't') | Some(b'd') => 8,
        Some(b'(') | Some(b'{') | Some(b'a') => 8,
        _ => 1,
    }
}

/// Length in signature bytes of the complete type starting at `sig[i]`.
/// Returns `None` for an invalid signature.
fn complete_type_len(sig: &[u8], i: usize) -> Option<usize> {
    let c = *sig.get(i)?;
    match c {
        b'm' => Some(complete_type_len(sig, i + 1)?.checked_add(1)?),
        b'a' => Some(complete_type_len(sig, i + 1)?.checked_add(1)?),
        b'(' | b'{' => {
            let (open, close) = if c == b'(' {
                (b'(', b')')
            } else {
                (b'{', b'}')
            };
            let mut depth = 0usize;
            let mut j = i;
            loop {
                let ch = *sig.get(j)?;
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j - i + 1);
                    }
                } else if !sig_char_ok(ch) {
                    return None;
                }
                j += 1;
                if j - i > 256 {
                    return None; // bounded: no unbounded signature walk
                }
            }
        }
        _ if sig_char_ok(c) => Some(1),
        _ => None,
    }
}

fn sig_char_ok(c: u8) -> bool {
    matches!(
        c,
        b'y' | b'b'
            | b'n'
            | b'q'
            | b'i'
            | b'u'
            | b'x'
            | b't'
            | b'd'
            | b's'
            | b'o'
            | b'g'
            | b'v'
            | b'm'
            | b'a'
            | b'('
            | b')'
            | b'{'
            | b'}'
    )
}

/// Split a framed value at the last NUL: body, signature.
fn split_framed(raw: &[u8]) -> Result<(&[u8], &[u8]), GVariantError> {
    let Some(pos) = raw.iter().rposition(|b| *b == 0) else {
        return Err(GVariantError::Unframed);
    };
    let sig = &raw[pos + 1..];
    if sig.is_empty() {
        return Err(GVariantError::BadSignature);
    }
    Ok((&raw[..pos], sig))
}

struct Body<'a> {
    data: &'a [u8],
}

impl<'a> Body<'a> {
    fn fixed(&self, off: usize, n: usize) -> Result<&'a [u8], GVariantError> {
        self.data.get(off..off + n).ok_or(GVariantError::Truncated)
    }
    /// Read a non-normal string: bytes up to the terminating NUL.
    fn string(&self, off: usize) -> Result<(String, usize), GVariantError> {
        let rest = self.data.get(off..).ok_or(GVariantError::Truncated)?;
        let end = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or(GVariantError::Truncated)?;
        let text = std::str::from_utf8(&rest[..end]).map_err(|_| GVariantError::InvalidUtf8)?;
        Ok((text.to_string(), off + end + 1))
    }
    fn align(&self, off: usize, alignment: usize) -> usize {
        off.div_ceil(alignment) * alignment
    }
}

/// Decode one complete type from `body` at `off`. `max_fields` bounds tuple
/// decoding (callers that only need the first field pass a small prefix).
fn decode_at(
    body: &Body<'_>,
    sig: &[u8],
    off: usize,
    out: &mut Vec<GVariantValue>,
) -> Result<usize, GVariantError> {
    let Some(len) = complete_type_len(sig, 0) else {
        return Err(GVariantError::BadSignature);
    };
    if len != sig.len() {
        // Trailing signature characters: only valid when the caller asked
        // for a prefix decode (handled by the tuple arm); standalone values
        // must consume everything.
        return Err(GVariantError::BadSignature);
    }
    decode_field(body, sig, off, out)
}

/// Decode the complete type at `sig[0..n]`, append to `out`, return the new
/// offset. Empty tuples decode to an empty field list marker.
fn decode_field(
    body: &Body<'_>,
    sig: &[u8],
    off: usize,
    out: &mut Vec<GVariantValue>,
) -> Result<usize, GVariantError> {
    let aligned = body.align(off, alignment_of(sig));
    match sig[0] {
        b'b' => {
            let bytes = body.fixed(aligned, 1)?;
            out.push(GVariantValue::Bool(bytes[0] != 0));
            Ok(aligned + 1)
        }
        b'y' => {
            let bytes = body.fixed(aligned, 1)?;
            out.push(GVariantValue::Byte(bytes[0]));
            Ok(aligned + 1)
        }
        b'n' => {
            let bytes = body.fixed(aligned, 2)?;
            out.push(GVariantValue::Int(
                i16::from_le_bytes([bytes[0], bytes[1]]) as i64
            ));
            Ok(aligned + 2)
        }
        b'q' => {
            let bytes = body.fixed(aligned, 2)?;
            out.push(GVariantValue::Uint(
                u16::from_le_bytes([bytes[0], bytes[1]]) as u64,
            ));
            Ok(aligned + 2)
        }
        b'i' => {
            let bytes = body.fixed(aligned, 4)?;
            out.push(GVariantValue::Int(
                i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64,
            ));
            Ok(aligned + 4)
        }
        b'u' => {
            let bytes = body.fixed(aligned, 4)?;
            out.push(GVariantValue::Uint(u32::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3],
            ]) as u64));
            Ok(aligned + 4)
        }
        b'x' => {
            let bytes = body.fixed(aligned, 8)?;
            let mut raw = [0u8; 8];
            raw.copy_from_slice(bytes);
            out.push(GVariantValue::Int(i64::from_le_bytes(raw)));
            Ok(aligned + 8)
        }
        b't' => {
            let bytes = body.fixed(aligned, 8)?;
            let mut raw = [0u8; 8];
            raw.copy_from_slice(bytes);
            out.push(GVariantValue::Uint(u64::from_le_bytes(raw)));
            Ok(aligned + 8)
        }
        b'd' => {
            let bytes = body.fixed(aligned, 8)?;
            let mut raw = [0u8; 8];
            raw.copy_from_slice(bytes);
            out.push(GVariantValue::Double(f64::from_le_bytes(raw)));
            Ok(aligned + 8)
        }
        b's' | b'o' => {
            let (text, next) = body.string(aligned)?;
            out.push(GVariantValue::Str(text));
            Ok(next)
        }
        b'g' => {
            let len = *body
                .fixed(aligned, 1)?
                .first()
                .ok_or(GVariantError::Truncated)?;
            let start = aligned + 1;
            let text = std::str::from_utf8(body.fixed(start, len as usize)?)
                .map_err(|_| GVariantError::InvalidUtf8)?
                .to_string();
            out.push(GVariantValue::Str(text));
            Ok(start + len as usize + 1) // + NUL
        }
        b'v' => {
            // Variant: 'g' header (signature) followed by the inner body.
            let mut header = Vec::new();
            let after = decode_field(body, b"g", aligned, &mut header)?;
            let GVariantValue::Str(inner_sig) = header.remove(0) else {
                return Err(GVariantError::BadSignature);
            };
            let mut inner = Vec::new();
            let inner_sig = inner_sig.into_bytes();
            let _ = decode_field(body, &inner_sig, after, &mut inner)?;
            let value = inner.remove(0);
            out.push(GVariantValue::Variant(
                String::from_utf8(inner_sig).unwrap_or_default(),
                Box::new(value),
            ));
            Ok(after)
        }
        b'm' => {
            // Non-normal maybe: a present byte, then the child when set.
            let flag = *body
                .fixed(aligned, 1)?
                .first()
                .ok_or(GVariantError::Truncated)?;
            let child_sig = &sig[1..];
            if flag == 0 {
                out.push(GVariantValue::Maybe(None));
                Ok(aligned + 1)
            } else {
                let mut child = Vec::new();
                let next = decode_field(body, child_sig, aligned + 1, &mut child)?;
                out.push(GVariantValue::Maybe(Some(Box::new(child.remove(0)))));
                Ok(next)
            }
        }
        b'(' => {
            // Tuple: fields are aligned from the body start. Bounded by the
            // signature walk; a malformed body aborts with Truncated.
            let mut fields = Vec::new();
            let mut cursor = aligned;
            let mut i = 1usize; // skip '('
            let mut guard = 0usize;
            while i < sig.len() && sig[i] != b')' {
                let flen = complete_type_len(sig, i).ok_or(GVariantError::BadSignature)?;
                cursor = decode_field(body, &sig[i..i + flen], cursor, &mut fields)?;
                i += flen;
                guard += 1;
                if guard > 64 {
                    return Err(GVariantError::BadSignature);
                }
            }
            out.push(GVariantValue::Tuple(fields));
            Ok(cursor)
        }
        b'a' | b'{' => Err(GVariantError::UnsupportedType("array or dictionary")),
        _ => Err(GVariantError::BadSignature),
    }
}

/// Decode a framed GVDB value (dconf leaf or schema key entry).
///
/// For schema entries the caller usually needs only the **first tuple
/// field** (the default value); pass `first_field_only = true` to stop
/// after it — this keeps enum/range entries like `(s(y(ys))(yau))`
/// decodable without implementing the array forms they trail.
pub fn decode_framed(raw: &[u8], first_field_only: bool) -> Result<GVariantValue, GVariantError> {
    let (body, sig) = split_framed(raw)?;
    let body = Body { data: body };
    if first_field_only && sig.first() == Some(&b'(') {
        // First field of a tuple: decode just that complete type.
        let flen = complete_type_len(sig, 1).ok_or(GVariantError::BadSignature)?;
        let mut out = Vec::new();
        decode_field(&body, &sig[1..1 + flen], 0, &mut out)?;
        return Ok(out.remove(0));
    }
    let mut out = Vec::new();
    decode_at(&body, sig, 0, &mut out)?;
    Ok(out.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte samples captured verbatim from real databases (see module doc):
    /// dconf scratch compile + system gschemas.compiled.
    #[test]
    fn decodes_real_dconf_leaves() {
        // string value
        let v = decode_framed(b"prefer-dark\x00\x00s", false).unwrap();
        assert_eq!(v, GVariantValue::Str("prefer-dark".into()));
        // boolean
        assert_eq!(
            decode_framed(b"\x01\x00b", false).unwrap(),
            GVariantValue::Bool(true)
        );
        assert_eq!(
            decode_framed(b"\x00\x00b", false).unwrap(),
            GVariantValue::Bool(false)
        );
        // int32 (42) / uint32 (7) / double (1.0)
        assert_eq!(
            decode_framed(b"*\x00\x00\x00\x00i", false).unwrap(),
            GVariantValue::Int(42)
        );
        assert_eq!(
            decode_framed(b"\x07\x00\x00\x00\x00i", false).unwrap(),
            GVariantValue::Int(7)
        );
        assert_eq!(
            decode_framed(b"\x00\x00\x00\x00\x00\x00\xf0?\x00d", false).unwrap(),
            GVariantValue::Double(1.0)
        );
        // short string 'v'
        assert_eq!(
            decode_framed(b"v\x00\x00s", false).unwrap(),
            GVariantValue::Str("v".into())
        );
    }

    #[test]
    fn decodes_schema_default_tuples_first_field() {
        // (s): string default
        let v = decode_framed(b"Adwaita\x00\x00(s)", true).unwrap();
        assert_eq!(v, GVariantValue::Str("Adwaita".into()));
        // (b): bool default
        assert_eq!(
            decode_framed(b"\x00\x00(b)", true).unwrap(),
            GVariantValue::Bool(false)
        );
        // (i): int default (cursor-size 24)
        assert_eq!(
            decode_framed(b"\x18\x00\x00\x00\x00(i)", true).unwrap(),
            GVariantValue::Int(24)
        );
        // (d): double default with min/max range trail
        let v = decode_framed(
            b"\x00\x00\x00\x00\x00\x00\xf0?r\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xe0?\x00\x00\x00\x00\x00\x00\x08@\x00(d(y(dd)))",
            true,
        )
        .unwrap();
        assert_eq!(v, GVariantValue::Double(1.0));
        // (i(y(ii))): int default with min/max
        let v = decode_framed(
            b"\xb0\x04\x00\x00r\x00\x00\x00d\x00\x00\x00\xc4\t\x00\x00\x00(i(y(ii)))",
            true,
        )
        .unwrap();
        assert_eq!(v, GVariantValue::Int(1200));
        // enum (s(y(ys))(yau)): first field is the default string; the
        // choice array is *not* decoded (documented subset).
        let v = decode_framed(
            b"24h\x00lt'24h'\x00e\x00\x00\x00\x00\x00\x00\x00\xff24h\x00\x00\x00\xff\x01\x00\x00\x00\xff12h\x00\x00\x00\xff\x0c\x04\x00(s(y(ys))(yau))",
            true,
        )
        .unwrap();
        assert_eq!(v, GVariantValue::Str("24h".into()));
        // (u): uint default (scaling-factor)
        assert_eq!(
            decode_framed(b"\x00\x00\x00\x00\x00(u)", true).unwrap(),
            GVariantValue::Uint(0)
        );
    }

    #[test]
    fn flat_tuple_fields_decode_in_order() {
        // (is) from the scratch db: 1, "x"
        let v = decode_framed(b"\x01\x00\x00\x00x\x00\x00(is)", false).unwrap();
        assert_eq!(
            v,
            GVariantValue::Tuple(vec![GVariantValue::Int(1), GVariantValue::Str("x".into())])
        );
    }

    #[test]
    fn rejects_malformed_and_unsupported() {
        assert_eq!(
            decode_framed(b"no-nul-here", false),
            Err(GVariantError::Unframed)
        );
        assert_eq!(
            decode_framed(b"\x00", false),
            Err(GVariantError::BadSignature)
        );
        // truncated int: body has fewer bytes than the signature claims
        assert!(matches!(
            decode_framed(b"*\x00\x00\x00i", false),
            Err(GVariantError::Truncated)
        ));
        // arrays are explicitly unsupported, never guessed
        assert!(matches!(
            decode_framed(b"a\x00bc\x00\x02\x05\x00as", false),
            Err(GVariantError::UnsupportedType(_))
        ));
        // invalid UTF-8 in a string body
        assert!(matches!(
            decode_framed(b"\xff\xfe\x00\x00s", false),
            Err(GVariantError::InvalidUtf8)
        ));
    }
}
