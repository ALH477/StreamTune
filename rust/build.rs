fn main() {
    prost_build::compile_protos(&["src/metadata.proto"], &["src/"]).unwrap();
}
