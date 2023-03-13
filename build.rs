use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=src/flatbuffers/ztoc.fbs");
    flatc_rust::run(flatc_rust::Args {
        inputs: &[Path::new("src/flatbuffers/ztoc.fbs")],
        out_dir: Path::new("target/flatbuffers/"),
        ..Default::default()
    })
    .expect("flatc");
}
