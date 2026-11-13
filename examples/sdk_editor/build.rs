//! Locate the Quercus engine dylib copied next to the binaries by the SDK
//! build script. The binary and its tests run from `target/<profile>` and
//! `target/<profile>/deps` respectively.

fn main() {
	match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
		Ok("macos") => {
			println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
			println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/..");
		}
		Ok("linux") => {
			println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
			println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/..");
		}
		_ => {}
	}
}
