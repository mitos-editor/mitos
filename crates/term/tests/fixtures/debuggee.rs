//! A debuggee fixture that records its actual process inputs.
use std::{collections::BTreeMap, path::PathBuf};
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let output = PathBuf::from(args.next().expect("output path"));
    let arguments: Vec<_> = args.map(|arg| arg.to_string_lossy().into_owned()).collect();
    let environment: BTreeMap<_, _> = ["MITOS_DEBUG_VALUE", "MITOS_DEBUG_REMOVE"]
        .into_iter()
        .map(|name| (name, std::env::var(name).ok()))
        .collect();
    std::fs::write(
        output,
        serde_json::to_vec(&serde_json::json!({
            "pid":std::process::id(),"cwd":std::env::current_dir()?,"args":arguments,"env":environment
        }))?,
    )?;
    Ok(())
}
