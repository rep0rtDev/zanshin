use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    env::set_var("PROTOC", protoc);
    let include = protoc_bin_vendored::include_path()?;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    let out = PathBuf::from(env::var("OUT_DIR")?);

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .file_descriptor_set_path(out.join("zanshin_descriptor.bin"))
        .compile_protos(
            &[root.join("zanshin/api/v1/zanshin.proto")],
            &[root, include],
        )?;

    println!("cargo:rerun-if-changed=../../proto");
    Ok(())
}
