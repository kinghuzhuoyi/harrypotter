fn main() {
    if std::env::var("CARGO_FEATURE_TAKEOVER").is_ok() {
        let quill = concat!(env!("CARGO_MANIFEST_DIR"), "/../quill");
        println!("cargo:rustc-link-search=native={quill}/build");
        println!("cargo:rustc-link-search=native={quill}/vendor");
        println!("cargo:rustc-link-lib=dylib=quill");
        println!("cargo:rustc-link-lib=dylib=qsgepaper");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/home/root/quill:/usr/lib/plugins/scenegraph");
        if let Ok(home) = std::env::var("HOME") {
            let sysroot =
                format!("{home}/rm-sdk-3.26/sysroots/cortexa53-crypto-remarkable-linux/usr/lib");
            println!("cargo:rustc-link-arg=-Wl,-rpath-link,{sysroot}");
        }
    }
}
