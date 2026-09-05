fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV");
    if target_os.as_deref() == Ok("windows") && target_env.as_deref() == Ok("msvc") {
        // esaxx-rs is built with the static C++ runtime, while Rust uses the
        // dynamic MSVC runtime. Ignore esaxx-rs's conflicting default library.
        println!("cargo:rustc-link-arg=/NODEFAULTLIB:LIBCMT");
    }
}
