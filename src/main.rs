//! Nenya distributed rate limiting server

#[cfg(not(feature = "server"))]
compile_error!(
    "The nenya binary requires the 'server' feature.\n\
     Install with: cargo install nenya\n\
     Build with: cargo build --features server"
);

#[cfg(feature = "server")]
fn main() {
    println!("Nenya distributed rate limiting server");
    println!("Version: {}", env!("CARGO_PKG_VERSION"));
    println!("\nPlaceholder - server implementation in Milestone 1");
    println!("See docs/roadmap.md for development plan");
}
