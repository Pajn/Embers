use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../docs/config-api")
                .canonicalize()
                .unwrap_or_else(|_| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/config-api")
                })
        });

    embers_client::scripting::generate_config_api_docs(&output_dir)?;
    embers_client::scripting::build_mdbook(&output_dir)?;
    println!("wrote config API docs to {}", output_dir.display());
    Ok(())
}
