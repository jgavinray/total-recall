fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // sqlite-vec is now provided by the sqlite-vec crate (crates.io),
    // which bundles and compiles the C extension automatically.
    // No manual vendor/sqlite-vec clone or compilation required.
}
