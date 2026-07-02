#![forbid(unsafe_code)]

fn main() {
    println!("rustify {}", env!("CARGO_PKG_VERSION"));
}
