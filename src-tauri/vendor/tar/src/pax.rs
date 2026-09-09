#![allow(dead_code)]
use std::io;
use std::io::Write;
use std::str;

use crate::other;

// Keywords for PAX extended header records.
pub const PAX_NONE: &str = ""; // Indicates that no PAX key is suitable
pub const PAX_PATH: &str = "path";
pub const PAX_LINKPATH: &str = "linkpath";
pub const PAX_SIZE: &str = "size";
pub const PAX_UID: &str = "uid";
pub const PAX_GID: &str = "gid";
pub const PAX_UNAME: &str = "uname";
pub const PAX_GNAME: &str = "gname";
pub const PAX_MTIME: &str = "mtime";
pub const PAX_ATIME: &str = "atime";
pub const PAX_CTIME: &str = "ctime"; // Removed from later revision of PAX spec, but was valid
pub const PAX_CHARSET: &str = "charset"; // Currently unused
pub const PAX_COMMENT: &str = "comment"; // Currently unused

pub const PAX_SCHILYXATTR: &str = "SCHILY.xattr.";

// Keywords for GNU sparse files in a PAX extended header.
pub const PAX_GNUSPARSE: &str = "GNU.sparse.";
pub const PAX_GNUSPARSENUMBLOCKS: &str = "GNU.sparse.numblocks";
pub const PAX_GNUSPARSEOFFSET: &str = "GNU.sparse.offset";
pub const PAX_GNUSPARSENUMBYTES: &str = "GNU.sparse.numbytes";
pub const PAX_GNUSPARSEMAP: &str = "GNU.sparse.map";
pub const PAX_GNUSPARSENAME: &str = "GNU.sparse.name";
pub const PAX_GNUSPARSEMAJOR: &str = "GNU.sparse.major";
pub const PAX_GNUSPARSEMINOR: &str = "GNU.sparse.minor";
pub const PAX_GNUSPARSESIZE: &str = "GNU.sparse.size";
pub const PAX_GNUSPARSEREALSIZE: &str = "GNU.sparse.realsize";

/// An iterator over the pax extensions in an archive entry.
///
/// This iterator yields structures which can themselves be parsed into
/// key/value pairs.
pub struct PaxExtensions<'entry> {
    data: &'entry [u8],
}

impl<'entry> PaxExtensions<'entry> {
    /// Create new pax extensions iterator from the given entry data.
    pub fn new(a: &'entry [u8]) -> Self {
        PaxExtensions { data: a }
    }
}

/// A key/value pair corresponding to a pax extension.
pub struct PaxExtension<'entry> {
    key: &'entry [u8],
    value: &'entry [u8],
}

pub fn pax_extensions_value(a: &[u8], key: &str) -> Option<u64> {
    for extension in PaxExtensions::new(a) {
        let current_extension = match extension {
            Ok(ext) => ext,
            Err(_) => return None,
        };
        if current_extension.key() != Ok(key) {
            continue;
        }

        let value = match current_extension.value() {
            Ok(value) => value,
            Err(_) => return None,
        };
        let result = match value.parse::<u64>() {
            Ok(result) => result,
            Err(_) => return None,
        };
        return Some(result);
    }
    None
}

impl<'entry> Iterator for PaxExtensions<'entry> {
    type Item = io::Result<PaxExtension<'entry>>;

    fn next(&mut self) -> Option<io::Result<PaxExtension<'entry>>> {
        if self.data.is_empty() { return None; }
        // PAX records are length-prefixed, not line-delimited. In particular,
        // SCHILY xattrs contain arbitrary bytes, including embedded newlines.
        let data = self.data;
        // Fail closed and terminate after a malformed record.
        self.data = &[];
        let parsed = (|| {
            let space = data.iter().position(|b| *b == b' ')?;
            if space == 0 || !data[..space].iter().all(u8::is_ascii_digit) { return None; }
            let length = str::from_utf8(&data[..space]).ok()?.parse::<usize>().ok()?;
            let record = data.get(..length)?;
            if record.last() != Some(&b'\n') { return None; }
            let payload = record.get(space + 1..length.checked_sub(1)?)?;
            let equals = payload.iter().position(|b| *b == b'=')?;
            if equals == 0 { return None; }
            Some((length, PaxExtension { key: &payload[..equals], value: &payload[equals + 1..] }))
        })();
        Some(match parsed {
            Some((length, extension)) => {
                self.data = &data[length..];
                Ok(extension)
            }
            None => Err(other("malformed pax extension")),
        })
    }
}

impl<'entry> PaxExtension<'entry> {
    /// Returns the key for this key/value pair parsed as a string.
    ///
    /// May fail if the key isn't actually utf-8.
    pub fn key(&self) -> Result<&'entry str, str::Utf8Error> {
        str::from_utf8(self.key)
    }

    /// Returns the underlying raw bytes for the key of this key/value pair.
    pub fn key_bytes(&self) -> &'entry [u8] {
        self.key
    }

    /// Returns the value for this key/value pair parsed as a string.
    ///
    /// May fail if the value isn't actually utf-8.
    pub fn value(&self) -> Result<&'entry str, str::Utf8Error> {
        str::from_utf8(self.value)
    }

    /// Returns the underlying raw bytes for this value of this key/value pair.
    pub fn value_bytes(&self) -> &'entry [u8] {
        self.value
    }
}

/// Extension trait for `Builder` to append PAX extended headers.
impl<T: Write> crate::Builder<T> {
    /// Append PAX extended headers to the archive.
    ///
    /// Takes in an iterator over the list of headers to add to convert it into a header set formatted.
    ///
    /// Returns io::Error if an error occurs, else it returns ()
    pub fn append_pax_extensions<'key, 'value>(
        &mut self,
        headers: impl IntoIterator<Item = (&'key str, &'value [u8])>,
    ) -> Result<(), io::Error> {
        // Store the headers formatted before write
        let mut data: Vec<u8> = Vec::new();

        // For each key in headers, convert into a sized space and add it to data.
        // This will then be written in the file
        for (key, value) in headers {
            let mut len_len = 1;
            let mut max_len = 10;
            let rest_len = 3 + key.len() + value.len();
            while rest_len + len_len >= max_len {
                len_len += 1;
                max_len *= 10;
            }
            let len = rest_len + len_len;
            write!(&mut data, "{} {}=", len, key)?;
            data.extend_from_slice(value);
            data.push(b'\n');
        }

        // Ignore the header append if it's empty.
        if data.is_empty() {
            return Ok(());
        }

        // Create a header of type XHeader, set the size to the length of the
        // data, set the entry type to XHeader, and set the checksum
        // then append the header and the data to the archive.
        let mut header = crate::Header::new_ustar();
        let data_as_bytes: &[u8] = &data;
        header.set_size(data_as_bytes.len() as u64);
        header.set_entry_type(crate::EntryType::XHeader);
        header.set_cksum();
        self.append(&header, data_as_bytes)
    }
}

#[cfg(test)]
mod length_record_tests {
    use super::*;
    fn record(key: &str, value: &[u8]) -> Vec<u8> {
        let mut length = key.len() + value.len() + 3;
        loop {
            let next = key.len() + value.len() + 3 + length.to_string().len();
            if next == length { break; }
            length = next;
        }
        let mut bytes = format!("{length} {key}=").into_bytes();
        bytes.extend_from_slice(value); bytes.push(b'\n'); bytes
    }
    #[test]
    fn binary_multiline_xattr_and_following_path() {
        let value = b"binary\0value\nwith newline\xff=end";
        let mut bytes = record("SCHILY.xattr.com.example.backup", value);
        bytes.extend(record("path", b"root/name\nsecond line"));
        let mut items = PaxExtensions::new(&bytes);
        let first = items.next().unwrap().unwrap();
        assert_eq!(first.key().unwrap(), "SCHILY.xattr.com.example.backup");
        assert_eq!(first.value_bytes(), value);
        assert_eq!(items.next().unwrap().unwrap().value_bytes(), b"root/name\nsecond line");
        assert!(items.next().is_none());
    }
    #[test]
    fn rejects_invalid_records_and_stops() {
        for bytes in [b"0 k=v\n".as_slice(), b"-1 k=v\n", b"+7 k=v\n", b"9999999999999999999999999999 k=v\n", b"7 k=v", b"7 k=v!", b"6 =v!\n", b"7 key!\n", b"\n", b"7 k=v\n"] {
            let mut items = PaxExtensions::new(bytes);
            assert!(items.next().unwrap().is_err(), "{bytes:?}");
            assert!(items.next().is_none());
        }
    }
    #[test]
    fn accepts_empty_value_and_length_digit_boundaries() {
        for size in [0, 1, 80, 90, 980, 990, 10000] {
            let value = vec![b'x'; size]; let bytes = record("key", &value);
            assert_eq!(PaxExtensions::new(&bytes).next().unwrap().unwrap().value_bytes(), value);
        }
    }
}
