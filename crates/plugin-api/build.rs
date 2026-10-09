use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=wit/plugin.wit");
    let wit = fs::read_to_string("wit/plugin.wit").expect("reading the plugin world");
    let definition = r#"
/// Generate bindings from the packaged plugin world without checkout paths.
#[macro_export]
macro_rules! generate_bindings {
    ($generator:path, $($options:tt)*) => {
        $generator!({ inline: __MITOS_WIT__, $($options)* });
    };
}
"#;
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    fs::write(
        output.join("wit_bindings.rs"),
        definition.replace("__MITOS_WIT__", &format!("{wit:?}")),
    )
    .expect("writing the packaged plugin world");
}
