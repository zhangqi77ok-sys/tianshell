fn main() {
    println!("cargo:rerun-if-changed=rust/assets/icon.ico");
    println!("cargo:rerun-if-changed=rust/assets/app.rc");

    #[cfg(target_os = "windows")]
    {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let assets_dir = std::path::Path::new(&manifest_dir).join("rust").join("assets");
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let out_obj = std::path::Path::new(&out_dir).join("app_res.o");

        let status = std::process::Command::new("windres")
            .current_dir(&assets_dir)
            .arg("app.rc")
            .arg("-O")
            .arg("coff")
            .arg("-o")
            .arg(&out_obj)
            .status();

        if let Ok(s) = status {
            if s.success() {
                println!("cargo:rustc-link-arg={}", out_obj.display());
            }
        }
    }
}
