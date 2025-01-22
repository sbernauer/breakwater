use std::{io::Write, path::PathBuf, process::Command};

fn main() {
    let rustc = std::env::var("RUSTC").unwrap();
    let rustc_version = String::from_utf8(
        Command::new(rustc)
            .arg("--version")
            .output()
            .expect("unable to run rustc")
            .stdout,
    )
    .unwrap();

    let out_path = PathBuf::from(std::env::var("OUT_DIR").expect("`OUT_DIR` is not set"));

    std::fs::File::create(out_path.join("RUSTC_VERSION.txt"))
        .unwrap()
        .write_all(rustc_version.as_bytes())
        .unwrap();
}
