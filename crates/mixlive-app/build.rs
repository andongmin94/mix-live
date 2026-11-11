fn main() {
    // gpui-design's native Windows examples use this stack reservation for GPUI layout.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bin=mixlive=/STACK:8000000");
    }
}
