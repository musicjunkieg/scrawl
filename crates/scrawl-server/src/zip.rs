//! Reads the `.zip` an upload carries: stored entries (what `scrawl-web`'s `zip()` writes) and
//! deflated ones (what Finder and `zip` write), each checked against its CRC-32. No ZIP64, no
//! encryption, no multi-disk archives: a bundle is well under 4 GB.

use std::io::Read;

/// Why an archive was refused. `TooBig` is the caller's size limit; everything else is a broken
/// or unsupported archive.
#[derive(Debug, PartialEq, Eq)]
pub enum ZipError {
    TooBig,
    Bad(String),
}

const END: u32 = 0x0605_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, ZipError> {
    bytes
        .get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| bad("the archive is cut short"))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, ZipError> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| bad("the archive is cut short"))
}

fn bad(why: &str) -> ZipError {
    ZipError::Bad(why.to_owned())
}

/// The archive's files as `(name, bytes)`, in the central directory's order, directories left
/// out. Their unpacked sizes together may not pass `max_total` bytes: that's checked against the
/// sizes the archive declares before anything is unpacked, and again while inflating, so a small
/// upload can't unpack into something huge.
pub fn unzip(bytes: &[u8], max_total: u64) -> Result<Vec<(String, Vec<u8>)>, ZipError> {
    // The end record is the last 22 bytes, unless a comment (up to 64 KB) follows it.
    let floor = bytes.len().saturating_sub(22 + 0xFFFF);
    let end = (floor..=bytes.len().saturating_sub(22))
        .rev()
        .find(|&at| u32_at(bytes, at) == Ok(END))
        .ok_or_else(|| bad("this isn't a zip archive"))?;
    if u16_at(bytes, end + 4)? != 0 || u16_at(bytes, end + 6)? != 0 {
        return Err(bad("split archives aren't supported"));
    }
    let count = u16_at(bytes, end + 10)? as usize;
    let mut at = u32_at(bytes, end + 16)? as usize;

    let mut entries = Vec::with_capacity(count);
    let mut declared: u64 = 0;
    for _ in 0..count {
        if u32_at(bytes, at)? != CENTRAL {
            return Err(bad("the archive's directory is damaged"));
        }
        let flags = u16_at(bytes, at + 8)?;
        let method = u16_at(bytes, at + 10)?;
        let crc = u32_at(bytes, at + 16)?;
        let packed = u32_at(bytes, at + 20)?;
        let size = u32_at(bytes, at + 24)?;
        let name_len = u16_at(bytes, at + 28)? as usize;
        let extra_len = u16_at(bytes, at + 30)? as usize;
        let comment_len = u16_at(bytes, at + 32)? as usize;
        let local = u32_at(bytes, at + 42)? as usize;
        let name = bytes
            .get(at + 46..at + 46 + name_len)
            .ok_or_else(|| bad("the archive is cut short"))?;
        let name = String::from_utf8(name.to_vec()).map_err(|_| bad("a file name isn't UTF-8"))?;
        at += 46 + name_len + extra_len + comment_len;

        if flags & 1 != 0 {
            return Err(bad("encrypted archives aren't supported"));
        }
        if packed == u32::MAX || size == u32::MAX || local == u32::MAX as usize {
            return Err(ZipError::TooBig);
        }
        if name.ends_with('/') {
            continue;
        }
        declared += u64::from(size);
        if declared > max_total {
            return Err(ZipError::TooBig);
        }
        entries.push((name, method, crc, packed as usize, size as usize, local));
    }

    let mut files = Vec::with_capacity(entries.len());
    let mut total: u64 = 0;
    for (name, method, crc, packed, size, local) in entries {
        if u32_at(bytes, local)? != LOCAL {
            return Err(bad("the archive is damaged"));
        }
        let start =
            local + 30 + u16_at(bytes, local + 26)? as usize + u16_at(bytes, local + 28)? as usize;
        let data = bytes
            .get(start..start + packed)
            .ok_or_else(|| bad("the archive is cut short"))?;
        let unpacked = match method {
            0 => data.to_vec(),
            8 => {
                // One byte past the declared size shows a lie without unpacking the rest.
                // Deflate grows data at most about 1032 times, so a size the archive overstates
                // can't make this reserve more than its data could ever unpack to.
                let mut out = Vec::with_capacity(size.min(packed.saturating_mul(1032)));
                flate2::read::DeflateDecoder::new(data)
                    .take(size as u64 + 1)
                    .read_to_end(&mut out)
                    .map_err(|_| bad("a file in the archive is damaged"))?;
                out
            }
            _ => return Err(bad("the archive uses a compression Scrawl can't read")),
        };
        if unpacked.len() != size {
            return Err(bad("a file in the archive isn't the size it says"));
        }
        total += unpacked.len() as u64;
        if total > max_total {
            return Err(ZipError::TooBig);
        }
        let mut check = flate2::Crc::new();
        check.update(&unpacked);
        if check.sum() != crc {
            return Err(bad("a file in the archive is damaged"));
        }
        files.push((name, unpacked));
    }
    Ok(files)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A zip of `files`, stored (as `scrawl-web` writes) or deflated (as Finder does).
    pub(crate) fn zip(files: &[(&str, &[u8])], deflate: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, bytes) in files {
            let mut crc = flate2::Crc::new();
            crc.update(bytes);
            let data = if deflate {
                let mut encoder =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(bytes).unwrap();
                encoder.finish().unwrap()
            } else {
                bytes.to_vec()
            };
            let method: u16 = if deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            out.extend(LOCAL.to_le_bytes());
            out.extend([20, 0, 0, 0]);
            out.extend(method.to_le_bytes());
            out.extend([0; 4]);
            out.extend(crc.sum().to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((bytes.len() as u32).to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes());
            out.extend([0, 0]);
            out.extend(name.as_bytes());
            out.extend(&data);

            central.extend(CENTRAL.to_le_bytes());
            central.extend([20, 0, 20, 0, 0, 0]);
            central.extend(method.to_le_bytes());
            central.extend([0; 4]);
            central.extend(crc.sum().to_le_bytes());
            central.extend((data.len() as u32).to_le_bytes());
            central.extend((bytes.len() as u32).to_le_bytes());
            central.extend((name.len() as u16).to_le_bytes());
            central.extend([0; 12]);
            central.extend(offset.to_le_bytes());
            central.extend(name.as_bytes());
        }
        let central_at = out.len() as u32;
        out.extend(&central);
        out.extend(END.to_le_bytes());
        out.extend([0; 4]);
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((central.len() as u32).to_le_bytes());
        out.extend(central_at.to_le_bytes());
        out.extend([0, 0]);
        out
    }

    #[test]
    fn stored_and_deflated_files_come_back() {
        let files: [(&str, &[u8]); 2] = [
            ("a.scrawl/summary.md", b"hello"),
            ("a.scrawl/x.bin", &[7; 5000]),
        ];
        for deflate in [false, true] {
            let got = unzip(&zip(&files, deflate), 1 << 20).unwrap();
            assert_eq!(got.len(), 2);
            assert_eq!(
                got[0],
                ("a.scrawl/summary.md".to_owned(), b"hello".to_vec())
            );
            assert_eq!(got[1].1, vec![7; 5000]);
        }
    }

    #[test]
    fn more_than_the_limit_is_too_big_before_and_while_unpacking() {
        let big = vec![0u8; 10_000];
        let archive = zip(&[("a.scrawl/big", &big)], true);
        assert_eq!(unzip(&archive, 9_999), Err(ZipError::TooBig));
        // An archive that understates a file's size is caught while inflating, not trusted.
        let mut lying = archive.clone();
        let central = lying.len() - 22 - (46 + "a.scrawl/big".len());
        lying[central + 24..central + 28].copy_from_slice(&100u32.to_le_bytes());
        assert!(matches!(unzip(&lying, 1_000), Err(ZipError::Bad(_))));
    }

    #[test]
    fn damage_is_caught() {
        let mut archive = zip(&[("a.scrawl/summary.md", b"hello")], false);
        assert!(matches!(unzip(b"not a zip", 100), Err(ZipError::Bad(_))));
        let at = 30 + "a.scrawl/summary.md".len();
        archive[at] = b'j';
        assert!(matches!(unzip(&archive, 100), Err(ZipError::Bad(_))));
        assert!(matches!(
            unzip(&archive[..archive.len() - 30], 100),
            Err(ZipError::Bad(_))
        ));
    }
}
