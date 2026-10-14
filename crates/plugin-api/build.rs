use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=wit/plugin.wit");
    let wit = fs::read_to_string("wit/plugin.wit").expect("reading the plugin world");
    let declaration = wit
        .lines()
        .find(|line| line.trim_start().starts_with("package "))
        .expect("versioned plugin WIT package");
    let api_version = declaration
        .trim()
        .strip_suffix(';')
        .and_then(|declaration| declaration.rsplit_once('@'))
        .map(|(_, version)| version)
        .expect("plugin WIT package version");
    let definition = r#"
/// Version of the exact packaged WIT service world, independently of JSON payloads.
pub const SERVICE_API_VERSION: &str = __MITOS_API_VERSION__;

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
        definition
            .replace("__MITOS_WIT__", &format!("{wit:?}"))
            .replace("__MITOS_API_VERSION__", &format!("{api_version:?}")),
    )
    .expect("writing the packaged plugin world");
}
