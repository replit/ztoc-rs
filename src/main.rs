mod zinfo;
mod ztoc;

#[allow(non_snake_case)]
#[path = "../target/flatbuffers/ztoc_generated.rs"]
pub mod ztoc_flatbuffers;

fn main() {
    println!("Hello, world!");
}
