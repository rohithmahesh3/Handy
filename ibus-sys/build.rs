use std::env;
use std::path::PathBuf;

fn main() {
    let library = pkg_config::Config::new()
        .atleast_version("1.5.0")
        .probe("ibus-1.0")
        .expect("Failed to find ibus-1.0 via pkg-config");

    let glib = pkg_config::Config::new()
        .probe("glib-2.0")
        .expect("Failed to find glib-2.0 via pkg-config");

    let gobject = pkg_config::Config::new()
        .probe("gobject-2.0")
        .expect("Failed to find gobject-2.0 via pkg-config");

    let include_paths: Vec<std::path::PathBuf> = library
        .include_paths
        .iter()
        .chain(glib.include_paths.iter())
        .chain(gobject.include_paths.iter())
        .cloned()
        .collect();

    for path in &include_paths {
        println!("cargo:include={}", path.display());
    }

    println!("cargo:rerun-if-changed=wrapper.c");
    println!("cargo:rerun-if-changed=wrapper.h");

    let mut build = cc::Build::new();
    build.file("wrapper.c");

    for path in &include_paths {
        build.include(path);
    }

    build.define("PKGDATADIR", Some("/usr/share/handy"));
    build.compile("ibus_handy_wrapper");

    println!("cargo:rustc-link-lib=ibus-1.0");
    println!("cargo:rustc-link-lib=glib-2.0");
    println!("cargo:rustc-link-lib=gobject-2.0");
    println!("cargo:rustc-link-lib=gio-2.0");
}
