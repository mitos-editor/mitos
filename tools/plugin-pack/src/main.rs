use std::{env, fs, io::Read, path::Path};

use anyhow::{bail, Context};

const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;

fn package(input: &Path, output: &Path) -> anyhow::Result<()> {
    if input == output {
        bail!("choose a separate component output path");
    }
    let mut bytes = Vec::new();
    fs::File::open(input)
        .with_context(|| format!("open {}", input.display()))?
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        bail!("plugin source exceeds 16 MiB");
    }
    let component = wit_component::ComponentEncoder::default()
        .module(&bytes)
        .context("expected a core WASM module with SDK component metadata")?
        .validate(true)
        .encode()
        .context("package the SDK module without WASI adapters")?;
    if component.len() as u64 > MAX_SOURCE_BYTES {
        bail!("packaged component exceeds 16 MiB");
    }
    fs::write(output, &component).with_context(|| format!("write {}", output.display()))?;
    println!("{} ({} bytes)", output.display(), component.len());
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() != 2 {
        bail!("usage: mitos-plugin-pack INPUT.wasm OUTPUT.component.wasm");
    }
    package(Path::new(&args[0]), Path::new(&args[1]))
}
