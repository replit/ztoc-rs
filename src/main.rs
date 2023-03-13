use std::io::{self, Cursor};

mod encode;
mod zinfo;
mod ztoc;

#[allow(non_snake_case, unused_imports, clippy::all)]
#[path = "../target/flatbuffers/ztoc_generated.rs"]
pub mod ztoc_flatbuffers;

fn main() -> io::Result<()> {
    let ztoc = ztoc::ZToc::new(std::io::stdin())?;
    let encoded = encode::encode_ztoc(&ztoc);
    std::io::copy(&mut Cursor::new(encoded), &mut std::io::stdout())?;
    Ok(())
}
