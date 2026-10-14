fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mut resolve = wit_bindgen_core::wit_parser::Resolve::default();
    let (package, _) = resolve.push_dir(&args[1])?;
    let world = resolve.select_world(&[package], Some("plugin"))?;
    let mut files = wit_bindgen_core::Files::default();
    wit_bindgen_c::Opts::default()
        .build()
        .generate(&mut resolve, world, &mut files)?;
    std::fs::create_dir_all(&args[2])?;
    for (name, contents) in files.iter() {
        std::fs::write(std::path::Path::new(&args[2]).join(name), contents)?;
    }
    Ok(())
}
