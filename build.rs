fn main() {
    // Rebuild when the C wrapper or libsensors changes
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/c/sensors_wrapper.c");

    cc::Build::new()
        .include("/usr/include/sensors")
        .include("/usr/include")
        .opt_level(2)
        .include("src/c")
        .file("src/c/sensors_wrapper.c")
        .compile("sensors_wrapper");

    // Link libsensors
    println!("cargo:rustc-link-lib=sensors");
}
