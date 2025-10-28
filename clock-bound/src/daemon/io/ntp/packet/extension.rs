//! NTP Extension Fields
//!
//! This module implements NTP extension fields as defined in RFC 5905 Section 7.5.
//! Extension fields provide additional data in NTP packets and are inserted
//! after the NTP header and before the MAC (when present).
//!
//! See: <https://www.rfc-editor.org/rfc/rfc5905.html>

use bytes::BufMut;
use nom::error::{Error, ErrorKind};
use nom::number::complete::{be_u8, be_u16, be_u64};

/// NTP Extension Field variants
///
/// Extension fields follow the standard NTP extension format from RFC 5905:
/// - Field Type (2 bytes): Identifies the extension type
/// - Length (2 bytes): Total length including padding, in octets
/// - Value (variable): Extension-specific payload
/// - Padding (as needed): Zero-padded to 4-octet boundary
///
///  0                   1                   2                   3
///  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |          Field Type           |            Length             |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// .                                                               .
/// .                            Value                              .
/// .                                                               .
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                       Padding (as needed)                     |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionField {
    Fec2V1(Fec2V1Value),
}

/// 0xFEC2 Extension Field (v1)
///
/// This extension provides ClockBound version information.
/// Field Type: 0xFEC2, Total Length: 16 octets (4-octet aligned).
/// Note 0xFEC2 is in the range of field types reserved for private and
/// experimental use (0xF000 - 0xFFFF).
/// See: <https://www.rfc-editor.org/rfc/rfc9748.html>
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fec2V1Value {
    /// Major version of this ClockBound
    pub major_version: u8,
    /// Minor version of this ClockBound
    pub minor_version: u8,
    /// Unique id generated on each daemon startup
    pub startup_id: u64,
}

impl ExtensionField {
    /// Returns the NTP extension Field Type as defined in RFC 5905
    ///
    /// A 16-bit value that uniquely identifies the specific extension field and its function
    ///
    /// # Returns
    /// - `0xFEC2` for `Fec2V1` extensions
    pub fn field_type(&self) -> u16 {
        match self {
            ExtensionField::Fec2V1 { .. } => 0xFEC2,
        }
    }

    /// Returns the total Length field value as defined in RFC 5905
    ///
    /// The Length field is a 16-bit unsigned integer that indicates the length
    /// of the entire extension field in octets, including any padding needed
    /// to align the whole extension field to a 4-octet boundary.
    ///
    /// # Returns
    /// - `16` octets for `Fec2V1` extensions
    pub fn length(&self) -> u16 {
        match self {
            ExtensionField::Fec2V1 { .. } => 16, // 4 header + 12 payload, already aligned
        }
    }

    /// Serializes the extension field to bytes in network byte order
    ///
    /// # Arguments
    /// * `buffer` - Mutable byte slice to write the extension data to
    ///
    /// # Panics
    /// Panics if the buffer is smaller than the extension length
    pub fn emit_bytes(&self, buffer: &mut [u8]) {
        let mut buf = buffer;
        buf.put_u16_ne(self.field_type().to_be());
        buf.put_u16_ne(self.length().to_be());

        match self {
            ExtensionField::Fec2V1(fec2v1) => {
                buf.put_u8(1); // 0xFEC2v1
                buf.put_u8(fec2v1.major_version);
                buf.put_u8(fec2v1.minor_version);
                buf.put_u8(0);
                buf.put_u64_ne(fec2v1.startup_id.to_be());
            }
        }
    }

    /// Parses an extension field from bytes in network byte order
    ///
    /// Attempts to parse an NTP extension field from the provided byte slice
    /// according to RFC 5905 format. The input should contain the complete
    /// extension including Field Type, Length, Value, and any padding.
    ///
    /// # Arguments
    /// * `input` - Byte slice containing the extension field data
    ///
    /// # Returns
    /// * `Ok((remaining_slice, extension))` - Successfully parsed extension and remaining bytes
    /// * `Err(_)` - Parse error for unknown field types or malformed data
    ///
    /// # Errors
    /// Returns a nom parsing error if:
    /// - The Field Type is not recognized
    /// - The input is too short for the expected format
    /// - The data is malformed
    pub fn parse_from_bytes(input: &[u8]) -> nom::IResult<&[u8], ExtensionField> {
        let (input, field_type) = be_u16(input)?;
        let (input, _length) = be_u16(input)?;

        match field_type {
            0xFEC2 => {
                let (input, _ext_version) = be_u8(input)?;
                let (input, major_version) = be_u8(input)?;
                let (input, minor_version) = be_u8(input)?;
                let (input, _) = be_u8(input)?;
                let (input, startup_id) = be_u64(input)?;

                Ok((
                    input,
                    ExtensionField::Fec2V1(Fec2V1Value {
                        major_version,
                        minor_version,
                        startup_id,
                    }),
                ))
            }
            _ => Err(nom::Err::Error(Error::new(input, ErrorKind::Switch))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_field_wire_format_emit() {
        let ext = ExtensionField::Fec2V1(Fec2V1Value {
            major_version: 2,
            minor_version: 3,
            startup_id: 0x123456789ABCDEF0,
        });

        let mut buffer = vec![0u8; ext.length() as usize];
        ext.emit_bytes(&mut buffer);

        let expected = [
            0xFE, 0xC2, // Field Type (0xFEC2)
            0x00, 0x10, // Length (16 octets)
            0x01, // ext_version (1)
            0x02, // major_version (2)
            0x03, // minor_version (3)
            0x00, // reserved (0)
            0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, // startup_id
        ];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn extension_field_wire_format_parse() {
        let wire_bytes = [
            0xFE, 0xC2, // Field Type (0xFEC2)
            0x00, 0x10, // Length (16 octets)
            0x01, // ext_version (1)
            0x02, // major_version (2)
            0x03, // minor_version (3)
            0x00, // reserved (0)
            0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, // startup_id
        ];

        let (leftover, parsed) = ExtensionField::parse_from_bytes(&wire_bytes).unwrap();
        assert!(leftover.is_empty());

        match parsed {
            ExtensionField::Fec2V1(fec2value) => {
                assert_eq!(fec2value.major_version, 2);
                assert_eq!(fec2value.minor_version, 3);
                assert_eq!(fec2value.startup_id, 0x123456789ABCDEF0);
            }
        }
    }

    #[test]
    fn extension_field_methods() {
        let ext = ExtensionField::Fec2V1(Fec2V1Value {
            major_version: 1,
            minor_version: 2,
            startup_id: 0xFFC0FFEE,
        });

        assert_eq!(ext.field_type(), 0xFEC2);
        assert_eq!(ext.length(), 16);
    }

    #[test]
    fn parse_unknown_field_type_fails() {
        let wire_bytes = [
            0xFF, 0xFF, // unknown Field Type
            0x00, 0x08, // Length (8 octets)
            0x01, 0x02, 0x03, 0x04, // payload
        ];

        let result = ExtensionField::parse_from_bytes(&wire_bytes);
        assert!(result.is_err());
    }

    #[test]
    fn extension_field_roundtrip() {
        let original = ExtensionField::Fec2V1(Fec2V1Value {
            major_version: 1,
            minor_version: 2,
            startup_id: 0xDEADBEEFCAFEBABE,
        });

        // Serialize
        let mut buffer = vec![0u8; original.length() as usize];
        original.emit_bytes(&mut buffer);

        // Parse back
        let (leftover, parsed) = ExtensionField::parse_from_bytes(&buffer).unwrap();
        assert!(leftover.is_empty());
        assert_eq!(original, parsed);
    }
}
