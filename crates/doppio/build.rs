fn main() {
    // Use the vendored `protoc` so the build doesn't depend on a
    // system-wide protobuf-compiler install. Honours an externally-set
    // `PROTOC` env var if one is provided (e.g. by `nix shell` or distro
    // packaging that wants to use its own protoc).
    if std::env::var_os("PROTOC").is_none() {
        // SAFETY: build script runs single-threaded before any user code.
        unsafe {
            std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path().unwrap());
        }
    }

    let proto = std::path::Path::new("proto").join("doppio.proto");
    let proto_dir = std::path::Path::new("proto");
    println!("cargo:rerun-if-changed={}", proto.display());
    prost_build::Config::new()
        .btree_map(["."])
        .compile_protos(&[&proto], &[&proto_dir])
        .expect("compile protos");
}
