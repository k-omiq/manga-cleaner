//! The CBZ container: the exported pages, in order, inside one zip.
//!
//! A CBZ is not a picture format. It is a zip archive of picture files with a
//! different extension, which is why it sits here beside the encoders rather
//! than among them: nothing in this module encodes a pixel. It takes files that
//! [`super::export_page`] already produced and stores them.
//!
//! ## Why the entries are stored rather than deflated
//!
//! Every file that goes in is a PNG or a TIFF, and both are already compressed.
//! Deflating a PNG a second time costs the whole archive's worth of CPU to
//! shrink it by a fraction of a percent, and it is the standard practice for
//! comic archives to store rather than compress for exactly that reason. Method
//! 0 also has no encoder to be wrong about, which for a format written by hand
//! here matters more than the bytes.
//!
//! ## The one place a page's encoded file is held whole
//!
//! Rule 8 streams every other export: [`super::export_page_to`] encodes straight into
//! the sink, and the stitched writer holds one page's *pixels* and no file at
//! all. This path cannot. A zip entry's local header carries the entry's CRC-32
//! and its length, both of which are facts about the finished file, and a
//! stored entry has to state them **before** its data. So one page's encoded
//! file is buffered, added, and dropped before the next page is read - one page
//! resident, which is the unit the whole memory budget is stated in, and never
//! the chapter.
//!
//! ## The ceiling
//!
//! This writes the original zip structures and not Zip64, so an archive is
//! bounded at 4 GiB and at 65 535 entries. A 200-page chapter of 4 MB pages is
//! 800 MB, so the bound is far from a chapter; it is checked rather than assumed
//! because the failure it prevents - a truncated offset silently naming the
//! wrong entry - produces an archive that opens and shows the wrong pages.

use std::io::Write;

use super::ExportError;

/// A zip archive being written, one stored entry at a time.
pub struct Cbz<W: Write> {
    sink: W,
    /// Bytes written so far, which is also where the next local header starts.
    offset: u64,
    entries: Vec<Entry>,
}

/// What the central directory has to say about one entry, kept until
/// [`Cbz::finish`] writes it.
struct Entry {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
}

/// 1980-01-01 00:00, the earliest timestamp the DOS fields can hold.
///
/// Fixed rather than taken from the clock: two exports of the same chapter
/// should produce the same archive, and a modification time is the only field
/// here that would otherwise differ. The byte-for-byte passthrough is a
/// statement about the pages, and an archive that changes every time it is
/// written would make it unverifiable through the container.
const DOS_DATE: u16 = 0x0021;
const DOS_TIME: u16 = 0;

/// Stored, and the version that first understood it.
const METHOD_STORED: u16 = 0;
const VERSION_NEEDED: u16 = 10;
/// Zip 2.0, MS-DOS. The high byte is the originating filesystem and zero - DOS -
/// is what makes the external attributes below mean nothing rather than mean
/// a Unix mode nobody set.
const VERSION_MADE_BY: u16 = 0x0014;

const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const END_SIGNATURE: u32 = 0x0605_4b50;

impl<W: Write> Cbz<W> {
    pub fn new(sink: W) -> Cbz<W> {
        Cbz { sink, offset: 0, entries: Vec::new() }
    }

    /// How many entries have been added.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Store one file under `name`.
    ///
    /// `name` is a file name and not a path: a zip entry may carry directory
    /// components, and an archive whose entries escape their own directory is
    /// the oldest trick there is against whatever unpacks it. An export has no
    /// reason to write one, so a name that could be one is refused rather than
    /// sanitised, which would silently rename a page.
    pub fn add(&mut self, name: &str, bytes: &[u8]) -> Result<(), ExportError> {
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(ExportError::Sink(format!("{name}: not a file name a zip entry may carry")));
        }
        let size = u32::try_from(bytes.len())
            .map_err(|_| ExportError::Sink(format!("{name}: larger than a zip entry may be")))?;
        let offset = u32::try_from(self.offset).map_err(|_| {
            ExportError::Sink("the archive passed 4 GiB, which this writer cannot address".into())
        })?;
        if self.entries.len() == u16::MAX as usize {
            return Err(ExportError::Sink(
                "the archive passed 65 535 entries, which this writer cannot address".into(),
            ));
        }
        let crc = crc32fast::hash(bytes);

        let mut header = Vec::with_capacity(30 + name.len());
        put_u32(&mut header, LOCAL_SIGNATURE);
        put_u16(&mut header, VERSION_NEEDED);
        put_u16(&mut header, 0); // general purpose flags
        put_u16(&mut header, METHOD_STORED);
        put_u16(&mut header, DOS_TIME);
        put_u16(&mut header, DOS_DATE);
        put_u32(&mut header, crc);
        put_u32(&mut header, size); // compressed, and stored means the two agree
        put_u32(&mut header, size);
        put_u16(&mut header, name.len() as u16);
        put_u16(&mut header, 0); // extra field
        header.extend_from_slice(name.as_bytes());

        self.sink.write_all(&header)?;
        self.sink.write_all(bytes)?;
        self.offset += header.len() as u64 + bytes.len() as u64;
        self.entries.push(Entry { name: name.to_owned(), crc, size, offset });
        Ok(())
    }

    /// Write the central directory and the end record. The archive is not a
    /// readable zip until this has run - which is why it consumes the writer.
    ///
    /// Returns the archive's length in bytes.
    pub fn finish(mut self) -> Result<u64, ExportError> {
        let directory_at = self.offset;
        let mut directory = Vec::new();
        for entry in &self.entries {
            put_u32(&mut directory, CENTRAL_SIGNATURE);
            put_u16(&mut directory, VERSION_MADE_BY);
            put_u16(&mut directory, VERSION_NEEDED);
            put_u16(&mut directory, 0);
            put_u16(&mut directory, METHOD_STORED);
            put_u16(&mut directory, DOS_TIME);
            put_u16(&mut directory, DOS_DATE);
            put_u32(&mut directory, entry.crc);
            put_u32(&mut directory, entry.size);
            put_u32(&mut directory, entry.size);
            put_u16(&mut directory, entry.name.len() as u16);
            put_u16(&mut directory, 0); // extra
            put_u16(&mut directory, 0); // comment
            put_u16(&mut directory, 0); // disk number
            put_u16(&mut directory, 0); // internal attributes
            put_u32(&mut directory, 0); // external attributes
            put_u32(&mut directory, entry.offset);
            directory.extend_from_slice(entry.name.as_bytes());
        }

        let directory_size = u32::try_from(directory.len())
            .map_err(|_| ExportError::Sink("the archive's directory passed 4 GiB".into()))?;
        let directory_offset = u32::try_from(directory_at)
            .map_err(|_| ExportError::Sink("the archive passed 4 GiB".into()))?;

        let mut end = Vec::with_capacity(22);
        put_u32(&mut end, END_SIGNATURE);
        put_u16(&mut end, 0); // this disk
        put_u16(&mut end, 0); // the disk the directory starts on
        put_u16(&mut end, self.entries.len() as u16);
        put_u16(&mut end, self.entries.len() as u16);
        put_u32(&mut end, directory_size);
        put_u32(&mut end, directory_offset);
        put_u16(&mut end, 0); // archive comment

        self.sink.write_all(&directory)?;
        self.sink.write_all(&end)?;
        self.sink.flush()?;
        Ok(directory_at + directory.len() as u64 + end.len() as u64)
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads an archive back the way an unpacker does - from the end record to
    /// the central directory to each local header - rather than from the writer's
    /// own bookkeeping. A writer checked against its own `entries` list would
    /// pass with every offset wrong by the same amount.
    fn entries_of(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
        let end = archive.len() - 22;
        assert_eq!(u32::from_le_bytes(archive[end..end + 4].try_into().unwrap()), END_SIGNATURE);
        let count = u16::from_le_bytes(archive[end + 10..end + 12].try_into().unwrap()) as usize;
        let mut at = u32::from_le_bytes(archive[end + 16..end + 20].try_into().unwrap()) as usize;

        let mut out = Vec::new();
        for _ in 0..count {
            assert_eq!(u32::from_le_bytes(archive[at..at + 4].try_into().unwrap()), CENTRAL_SIGNATURE);
            let crc = u32::from_le_bytes(archive[at + 16..at + 20].try_into().unwrap());
            let size = u32::from_le_bytes(archive[at + 24..at + 28].try_into().unwrap()) as usize;
            let name_len = u16::from_le_bytes(archive[at + 28..at + 30].try_into().unwrap()) as usize;
            let local = u32::from_le_bytes(archive[at + 42..at + 46].try_into().unwrap()) as usize;
            let name = String::from_utf8(archive[at + 46..at + 46 + name_len].to_vec()).unwrap();
            at += 46 + name_len;

            assert_eq!(
                u32::from_le_bytes(archive[local..local + 4].try_into().unwrap()),
                LOCAL_SIGNATURE,
                "{name}: the central directory points at something that is not a local header"
            );
            let local_name_len =
                u16::from_le_bytes(archive[local + 26..local + 28].try_into().unwrap()) as usize;
            let extra = u16::from_le_bytes(archive[local + 28..local + 30].try_into().unwrap()) as usize;
            let data = local + 30 + local_name_len + extra;
            let bytes = archive[data..data + size].to_vec();
            assert_eq!(crc32fast::hash(&bytes), crc, "{name}: the stored CRC is not the data's");
            out.push((name, bytes));
        }
        out
    }

    #[test]
    fn the_pages_come_back_in_the_order_they_were_added() {
        let mut out = Vec::new();
        let mut cbz = Cbz::new(&mut out);
        cbz.add("001.png", b"the first page").unwrap();
        cbz.add("002.png", &[0u8; 4096]).unwrap();
        cbz.add("003.tiff", b"").unwrap();
        assert_eq!(cbz.len(), 3);
        let written = cbz.finish().unwrap();
        assert_eq!(written, out.len() as u64, "the reported length is not the archive's");

        let entries = entries_of(&out);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], ("001.png".into(), b"the first page".to_vec()));
        assert_eq!(entries[1].0, "002.png");
        assert_eq!(entries[1].1, vec![0u8; 4096]);
        assert_eq!(entries[2], ("003.tiff".into(), Vec::new()));
    }

    /// Two exports of the same pages produce the same archive, because the one
    /// field that could carry a clock does not.
    #[test]
    fn the_same_pages_produce_the_same_archive_twice() {
        let write = || {
            let mut out = Vec::new();
            let mut cbz = Cbz::new(&mut out);
            cbz.add("001.png", b"page").unwrap();
            cbz.finish().unwrap();
            out
        };
        assert_eq!(write(), write());
    }

    /// A name that could escape the directory it is unpacked into is refused
    /// rather than rewritten: an export has no reason to produce one, and a
    /// silent rename would lose the page's name.
    #[test]
    fn a_name_that_is_a_path_is_refused() {
        let mut out = Vec::new();
        let mut cbz = Cbz::new(&mut out);
        for name in ["", "a/b.png", "a\\b.png", "../escape.png"] {
            assert!(cbz.add(name, b"x").is_err(), "{name:?} was accepted as an entry name");
        }
        assert!(cbz.is_empty(), "a refused entry was recorded anyway");
        assert!(out.is_empty(), "a refused entry wrote bytes");
    }

    #[test]
    fn an_empty_archive_is_still_a_readable_zip() {
        let mut out = Vec::new();
        Cbz::new(&mut out).finish().unwrap();
        assert_eq!(out.len(), 22, "an empty archive is the end record alone");
        assert!(entries_of(&out).is_empty());
    }
}
