use chrono::Utc;
use tar::EntryType;

use crate::ztoc_flatbuffers::ztoc::{
    CompressionAlgorithm, CompressionInfo, CompressionInfoArgs, FileMetadata, FileMetadataArgs,
    TOCArgs, Xattr, XattrArgs, Ztoc, ZtocArgs, TOC,
};

fn entry_to_string(entry: &EntryType) -> &'static str {
    match entry {
        EntryType::Regular => "reg",
        EntryType::Link => "hardlink",
        EntryType::Symlink => "symlink",
        EntryType::Char => "char",
        EntryType::Block => "block",
        EntryType::Directory => "dir",
        EntryType::Fifo => "fifo",
        _ => unimplemented!("Unexpected entry type {:?}", entry),
    }
}

pub fn encode_ztoc(ztoc: &crate::ztoc::ZToc) -> Vec<u8> {
    let mut builder = flatbuffers::FlatBufferBuilder::with_capacity(1024);
    let version = builder.create_string(&ztoc.version);
    let build_tool_identifier = builder.create_string(&ztoc.build_tool_identifier);

    let mut metadata = Vec::with_capacity(ztoc.toc.metadata.len());
    for entry in &ztoc.toc.metadata {
        let name =
            builder.create_string(entry.name.to_str().expect("unexpected non-UTF 8 encoding"));
        let linkname = builder.create_string(
            entry
                .link_name
                .as_ref()
                .map(|link| link.to_str().expect("unexpected non-UTF 8 encoding"))
                .unwrap_or_default(),
        );
        let uname = entry
            .uname
            .as_ref()
            .map(|uname| builder.create_string(uname));
        let gname = entry
            .gname
            .as_ref()
            .map(|gname| builder.create_string(gname));
        let type_ = builder.create_string(entry_to_string(&entry.r#type));
        let mod_time =
            builder.create_string(&entry.mod_time.and_local_timezone(Utc).unwrap().to_rfc3339());

        let mut xattrs = Vec::with_capacity(entry.x_attrs.len());
        for (key, value) in &entry.x_attrs {
            let key = builder.create_string(key);
            let value = builder.create_string(value);
            xattrs.push(Xattr::create(
                &mut builder,
                &XattrArgs {
                    key: Some(key),
                    value: Some(value),
                },
            ))
        }
        let xattrs = builder.create_vector(&xattrs);

        metadata.push(FileMetadata::create(
            &mut builder,
            &FileMetadataArgs {
                name: Some(name),
                type_: Some(type_),
                uncompressed_offset: entry.uncompressed_offset.0 as i64,
                uncompressed_size: entry.uncompressed_size.0 as i64,
                linkname: Some(linkname),
                mode: entry.mode as i64,
                uid: entry.uid as u32,
                gid: entry.gid as u32,
                uname,
                gname,
                mod_time: Some(mod_time),
                devmajor: entry.dev_minor.unwrap_or_default() as i64,
                devminor: entry.dev_major.unwrap_or_default() as i64,
                xattrs: Some(xattrs),
            },
        ));
    }

    let metadata = builder.create_vector(&metadata);
    let toc = TOC::create(
        &mut builder,
        &TOCArgs {
            metadata: Some(metadata),
        },
    );

    let span_digests = ztoc
        .compression_info
        .span_digests
        .iter()
        .map(|digest| builder.create_string(digest))
        .collect::<Vec<_>>();
    let span_digests = builder.create_vector(&span_digests);
    let checkpoints = builder.create_vector(&ztoc.compression_info.checkpoints);

    let compression_info = CompressionInfo::create(
        &mut builder,
        &CompressionInfoArgs {
            compression_algorithm: CompressionAlgorithm::Gzip,
            max_span_id: ztoc.compression_info.max_span_id as i32,
            span_digests: Some(span_digests),
            checkpoints: Some(checkpoints),
        },
    );

    let ztoc = Ztoc::create(
        &mut builder,
        &ZtocArgs {
            version: Some(version),
            build_tool_identifier: Some(build_tool_identifier),
            compressed_archive_size: ztoc.compressed_achrive_size.0 as i64,
            uncompressed_archive_size: ztoc.uncompressed_archive_size.0 as i64,
            toc: Some(toc),
            compression_info: Some(compression_info),
        },
    );
    builder.finish(ztoc, None);

    builder.finished_data().to_vec()
}

#[cfg(test)]
mod test {
    use std::fs::File;

    use chrono::DateTime;

    use crate::{ztoc::ZToc, ztoc_flatbuffers};

    use super::encode_ztoc;

    #[test]
    fn test_compare_soci_snapshotter() {
        let layer = File::open("./src/testdata/layer.tar.gz").unwrap();
        let ztoc = ZToc::new(layer).unwrap();
        let encoded = encode_ztoc(&ztoc);

        let decoded = ztoc_flatbuffers::ztoc::root_as_ztoc(&encoded).unwrap();
        let expected =
            ztoc_flatbuffers::ztoc::root_as_ztoc(include_bytes!("testdata/expected")).unwrap();

        assert_eq!(decoded.version(), expected.version());
        assert_eq!(
            decoded.compressed_archive_size(),
            expected.compressed_archive_size()
        );
        assert_eq!(
            decoded.uncompressed_archive_size(),
            expected.uncompressed_archive_size()
        );

        let decoded_compression_info = decoded.compression_info().unwrap();
        let expected_compression_info = expected.compression_info().unwrap();
        assert_eq!(
            decoded_compression_info.max_span_id(),
            expected_compression_info.max_span_id(),
        );
        assert_eq!(
            decoded_compression_info.checkpoints().unwrap().bytes(),
            expected_compression_info.checkpoints().unwrap().bytes(),
        );
        assert_eq!(
            decoded_compression_info.span_digests().unwrap().len(),
            expected_compression_info.span_digests().unwrap().len(),
        );
        for (i, (decoded_digest, expected_digest)) in decoded_compression_info
            .span_digests()
            .unwrap()
            .into_iter()
            .zip(
                expected_compression_info
                    .span_digests()
                    .unwrap()
                    .into_iter(),
            )
            .enumerate()
        {
            assert_eq!(decoded_digest, expected_digest, "index {}", i);
        }

        let decoded_toc = decoded.toc().unwrap();
        let expected_toc = expected.toc().unwrap();
        assert_eq!(
            decoded_toc.metadata().unwrap().len(),
            expected_toc.metadata().unwrap().len(),
        );
        for (decoded_entry, expected_entry) in decoded_toc
            .metadata()
            .unwrap()
            .into_iter()
            .zip(expected_toc.metadata().unwrap().into_iter())
        {
            let decoded_time =
                DateTime::parse_from_rfc3339(decoded_entry.mod_time().take().unwrap()).unwrap();
            let expected_time =
                DateTime::parse_from_rfc3339(expected_entry.mod_time().take().unwrap()).unwrap();
            assert_eq!(decoded_time, expected_time);
            assert_eq!(decoded_entry.name(), expected_entry.name());
            assert_eq!(decoded_entry.type_(), expected_entry.type_());
            assert_eq!(
                decoded_entry.uncompressed_offset(),
                expected_entry.uncompressed_offset()
            );
            assert_eq!(
                decoded_entry.uncompressed_size(),
                expected_entry.uncompressed_size()
            );
            assert_eq!(decoded_entry.linkname(), expected_entry.linkname());
            assert_eq!(decoded_entry.mode(), expected_entry.mode());
            assert_eq!(decoded_entry.uid(), expected_entry.uid());
            assert_eq!(decoded_entry.gid(), expected_entry.gid());
            assert_eq!(decoded_entry.uname(), expected_entry.uname());
            assert_eq!(decoded_entry.gname(), expected_entry.gname());
            assert_eq!(decoded_entry.devmajor(), expected_entry.devmajor());
            assert_eq!(decoded_entry.devminor(), expected_entry.devminor());
            assert_eq!(
                decoded_entry.xattrs().unwrap().bytes(),
                expected_entry.xattrs().unwrap().bytes()
            );
        }
    }
}
