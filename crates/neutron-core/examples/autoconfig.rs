//! Look up server settings for an address: `cargo run --example autoconfig -- you@example.org`
#[tokio::main]
async fn main() {
    let email = std::env::args().nth(1).expect("usage: autoconfig <email>");
    match neutron_core::providers::detect(&email) {
        Some(p) => println!("built-in profile: {}", p.name),
        None => match neutron_core::providers::autoconfig(&email).await {
            Ok(d) => println!("{d:#?}"),
            Err(e) => println!("error: {e}"),
        },
    }
}
