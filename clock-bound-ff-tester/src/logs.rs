//! Shared framework for streaming log files.
//!
//! Both the replay tooling and the log-fixing tooling stream log files line by
//! line, with different parsing on top. This module provides the common pieces:
//!
//! - [`open_log`]: open a possibly gzip-compressed log file as a buffered reader
//! - [`order`]: ordering signals, anomaly classification and run summaries

pub mod order;

use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use flate2::read::MultiGzDecoder;

/// Magic number identifying a gzip compressed file
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Open a log file as a buffered reader, transparently decoding gzip.
///
/// Compression is detected from the file content (magic number), not the file
/// extension. Concatenated gzip members (e.g. rotated logs joined with `cat`) are
/// decoded in full.
///
/// The plain and gzip branches produce different reader types, unified behind a
/// `Box<dyn BufRead>`: one allocation per file and one virtual call per line,
/// negligible next to per-line parsing.
///
/// # Errors
/// Returns an error if the file cannot be opened or read.
pub fn open_log(file_path: impl AsRef<Path>) -> anyhow::Result<Box<dyn BufRead>> {
    let mut reader = BufReader::new(File::open(&file_path)?);

    // Peek at the first bytes to detect compression: fill_buf() reads at most one BufReader
    // buffer's worth (8 KiB by default, one read syscall) and returns a borrowed slice without
    // consuming it, so the bytes remain for the actual parsing. An empty or 1-byte file
    // yields a short buffer, which simply fails the match.
    let is_gzip = reader.fill_buf()?.starts_with(&GZIP_MAGIC);
    Ok(if is_gzip {
        Box::new(BufReader::new(MultiGzDecoder::new(reader)))
    } else {
        Box::new(reader)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn open_log_reads_plain_and_gzip_identically() {
        use flate2::{Compression, write::GzEncoder};

        let content = "line one\nline two\n";

        let plain = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(plain.path(), content).unwrap();

        // Two concatenated gzip members: the full stream must be decoded, not
        // just the first member (MultiGzDecoder, unlike GzDecoder).
        let (first, second) = content.as_bytes().split_at(content.len() / 2);
        let mut compressed = Vec::new();
        for part in [first, second] {
            let mut encoder = GzEncoder::new(&mut compressed, Compression::default());
            encoder.write_all(part).unwrap();
            encoder.finish().unwrap();
        }
        let gz = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(gz.path(), &compressed).unwrap();

        let read_all = |path| {
            open_log(path)
                .unwrap()
                .lines()
                .collect::<std::io::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(read_all(plain.path()), vec!["line one", "line two"]);
        assert_eq!(read_all(gz.path()), vec!["line one", "line two"]);
    }
}
