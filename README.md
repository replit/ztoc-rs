# ztoc-rs

`ztoc-rs` generates gzip table of contents (ztoc) from `.tar.gz` files. This is for use with the
[soci-snapshotter](https://github.com/awslabs/soci-snapshotter) which allows lazily pulling OCI images without
modifying images themselves.

This is a reimplementation of ztoc generation from soci-snapshotter that does not require multiple intermediate
temp files.

