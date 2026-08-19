//! Run a cartridge for N frames and print bytes — our side of a two-implementation
//! measurement, matching `tools/sameboy-oracle`'s `peek` mode argument for argument.
//!
//! ```text
//! cargo run --release --example peek_probe -- <rom> <frames> <hex addr>...
//! ```
use terminalgb::gameboy::Gameboy;
use terminalgb::mode::Model;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 3 {
        eprintln!("usage: peek_probe <rom> <frames> <addr-hex>...   [GB_MODEL=…]");
        std::process::exit(2);
    }
    let rom = std::fs::read(&a[0]).expect("rom");
    let frames: u32 = a[1].parse().expect("frames");
    let model = std::env::var("GB_MODEL")
        .ok()
        .and_then(|m| Model::parse(&m))
        .unwrap_or(Model::Auto);
    let mut gb = Gameboy::new_with_model(rom, None, model);
    for _ in 0..frames {
        gb.frame();
    }
    let out: Vec<String> = a[2..]
        .iter()
        .map(|s| {
            let addr = u16::from_str_radix(s.trim_start_matches("0x"), 16).expect("addr");
            format!("{addr:04X}={:02X}", gb.peek(addr))
        })
        .collect();
    println!("{}", out.join(" "));
}
