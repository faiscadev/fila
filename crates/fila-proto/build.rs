use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let proto_root = manifest_dir.join("proto");

    let proto_files = [
        proto_root.join("fila/v1/messages.proto"),
        proto_root.join("fila/v1/service.proto"),
        proto_root.join("fila/v1/admin.proto"),
        proto_root.join("fila/v1/cluster.proto"),
    ];

    // Rebuild if any proto file changes.
    for proto in &proto_files {
        println!("cargo:rerun-if-changed={}", proto.display());
    }

    tonic_prost_build::configure()
        // Use bytes::Bytes for payload fields on the hot path (zero-copy
        // reference-counted clones instead of Vec<u8> memcpy).
        .bytes(".fila.v1.Message.payload")
        .bytes(".fila.v1.EnqueueRequest.payload")
        .bytes(".fila.v1.RaftInstallSnapshotRequest.data")
        .compile_protos(
            &proto_files
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            &[proto_root.to_string_lossy().into_owned()],
        )?;
    Ok(())
}
