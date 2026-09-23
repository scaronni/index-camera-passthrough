fn main() {
    // Only the include path is needed from pkg-config: link just the modules used,
    // not every OpenCV module listed in opencv4.pc.
    let opencv = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("opencv4")
        .unwrap();
    cxx_build::bridge("src/lib.rs")
        .file("src/stereo.cpp")
        .includes(&opencv.include_paths)
        .std("c++17")
        .compile("opencv_stereo");
    // The calib3d headers reference features2d and flann, depending on the version.
    for lib in [
        "opencv_calib3d",
        "opencv_features2d",
        "opencv_flann",
        "opencv_imgproc",
        "opencv_core",
    ] {
        println!("cargo:rustc-link-lib={lib}");
    }
    for path in &opencv.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/stereo.cpp");
    println!("cargo:rerun-if-changed=include/stereo.h");
}
