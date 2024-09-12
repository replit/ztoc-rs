use std::{
    collections::HashMap,
    io::{self, Read, Result},
    path::PathBuf,
    str::Utf8Error,
};

use chrono::{DateTime, NaiveDateTime};
use tar::Archive;

use crate::zinfo::{GzipZInfoDecompressor, ZInfo};

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
        let span_size = 1 << 22; // 4MiB
        let mut decompressor = GzipZInfoDecompressor::new(reader, span_size)?;
        let toc = generate_tar_metadata(&mut decompressor)?;
        // Ensure we read the rest.
        let mut buf = [0u8; 1 << 10];
        while decompressor.read(&mut buf)? > 0 {}
        let zinfo = decompressor.into_zinfo();

        Ok(ZToc {
            version: String::from("0.9"),
            build_tool_identifier: String::from("Replit SOCI v0.1"),
            compressed_achrive_size: CompressionOffset(zinfo.total_in as u64),
            uncompressed_archive_size: CompressionOffset(zinfo.total_out as u64),
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

impl From<ZInfo> for CompressionInfo {
    fn from(zinfo: ZInfo) -> Self {
        let mut checkpoints = Vec::new();

        checkpoints.extend_from_slice(&(zinfo.checkpoints.len() as u32).to_le_bytes());
        checkpoints.extend_from_slice(&(zinfo.span_size as u64).to_le_bytes());

        for span in &zinfo.checkpoints {
            checkpoints.extend_from_slice(&span.r#in.to_le_bytes());
            checkpoints.extend_from_slice(&span.out.to_le_bytes());
            checkpoints.push(span.bits);
            checkpoints.extend_from_slice(&span.window);
        }

        CompressionInfo {
            max_span_id: zinfo.checkpoints.len() - 1,
            span_digests: zinfo.span_digests,
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
            mod_time: DateTime::from_timestamp(entry.header().mtime()? as i64, 0)
                .ok_or(io::Error::new(io::ErrorKind::InvalidData, "invalid mtime"))?
                .naive_utc(),
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
    }
}
