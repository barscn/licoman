//! Minimal dependency-free D-Bus session-bus probe (03_ENVIRONMENT_-
//! DISCOVERY.md §4).
//!
//! Speaks just enough of the D-Bus wire protocol to answer one question —
//! "is there a session bus, and which names does it hold?" — via `Hello` +
//! `ListNames`. This is strictly a **passive probe**: no service is
//! activated, no daemon launched, nothing written.
//!
//! Why hand-rolled: the D-Bus method-call surface used here is two calls
//! with no arguments; a client library (zbus et al.) would pull a large
//! async dependency tree into the UI-independent core for the same result
//! (D25). Marshal/parse are pure functions with exact-byte unit tests; the
//! socket path lives only in the impure `probe_session_bus`.
//!
//! Supported transports: `unix:path=…` and `unix:abstract=…` (little-endian
//! only, like the rest of the format work — both supported targets are LE).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const ENDIAN: u8 = b'l';
const PROTOCOL_VERSION: u8 = 1;
const MESSAGE_METHOD_CALL: u8 = 1;
const MESSAGE_METHOD_RETURN: u8 = 2;
const MESSAGE_ERROR: u8 = 3;
const MESSAGE_SIGNAL: u8 = 4;

const FIELD_PATH: u8 = 1;
const FIELD_INTERFACE: u8 = 2;
const FIELD_MEMBER: u8 = 3;
const FIELD_ERROR_NAME: u8 = 4;
const FIELD_REPLY_SERIAL: u8 = 5;
const FIELD_DESTINATION: u8 = 6;

const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";

/// A parsed reply from the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub kind: u8,
    /// `ERROR_NAME` field for error replies.
    pub error_name: Option<String>,
    /// `REPLY_SERIAL` field: which call this message answers.
    pub reply_serial: Option<u32>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    Malformed(&'static str),
}

/// Marshal a little-endian method call with no arguments. Pure.
///
/// Header-field layout (verified byte-for-byte against a captured libdbus
/// `Hello`): each field is a struct (yv) whose start is 8-byte aligned
/// relative to the message; the `y` code is followed **immediately** by the
/// variant's `g` signature (alignment 1); the variant's value is 4-byte
/// aligned relative to the message. `PATH` carries an object-path value
/// (signature `o`), all other fields used here are strings (`s`).
pub fn marshal_method_call(serial: u32, member: &str, destination: &str) -> Vec<u8> {
    let mut fields = Vec::new();
    let mut offset = 16usize;
    let push_field = |fields: &mut Vec<u8>, offset: &mut usize, code: u8, value: &str| {
        while !(*offset).is_multiple_of(8) {
            fields.push(0);
            *offset += 1;
        }
        fields.push(code);
        *offset += 1;
        // variant signature `g`: u8 length + bytes + NUL, alignment 1
        let sig: &[u8] = if code == FIELD_PATH { b"o" } else { b"s" };
        fields.push(sig.len() as u8);
        fields.extend_from_slice(sig);
        fields.push(0);
        *offset += sig.len() + 2;
        // variant value: 4-byte aligned u32 length + bytes + NUL
        while !(*offset).is_multiple_of(4) {
            fields.push(0);
            *offset += 1;
        }
        let len = value.len() as u32;
        fields.extend_from_slice(&len.to_le_bytes());
        fields.extend_from_slice(value.as_bytes());
        fields.push(0);
        *offset += 4 + value.len() + 1;
    };
    push_field(&mut fields, &mut offset, FIELD_PATH, DBUS_PATH);
    push_field(&mut fields, &mut offset, FIELD_DESTINATION, destination);
    push_field(&mut fields, &mut offset, FIELD_INTERFACE, DBUS_INTERFACE);
    push_field(&mut fields, &mut offset, FIELD_MEMBER, member);

    let mut message = Vec::with_capacity(16 + fields.len() + 8);
    message.push(ENDIAN);
    message.push(MESSAGE_METHOD_CALL);
    message.push(0); // flags: none
    message.push(PROTOCOL_VERSION);
    message.extend_from_slice(&0u32.to_le_bytes()); // body length, patched below
    message.extend_from_slice(&serial.to_le_bytes());
    message.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    message.extend_from_slice(&fields);
    while message.len() % 8 != 0 {
        message.push(0); // body starts on an 8-byte boundary
    }
    // No-argument calls have an empty body: the header field at 4..8 is the
    // BODY length (0), not the total message length.
    message[4..8].copy_from_slice(&0u32.to_le_bytes());
    message
}

/// Parse the fixed header + fields of a reply. Pure.
pub fn parse_reply(data: &[u8]) -> Result<Reply, WireError> {
    if data.len() < 16 {
        return Err(WireError::Malformed("short header"));
    }
    let kind = data[1];
    let body_len = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let fields_len = u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;
    let fields_end = 16usize
        .checked_add(fields_len)
        .ok_or(WireError::Malformed("fields length"))?;
    if fields_end > data.len() {
        return Err(WireError::Malformed("fields exceed message"));
    }
    let body_start = fields_end.div_ceil(8) * 8;
    if body_start + body_len > data.len() {
        return Err(WireError::Malformed("body exceeds message"));
    }

    let mut error_name = None;
    let mut reply_serial = None;
    let mut offset = 16usize;
    while offset < fields_end {
        offset = offset.div_ceil(8) * 8; // struct (yv) start, 8-aligned
        if offset >= fields_end {
            break;
        }
        let code = data[offset];
        offset += 1;
        // variant: `g` signature directly after the code (alignment 1)
        if offset >= fields_end {
            return Err(WireError::Malformed("variant signature"));
        }
        let sig_len = data[offset] as usize;
        let sig_start = offset + 1;
        let sig = data
            .get(sig_start..sig_start + sig_len)
            .ok_or(WireError::Malformed("signature"))?;
        let mut value_offset = sig_start + sig_len + 1; // + NUL
        match sig {
            b"s" | b"o" => {
                value_offset = value_offset.div_ceil(4) * 4;
                let (text, next) = read_string(data, value_offset, fields_end)?;
                if code == FIELD_ERROR_NAME {
                    error_name = Some(text);
                }
                offset = next;
            }
            b"u" => {
                value_offset = value_offset.div_ceil(4) * 4;
                if value_offset + 4 > fields_end {
                    return Err(WireError::Malformed("u32 field"));
                }
                if code == FIELD_REPLY_SERIAL {
                    reply_serial = Some(u32::from_le_bytes([
                        data[value_offset],
                        data[value_offset + 1],
                        data[value_offset + 2],
                        data[value_offset + 3],
                    ]));
                }
                offset = value_offset + 4;
            }
            // SIGNATURE fields (code 8) carry a `g` value: u8 length +
            // bytes + NUL, alignment 1. Seen in real replies.
            b"g" => {
                if value_offset >= fields_end {
                    return Err(WireError::Malformed("signature value"));
                }
                let len = data[value_offset] as usize;
                let end = value_offset + 1 + len + 1;
                if end > fields_end {
                    return Err(WireError::Malformed("signature value end"));
                }
                offset = end;
            }
            _ => return Err(WireError::Malformed("unsupported field signature")),
        }
    }

    Ok(Reply {
        kind,
        error_name,
        reply_serial,
        body: data[body_start..body_start + body_len].to_vec(),
    })
}

fn read_string(data: &[u8], offset: usize, limit: usize) -> Result<(String, usize), WireError> {
    if offset + 4 > limit {
        return Err(WireError::Malformed("string length"));
    }
    let len = u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]) as usize;
    let start = offset + 4;
    let end = start
        .checked_add(len + 1)
        .ok_or(WireError::Malformed("string end"))?;
    if end > limit {
        return Err(WireError::Malformed("string exceeds region"));
    }
    let text = std::str::from_utf8(&data[start..start + len])
        .map_err(|_| WireError::Malformed("string utf8"))?
        .to_string();
    Ok((text, end))
}

/// Parse a body of signature `s` (Hello's reply). Pure.
pub fn parse_single_string(body: &[u8]) -> Result<String, WireError> {
    read_string(body, 0, body.len()).map(|(text, _)| text)
}

/// Parse a body of signature `as` (ListNames's reply). Pure.
pub fn parse_string_array(body: &[u8]) -> Result<Vec<String>, WireError> {
    if body.len() < 4 {
        return Err(WireError::Malformed("array length"));
    }
    let array_len = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
    let end = 4usize
        .checked_add(array_len)
        .ok_or(WireError::Malformed("array end"))?;
    if end > body.len() {
        return Err(WireError::Malformed("array exceeds body"));
    }
    let mut names = Vec::new();
    let mut offset = 4usize;
    while offset < end {
        offset = offset.div_ceil(4) * 4;
        if offset >= end {
            break;
        }
        let (text, next) = read_string(body, offset, end)?;
        names.push(text);
        offset = next;
    }
    Ok(names)
}

/// The resolved session-bus address. Pure over the given inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BusAddress {
    Path(String),
    Abstract(String),
}

/// Resolve the session-bus address from `DBUS_SESSION_BUSADDRESS` or the
/// default `$XDG_RUNTIME_DIR/bus` socket. Pure; returns `None` when no bus
/// can be located (a documented safe-unavailable state, never an error).
pub fn session_bus_address(
    dbus_address: Option<&str>,
    runtime_dir: Option<&str>,
) -> Option<BusAddress> {
    if let Some(address) = dbus_address {
        // `unix:path=/run/user/1000/bus,guid=…`; multiple transports are
        // `;`-separated — the first usable unix one wins.
        for transport in address.split(';') {
            let transport = transport.trim();
            if let Some(rest) = transport.strip_prefix("unix:") {
                for pair in rest.split(',') {
                    if let Some(path) = pair.strip_prefix("path=") {
                        return Some(BusAddress::Path(path.to_string()));
                    }
                    if let Some(name) = pair.strip_prefix("abstract=") {
                        return Some(BusAddress::Abstract(name.to_string()));
                    }
                }
            }
        }
        return None;
    }
    runtime_dir
        .filter(|dir| !dir.is_empty())
        .map(|dir| BusAddress::Path(format!("{dir}/bus")))
}

fn connect(address: &BusAddress) -> std::io::Result<UnixStream> {
    let stream = match address {
        BusAddress::Path(path) => UnixStream::connect(path)?,
        BusAddress::Abstract(name) => {
            use std::os::linux::net::SocketAddrExt;
            let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
            UnixStream::connect_addr(&addr)?
        }
    };
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    Ok(stream)
}

/// The SASL EXTERNAL handshake: `\0AUTH EXTERNAL <hex-uid>\r\n` → `OK <guid>`,
/// then `BEGIN`. The uid comes from `/proc/self/status` (no libc).
fn authenticate(stream: &mut UnixStream) -> Result<(), String> {
    let uid = read_uid();
    let hex_uid: String = uid.bytes().map(|byte| format!("{byte:02x}")).collect();
    stream
        .write_all(b"\0")
        .and_then(|_| stream.write_all(format!("AUTH EXTERNAL {hex_uid}\r\n").as_bytes()))
        .map_err(|e| format!("auth write failed: {e}"))?;
    let line = read_line(stream)?;
    if !line.starts_with("OK ") {
        return Err("bus rejected EXTERNAL auth".to_string());
    }
    stream
        .write_all(b"BEGIN\r\n")
        .map_err(|e| format!("begin write failed: {e}"))
}

fn read_uid() -> String {
    // "Uid:\t1000\t1000\t1000\t1000" — the real uid is the first number.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("Uid:"))
                .and_then(|line| line.split_whitespace().nth(1).map(str::to_string))
        })
        .unwrap_or_else(|| "1000".to_string())
}

fn read_line(stream: &mut UnixStream) -> Result<String, String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if line.len() > 256 {
            return Err("auth reply too long".to_string());
        }
        stream
            .read(&mut byte)
            .map_err(|e| format!("auth read failed: {e}"))?;
        if byte[0] == b'\n' {
            return Ok(String::from_utf8_lossy(&line)
                .trim_end_matches('\r')
                .to_string());
        }
        line.push(byte[0]);
    }
}

/// Read one complete message from the stream.
fn read_message(stream: &mut UnixStream, member: &str) -> Result<Reply, String> {
    let mut header = [0u8; 16];
    stream
        .read_exact(&mut header)
        .map_err(|e| format!("{member} header read failed: {e}"))?;
    let body_len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let fields_len = u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize;
    let fields_end = 16 + fields_len;
    let body_start = fields_end.div_ceil(8) * 8;
    let total = body_start + body_len;
    if total > 64 * 1024 * 1024 {
        return Err(format!("{member} reply too large"));
    }
    let mut rest = vec![0u8; total - 16];
    stream
        .read_exact(&mut rest)
        .map_err(|e| format!("{member} body read failed: {e}"))?;
    let mut message = header.to_vec();
    message.extend_from_slice(&rest);
    parse_reply(&message).map_err(|e| format!("{member} reply parse failed: {e:?}"))
}

fn call(stream: &mut UnixStream, serial: u32, member: &str) -> Result<Reply, String> {
    let message = marshal_method_call(serial, member, DBUS_SERVICE);
    stream
        .write_all(&message)
        .map_err(|e| format!("{member} write failed: {e}"))?;
    // The bus may deliver signals (e.g. NameAcquired) before the reply;
    // skip every non-reply message until ours arrives (matched by
    // REPLY_SERIAL when present).
    for _ in 0..64 {
        let reply = read_message(stream, member)?;
        match reply.kind {
            MESSAGE_SIGNAL => continue,
            MESSAGE_ERROR => {
                return Err(format!(
                    "{} returned error {}",
                    member,
                    reply.error_name.unwrap_or_else(|| "unknown".into())
                ));
            }
            MESSAGE_METHOD_RETURN if reply.reply_serial.is_none_or(|rs| rs == serial) => {
                return Ok(reply);
            }
            _ => continue, // a reply for something else; skip
        }
    }
    Err(format!(
        "{member} reply not received (too many intervening messages)"
    ))
}

/// Outcome of the live probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusProbe {
    pub available: bool,
    /// Well-known names on the bus (ListNames). Bounded to a sane count.
    pub names: Vec<String>,
    pub reason: Option<String>,
}

/// Probe the session bus. Passive: connects, authenticates, calls `Hello`
/// and `ListNames`, disconnects. Any failure becomes `available: false`
/// plus a reason — never a panic, never a retry storm.
pub fn probe_session_bus(dbus_address: Option<&str>, runtime_dir: Option<&str>) -> BusProbe {
    let Some(address) = session_bus_address(dbus_address, runtime_dir) else {
        return BusProbe {
            available: false,
            names: Vec::new(),
            reason: Some("no session bus address found".to_string()),
        };
    };
    let mut stream = match connect(&address) {
        Ok(stream) => stream,
        Err(error) => {
            return BusProbe {
                available: false,
                names: Vec::new(),
                reason: Some(format!("session bus connect failed: {error}")),
            };
        }
    };
    let result = (|| -> Result<Vec<String>, String> {
        authenticate(&mut stream)?;
        let _hello = call(&mut stream, 1, "Hello")?;
        let names_reply = call(&mut stream, 2, "ListNames")?;
        parse_string_array(&names_reply.body)
            .map_err(|e| format!("ListNames body parse failed: {e:?}"))
    })();
    match result {
        Ok(mut names) => {
            names.truncate(4096);
            BusProbe {
                available: true,
                names,
                reason: None,
            }
        }
        Err(reason) => BusProbe {
            available: false,
            names: Vec::new(),
            reason: Some(reason),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exact wire bytes of a real libdbus `Hello` (captured by connecting
    /// the reference client to a recording fake bus): header fields are
    /// structs (yv) at 8-byte alignment; the variant `g` follows the code
    /// byte immediately; PATH's variant signature is `o`. Fields span
    /// 16..126 (length 110), message padded to 128.
    const HELLO: &[u8] = &[
        b'l', 1, 0, 1, // endian, method_call, flags, version
        0, 0, 0, 0, // body length: 0 (no arguments)
        1, 0, 0, 0, // serial 1
        110, 0, 0, 0, // fields array length: 110
        // --- PATH at 16: code, g-sig "o", aligned value ---
        1, 1, b'o', 0, 21, 0, 0, 0, b'/', b'o', b'r', b'g', b'/', b'f', b'r', b'e', b'e', b'd',
        b'e', b's', b'k', b't', b'o', b'p', b'/', b'D', b'B', b'u', b's', 0, 0, 0,
        // --- DESTINATION at 48 ---
        6, 1, b's', 0, 20, 0, 0, 0, b'o', b'r', b'g', b'.', b'f', b'r', b'e', b'e', b'd', b'e',
        b's', b'k', b't', b'o', b'p', b'.', b'D', b'B', b'u', b's', 0, 0, 0, 0,
        // --- INTERFACE at 80 ---
        2, 1, b's', 0, 20, 0, 0, 0, b'o', b'r', b'g', b'.', b'f', b'r', b'e', b'e', b'd', b'e',
        b's', b'k', b't', b'o', b'p', b'.', b'D', b'B', b'u', b's', 0, 0, 0, 0,
        // --- MEMBER at 112 ---
        3, 1, b's', 0, 5, 0, 0, 0, b'H', b'e', b'l', b'l', b'o', 0, 0, 0,
    ];

    #[test]
    fn hello_message_matches_exact_wire_bytes() {
        assert_eq!(
            marshal_method_call(1, "Hello", DBUS_SERVICE),
            HELLO.to_vec()
        );
    }

    #[test]
    fn parses_method_return_with_string_body() {
        // Reply: header + one field (REPLY_SERIAL would be here; minimal
        // variant: no fields) + body "s" = "…guid…".
        let text = b"1234abcdef";
        let mut body = (text.len() as u32).to_le_bytes().to_vec();
        body.extend_from_slice(text);
        body.push(0);
        // struct (yv) per the real layout: code, g-sig "u" immediately,
        // u32 value 4-aligned relative to the message (20..24).
        let fields = vec![5, 1, b'u', 0, 2, 0, 0, 0];
        let mut message = vec![b'l', MESSAGE_METHOD_RETURN, 0, 1];
        message.extend_from_slice(&(body.len() as u32).to_le_bytes());
        message.extend_from_slice(&1u32.to_le_bytes()); // serial
        message.extend_from_slice(&(fields.len() as u32).to_le_bytes());
        message.extend_from_slice(&fields);
        while message.len() % 8 != 0 {
            message.push(0);
        }
        message.extend_from_slice(&body);
        let reply = parse_reply(&message).expect("parses");
        assert_eq!(reply.kind, MESSAGE_METHOD_RETURN);
        assert_eq!(reply.error_name, None);
        assert_eq!(reply.reply_serial, Some(2));
        assert_eq!(parse_single_string(&reply.body).unwrap(), "1234abcdef");
    }

    #[test]
    fn parses_error_reply_name() {
        let mut fields = vec![4, 1, b's', 0]; // code + g-sig "s" (offsets 16..20)
        let name = b"org.freedesktop.DBus.Error.AccessDenied";
        fields.extend_from_slice(&(name.len() as u32).to_le_bytes());
        fields.extend_from_slice(name);
        fields.push(0);
        let mut message = vec![b'l', MESSAGE_ERROR, 0, 1];
        message.extend_from_slice(&0u32.to_le_bytes());
        message.extend_from_slice(&9u32.to_le_bytes());
        message.extend_from_slice(&(fields.len() as u32).to_le_bytes());
        message.extend_from_slice(&fields);
        while message.len() % 8 != 0 {
            message.push(0);
        }
        let reply = parse_reply(&message).expect("parses");
        assert_eq!(reply.kind, MESSAGE_ERROR);
        assert_eq!(
            reply.error_name.as_deref(),
            Some("org.freedesktop.DBus.Error.AccessDenied")
        );
    }

    #[test]
    fn parses_string_array_bodies() {
        let mut body = Vec::new();
        let mut strings = Vec::new();
        for name in ["org.gnome.Shell", "ca.desrt.dconf"] {
            let mut s = Vec::new();
            s.extend_from_slice(&(name.len() as u32).to_le_bytes());
            s.extend_from_slice(name.as_bytes());
            s.push(0);
            strings.push(s);
        }
        let s0 = &strings[0];
        let s1 = &strings[1];
        // 20 + 19 bytes of elements; both element starts are 4-aligned
        // relative to the body start, so no padding is inserted.
        let array_len = s0.len() + s1.len();
        body.extend_from_slice(&(array_len as u32).to_le_bytes());
        body.extend_from_slice(s0);
        body.extend_from_slice(s1);
        let names = parse_string_array(&body).expect("parses");
        assert_eq!(names, vec!["org.gnome.Shell", "ca.desrt.dconf"]);

        // An element whose total is not a multiple of 4 leaves a misaligned
        // next start; the parser consumes the single pad byte.
        let mut body2 = Vec::new();
        let mut odd = Vec::new();
        odd.extend_from_slice(&2u32.to_le_bytes());
        odd.extend_from_slice(b"ab");
        odd.push(0);
        body2.extend_from_slice(&((odd.len() + 1 + odd.len()) as u32).to_le_bytes());
        body2.extend_from_slice(&odd);
        body2.push(0); // pad to 4-align the next element
        body2.extend_from_slice(&odd);
        let names2 = parse_string_array(&body2).expect("parses padded");
        assert_eq!(names2, vec!["ab", "ab"]);
    }

    #[test]
    fn address_resolution_is_pure_and_safe() {
        assert_eq!(
            session_bus_address(Some("unix:path=/run/user/1000/bus,guid=abc"), None),
            Some(BusAddress::Path("/run/user/1000/bus".into()))
        );
        assert_eq!(
            session_bus_address(Some("unix:abstract=/tmp/dbus-x1"), None),
            Some(BusAddress::Abstract("/tmp/dbus-x1".into()))
        );
        assert_eq!(
            session_bus_address(None, Some("/run/user/1000")),
            Some(BusAddress::Path("/run/user/1000/bus".into()))
        );
        assert_eq!(session_bus_address(None, None), None);
        // Malformed address → no bus, not a guess.
        assert_eq!(session_bus_address(Some("tcp:host=localhost"), None), None);
    }

    #[test]
    fn probe_without_bus_is_safe_unavailable() {
        let probe = probe_session_bus(None, None);
        assert!(!probe.available);
        assert!(probe.reason.is_some());
        assert!(probe.names.is_empty());
    }
}
