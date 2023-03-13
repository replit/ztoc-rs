use flatbuffers::ForwardsUOffset;

use crate::ztoc_flatbuffers::ztoc::{FileMetadata, FileMetadataArgs, TOCArgs, Ztoc, ZtocArgs, TOC};

pub fn encode_ztoc(ztoc: &crate::ztoc::ZToc) -> Vec<u8> {
    let mut builder = flatbuffers::FlatBufferBuilder::with_capacity(1024);
    let version = builder.create_string(&ztoc.version);
    let build_tool_identifier = builder.create_string(&ztoc.build_tool_identifier);

    builder.start_vector::<ForwardsUOffset<FileMetadata>>(ztoc.toc.metadata.len());
    for entry in &ztoc.toc.metadata {
        let name =
            builder.create_string(&entry.name.to_str().expect("unexpected non-UTF 8 encoding"));
        let linkname = entry.link_name.as_ref().map(|link_name| {
            builder.create_string(link_name.to_str().expect("unexpected non-UTF 8 encoding"))
        });
        let uname = entry
            .uname
            .as_ref()
            .map(|uname| builder.create_string(uname));
        let gname = entry
            .gname
            .as_ref()
            .map(|gname| builder.create_string(gname));
        let entry = FileMetadata::create(
            &mut builder,
            &FileMetadataArgs {
                name: Some(name),
                type_: todo!(),
                uncompressed_offset: entry.uncompressed_offset.0 as i64,
                uncompressed_size: entry.uncompressed_size.0 as i64,
                linkname,
                mode: entry.mode as i64,
                uid: entry.uid as u32,
                gid: entry.gid as u32,
                uname,
                gname,
                mod_time: todo!(),
                devmajor: entry.dev_minor.unwrap_or_default() as i64,
                devminor: entry.dev_major.unwrap_or_default() as i64,
                xattrs: todo!(),
            },
        );
        builder.push(entry);
    }

    let metadata = builder.end_vector(ztoc.toc.metadata.len());
    let toc = TOC::create(
        &mut builder,
        &TOCArgs {
            metadata: Some(metadata),
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
            compression_info: todo!(),
        },
    );
    builder.finish(ztoc, None);

    builder.finished_data().to_vec()
}
