use std::{env, path::Path};

fn main() {
    if env::var_os("CARGO_FEATURE_BUILTIN").is_none() {
        return;
    }

    for (name, flag) in [
        ("ip-city-packed.lz4", "has_flat_city_pack"),
        ("ip-city-packed-paged.bin", "has_paged_city_pack"),
    ] {
        println!("cargo::rerun-if-changed={name}");
        if Path::new(name).is_file() {
            println!("cargo::rustc-cfg={flag}");
        }
    }
}
