//! Links the kernel with its own script when it is built for bare metal.
//! Host builds, such as `cargo test`, link the usual way.

fn main() {
    println!("cargo::rerun-if-changed=link.ld");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        println!("cargo::rustc-link-arg-bins=-T{dir}/link.ld");
    }
}
