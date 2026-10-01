// `#[link(name = "oak_engine")]` (src/audio.rs) needs the linker to find
// the engine cdylib. Cargo builds it into the dependency dir but passes
// that dir only to rustc (`-L dependency=`), not to the system linker —
// forward it as a native search path.
use std::path::Path;

fn main() {
	// OUT_DIR = target/<profile>/build/<pkg>-<hash>/out
	let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set for build scripts");
	let profile_dir = Path::new(&out_dir)
		.ancestors()
		.nth(3)
		.expect("OUT_DIR is target/<profile>/build/<pkg>/out");
	println!(
		"cargo:rustc-link-search=native={}",
		profile_dir.join("deps").display()
	);
	println!("cargo:rustc-link-search=native={}", profile_dir.display());
}
