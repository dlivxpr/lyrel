//! Embeds which sources this fixture package was built from.
//!
//! The desktop consumer tests hash the same files at run time. A binary from
//! another revision therefore cannot silently serve the current protocol: the
//! tests rebuild it before spawning a daemon or PTY producer.

mod identity {
    include!("src/fixture_identity.rs");
}

fn main() {
    let manifest = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("Cargo provides the manifest directory"),
    );
    let root = manifest.join("../..");
    let identity = identity::fixture_source_identity(&root)
        .unwrap_or_else(|error| panic!("graphics fixture identity: {error}"));
    println!("cargo:rustc-env=LYREL_GRAPHICS_FIXTURE_ID={identity}");
    // The embedded value must change with every input `fixture_identity.rs`
    // hashes. Cargo re-runs this script only for the paths named here.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    for relative in identity::FIXTURE_IDENTITY_CRATES {
        println!("cargo:rerun-if-changed=../../{relative}");
    }
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=../../assets/windows/conpty/manifest.json");
}
