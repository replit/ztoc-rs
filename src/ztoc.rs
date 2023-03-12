use std::{
    collections::HashMap,
    io::{self, Read, Result},
    path::PathBuf,
    str::Utf8Error,
};

use chrono::NaiveDateTime;
use tar::Archive;

#[derive(Debug)]
pub struct Toc {
    pub metadata: Vec<FileMetadata>,
}

#[derive(Debug)]
pub struct CompressionOffset(u64);

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

fn map_utf8_error(_: Utf8Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid UTF-8")
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

#[cfg(test)]
mod test {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn test_generate_zinfo() {
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
}
