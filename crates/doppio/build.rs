fn main() {
    let proto = std::path::Path::new("..")
        .join("..")
        .join("proto")
        .join("doppio.proto");
    let proto_dir = std::path::Path::new("..").join("..").join("proto");
    println!("cargo:rerun-if-changed={}", proto.display());
    prost_build::Config::new()
        .compile_protos(&[&proto], &[&proto_dir])
        .expect("compile protos");
}
