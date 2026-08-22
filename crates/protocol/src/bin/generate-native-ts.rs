use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let Some(output_dir) = std::env::args_os().nth(1) else {
        eprintln!("usage: generate-native-ts <output-dir>");
        std::process::exit(2);
    };

    devo_protocol::acp_ts::write_native_typescript_dir(PathBuf::from(output_dir))
}
