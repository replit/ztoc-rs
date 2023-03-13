use std::{
    collections::HashMap,
    io::{self, Read, Result},
    path::PathBuf,
    str::Utf8Error,
};

use chrono::NaiveDateTime;
use sha2::{Digest, Sha256};
use tar::Archive;

use crate::zinfo::{GzipZInfoDecompressor, GzipZinfo};

#[derive(Debug)]
pub struct CompressionOffset(pub u64);

#[derive(Debug)]
pub struct ZToc {
    pub version: String,
    pub build_tool_identifier: String,
    pub compressed_achrive_size: CompressionOffset,
    pub uncompressed_archive_size: CompressionOffset,
    pub toc: Toc,
    pub compression_info: CompressionInfo,
}

impl ZToc {
    pub fn new<R>(reader: R) -> Result<ZToc>
    where
        R: Read,
    {
        // TODO: Make this configurable.
        let span_size = 1 << 2; // 4MiB
        let compressed_length_reader = LengthReader::new(reader);
        let decompressor = GzipZInfoDecompressor::new(compressed_length_reader, span_size)?;
        let mut uncompressed_length_reader = LengthReader::new(decompressor);
        let toc = generate_tar_metadata(&mut uncompressed_length_reader)?;

        // Unwrap all the readers so we can get their results...
        // TODO: There might be a better way to accomplish this.
        let uncompressed_archive_size =
            CompressionOffset(uncompressed_length_reader.length() as u64);
        let decompressor = uncompressed_length_reader.to_inner();
        let (zinfo, compressed_length_reader) = decompressor.to_zinfo();
        let compressed_achrive_size = CompressionOffset(compressed_length_reader.length() as u64);

        Ok(ZToc {
            version: String::from("0.9"),
            build_tool_identifier: String::from("Replit SOCI v0.1"),
            compressed_achrive_size,
            uncompressed_archive_size,
            toc,
            compression_info: zinfo.into(),
        })
    }
}

#[derive(Debug)]
pub struct CompressionInfo {
    pub max_span_id: usize,
    pub span_digests: Vec<String>,
    pub checkpoints: Vec<u8>,
}

impl From<GzipZinfo> for CompressionInfo {
    fn from(zinfo: GzipZinfo) -> Self {
        let mut span_digests = Vec::with_capacity(zinfo.checkpoints.len());
        let mut checkpoints = Vec::new();

        for span in &zinfo.checkpoints {
            let mut hasher = Sha256::new();
            hasher.update(&span.window);
            span_digests.push(format!("sha256:{:x}", hasher.finalize()));

            // TODO: Is this the right endianness?
            checkpoints.extend_from_slice(&span.r#in.to_be_bytes());
            checkpoints.extend_from_slice(&span.out.to_be_bytes());
            checkpoints.push(span.bits);
            checkpoints.extend_from_slice(&span.window);
        }

        CompressionInfo {
            max_span_id: zinfo.checkpoints.len() - 1,
            span_digests,
            checkpoints,
        }
    }
}

#[derive(Debug)]
pub struct Toc {
    pub metadata: Vec<FileMetadata>,
}

#[derive(Debug)]
pub struct FileMetadata {
    pub name: PathBuf,
    pub r#type: tar::EntryType,
    pub uncompressed_offset: CompressionOffset,
    pub uncompressed_size: CompressionOffset,
    pub link_name: Option<PathBuf>,
    pub mode: u32,
    pub uid: u64,
    pub gid: u64,
    pub uname: Option<String>,
    pub gname: Option<String>,
    pub mod_time: NaiveDateTime,
    pub dev_major: Option<u32>,
    pub dev_minor: Option<u32>,
    pub x_attrs: HashMap<String, String>,
}

impl<R: Read> TryFrom<tar::Entry<'_, R>> for FileMetadata {
    type Error = io::Error;

    fn try_from(mut entry: tar::Entry<R>) -> std::result::Result<Self, Self::Error> {
        let mut meta = FileMetadata {
            name: entry.path()?.into(),
            r#type: entry.header().entry_type(),
            // TODO: Should this be file or header position?
            uncompressed_offset: CompressionOffset(entry.raw_file_position()),
            uncompressed_size: CompressionOffset(entry.size()),
            link_name: entry.link_name()?.map(Into::into),
            mode: entry.header().mode()?,
            uid: entry.header().uid()?,
            gid: entry.header().gid()?,
            uname: entry
                .header()
                .username()
                .map_err(map_utf8_error)?
                .map(Into::into),
            gname: entry
                .header()
                .groupname()
                .map_err(map_utf8_error)?
                .map(Into::into),
            mod_time: NaiveDateTime::from_timestamp_opt(entry.header().mtime()? as i64, 0)
                .ok_or(io::Error::new(io::ErrorKind::InvalidData, "invalid mtime"))?,
            dev_major: None,
            dev_minor: None,
            // lol maybe I went too far...
            x_attrs: entry
                .pax_extensions()?
                .map(|exts| {
                    exts.map(|ext| {
                        ext.and_then(|ext| {
                            Ok((
                                ext.key().map_err(map_utf8_error)?.to_string(),
                                ext.value().map_err(map_utf8_error)?.to_string(),
                            ))
                        })
                    })
                    .collect::<Result<_>>()
                })
                .transpose()?
                .unwrap_or_default(),
        };
        if matches!(
            entry.header().entry_type(),
            tar::EntryType::Block | tar::EntryType::Char
        ) {
            meta.dev_major = entry.header().device_major()?;
            meta.dev_minor = entry.header().device_minor()?;
        }
        Ok(meta)
    }
}

fn generate_tar_metadata<R: Read>(reader: &mut R) -> Result<Toc> {
    let mut archive = Archive::new(reader);
    archive.set_unpack_xattrs(true);
    archive.set_preserve_permissions(true);
    let metadata = archive
        .entries()?
        .map(|entry| entry.and_then(TryInto::try_into))
        .collect::<Result<_>>()?;
    Ok(Toc { metadata })
}

/// A wrapper around a reader which records the total number of bytes read.
struct LengthReader<R> {
    reader: R,
    length: usize,
}

impl<R> LengthReader<R> {
    fn new(reader: R) -> Self {
        Self { reader, length: 0 }
    }
    fn length(&self) -> usize {
        self.length
    }
    fn to_inner(self) -> R {
        self.reader
    }
}

impl<R> Read for LengthReader<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let read = self.reader.read(buf)?;
        self.length += read;
        Ok(read)
    }
}

fn map_utf8_error(_: Utf8Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid UTF-8")
}

#[cfg(test)]
mod test {
    use std::io::Cursor;

    use crate::zinfo::GzipZInfoDecompressor;

    use super::*;

    #[test]
    fn test_generate_ztoc() {
        let mut reader = Cursor::new(include_bytes!("testdata/test.tar"));
        let meta = generate_tar_metadata(&mut reader).expect("failed to generate tar metadata");
        assert_eq!(
            vec!["src/", "src/zinfo.rs", "src/main.rs", "src/testdata/",],
            meta.metadata
                .iter()
                .map(|m| m.name.to_str().unwrap())
                .collect::<Vec<&str>>(),
        );
    }

    #[test]
    fn test_generate_full() {
        let reader = Cursor::new(include_bytes!("testdata/test.tar.gz"));
        let mut decompressor = GzipZInfoDecompressor::new(reader, 4096).unwrap();
        let meta =
            generate_tar_metadata(&mut decompressor).expect("failed to generate tar metadata");
        assert_eq!(
            vec!["src/", "src/zinfo.rs", "src/main.rs", "src/testdata/",],
            meta.metadata
                .iter()
                .map(|m| m.name.to_str().unwrap())
                .collect::<Vec<&str>>(),
        );
        dbg!(decompressor.to_zinfo());
    }
}
