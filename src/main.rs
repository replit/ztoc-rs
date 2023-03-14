use std::io::{self, Cursor, Read, Write};

use sha2::{Digest, Sha256};

mod encode;
mod zinfo;
mod ztoc;

#[allow(non_snake_case, unused_imports, clippy::all)]
#[path = "../target/flatbuffers/ztoc_generated.rs"]
pub mod ztoc_flatbuffers;

struct Tee<R, W> {
    reader: R,
    writer: W,
}

impl<R, W> Tee<R, W> {
    fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }
}

impl<R, W> Read for Tee<R, W>
where
    R: Read,
    W: Write,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.reader.read(buf)?;
        self.writer.write_all(&buf[..read])?;
        Ok(read)
    }
}

fn main() -> io::Result<()> {
    let mut hasher = Sha256::new();
    let ztoc = ztoc::ZToc::new(Tee::new(std::io::stdin(), &mut hasher))?;
    let encoded = encode::encode_ztoc(&ztoc);
    std::io::copy(&mut Cursor::new(encoded), &mut std::io::stdout())?;
    eprintln!("Digest: sha256:{:x}", hasher.finalize());
    Ok(())
}
